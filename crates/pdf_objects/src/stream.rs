use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;

use crate::error::{PdfError, PdfResult};
use crate::types::{PdfStream, PdfValue};

/// Compress `data` with FlateDecode (zlib / deflate) at the default
/// compression level. Used by the writer when re-emitting rewritten content
/// streams so the saved PDF does not bloat with plaintext content bytes.
pub fn flate_encode(data: &[u8]) -> PdfResult<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(data)
        .map_err(|error| PdfError::Corrupt(format!("flate encode failed: {error}")))?;
    encoder
        .finish()
        .map_err(|error| PdfError::Corrupt(format!("flate encode finalize failed: {error}")))
}

pub fn decode_stream(stream: &PdfStream) -> PdfResult<Vec<u8>> {
    let filter_names = normalize_filter_list(stream.dict.get("Filter"))?;
    let decode_parms = stream.dict.get("DecodeParms");
    let mut decoded = stream.data.clone();
    for (index, filter_name) in filter_names.iter().enumerate() {
        let is_last = index + 1 == filter_names.len();
        decoded = match filter_name.as_str() {
            // LZW needs `DecodeParms /EarlyChange`; every other filter ignores
            // DecodeParms here because predictors are applied after the chain.
            "LZWDecode" | "LZW" => {
                let early_change = if is_last {
                    lzw_early_change(decode_parms)?
                } else {
                    true
                };
                lzw_decode(&decoded, early_change)?
            }
            _ => apply_filter(filter_name, &decoded)?,
        };
    }
    apply_predictor(&decoded, decode_parms)
}

/// Return the /Filter entry as an ordered list of filter names, whether
/// the source dictionary uses the single-name shorthand or the array
/// form. Empty list means no filters applied (raw data).
fn normalize_filter_list(value: Option<&PdfValue>) -> PdfResult<Vec<String>> {
    match value {
        None => Ok(Vec::new()),
        Some(PdfValue::Null) => Ok(Vec::new()),
        Some(PdfValue::Name(name)) => Ok(vec![name.clone()]),
        Some(PdfValue::Array(items)) => {
            let mut names = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    PdfValue::Name(name) => names.push(name.clone()),
                    _ => {
                        return Err(PdfError::Corrupt(
                            "stream /Filter array contains a non-name entry".to_string(),
                        ));
                    }
                }
            }
            Ok(names)
        }
        Some(_) => Err(PdfError::Corrupt(
            "stream /Filter is neither a name nor an array of names".to_string(),
        )),
    }
}

fn apply_filter(filter: &str, data: &[u8]) -> PdfResult<Vec<u8>> {
    match filter {
        "FlateDecode" | "Fl" => inflate(data),
        "ASCII85Decode" | "A85" => ascii85_decode(data),
        "ASCIIHexDecode" | "AHx" => ascii_hex_decode(data),
        "LZWDecode" | "LZW" => lzw_decode(data, true),
        "RunLengthDecode" | "RL" => run_length_decode(data),
        other => Err(PdfError::Unsupported(format!(
            "stream filter /{other} is not supported"
        ))),
    }
}

/// Read the `DecodeParms /EarlyChange` flag for an LZW stream. PDF
/// defaults to `1` when the entry is missing; `0` disables the
/// one-code-early width switch that TIFF-flavoured LZW implementations
/// use. Any other value is rejected as corrupt so we never silently
/// misalign on an unknown flag.
fn lzw_early_change(decode_parms: Option<&PdfValue>) -> PdfResult<bool> {
    let Some(value) = decode_parms else {
        return Ok(true);
    };
    let dict = match value {
        PdfValue::Dictionary(dict) => dict,
        PdfValue::Null => return Ok(true),
        PdfValue::Array(_) => {
            return Err(PdfError::Unsupported(
                "per-filter DecodeParms arrays are not supported".to_string(),
            ));
        }
        _ => {
            return Err(PdfError::Corrupt(
                "DecodeParms is not a dictionary".to_string(),
            ));
        }
    };
    match dict.get("EarlyChange").and_then(PdfValue::as_integer) {
        None => Ok(true),
        Some(1) => Ok(true),
        Some(0) => Ok(false),
        Some(other) => Err(PdfError::Corrupt(format!(
            "unsupported LZW EarlyChange value {other}"
        ))),
    }
}

/// Maximum decompressed stream size (256 MiB). Prevents decompression bombs from
/// exhausting memory in WASM or native contexts.
const MAX_DECOMPRESSED_SIZE: u64 = 256 * 1024 * 1024;

fn inflate(data: &[u8]) -> PdfResult<Vec<u8>> {
    let decoder = ZlibDecoder::new(data);
    let mut output = Vec::new();
    decoder
        .take(MAX_DECOMPRESSED_SIZE + 1)
        .read_to_end(&mut output)
        .map_err(|error| PdfError::Corrupt(format!("failed to decode flate stream: {error}")))?;
    if output.len() as u64 > MAX_DECOMPRESSED_SIZE {
        return Err(PdfError::Corrupt(
            "decompressed stream exceeds maximum allowed size".to_string(),
        ));
    }
    Ok(output)
}

/// Decode an LZW-encoded byte run (PDF § 7.4.4). Uses the TIFF-compatible
/// variable-width code flavour: 9–12-bit codes, 256 = CLEAR, 257 = EOD,
/// the literal dictionary seeds indices 0–255, and growth starts at 258.
/// `early_change` mirrors `DecodeParms /EarlyChange` — `true` (the PDF
/// default) switches to a wider code one entry before the dictionary is
/// fully populated at the current width.
fn lzw_decode(data: &[u8], early_change: bool) -> PdfResult<Vec<u8>> {
    const CLEAR: u32 = 256;
    const EOD: u32 = 257;
    const MAX_WIDTH: u32 = 12;
    let width_threshold = |width: u32| {
        if early_change {
            (1u32 << width) - 1
        } else {
            1u32 << width
        }
    };

    let mut reader = BitReader::new(data);
    let mut dict: Vec<Vec<u8>> = Vec::with_capacity(1 << MAX_WIDTH);
    let reset_dict = |dict: &mut Vec<Vec<u8>>| {
        dict.clear();
        for byte in 0u32..256 {
            dict.push(vec![byte as u8]);
        }
        dict.push(Vec::new()); // 256 — placeholder for CLEAR
        dict.push(Vec::new()); // 257 — placeholder for EOD
    };
    reset_dict(&mut dict);

    let mut output: Vec<u8> = Vec::new();
    let mut code_width: u32 = 9;
    let mut previous: Option<Vec<u8>> = None;
    loop {
        let Some(code) = reader.read_bits(code_width) else {
            break;
        };
        if code == EOD {
            break;
        }
        if code == CLEAR {
            reset_dict(&mut dict);
            code_width = 9;
            previous = None;
            continue;
        }
        let entry = if (code as usize) < dict.len() {
            let entry = dict[code as usize].clone();
            if entry.is_empty() {
                return Err(PdfError::Corrupt(format!(
                    "LZW code {code} references placeholder entry"
                )));
            }
            entry
        } else if code as usize == dict.len() {
            // Standard LZW K+K[0] special case: the code points at the
            // entry we are about to add, so reconstruct it from the
            // previous entry plus its own first byte.
            let prev = previous.clone().ok_or_else(|| {
                PdfError::Corrupt("LZW code out of sequence".to_string())
            })?;
            let first = *prev.first().ok_or_else(|| {
                PdfError::Corrupt("LZW previous entry was empty".to_string())
            })?;
            let mut entry = prev;
            entry.push(first);
            entry
        } else {
            return Err(PdfError::Corrupt(format!(
                "LZW code {code} outside dictionary"
            )));
        };
        if output.len() + entry.len() > MAX_DECOMPRESSED_SIZE as usize {
            return Err(PdfError::Corrupt(
                "decompressed stream exceeds maximum allowed size".to_string(),
            ));
        }
        output.extend_from_slice(&entry);
        if let Some(prev_entry) = previous.take() {
            let mut new_entry = prev_entry;
            new_entry.push(entry[0]);
            if dict.len() < (1 << MAX_WIDTH) {
                dict.push(new_entry);
            }
            // The encoder bumps width against `next_code` (the index of the
            // slot just filled, i.e. `dict.len()` here) AFTER the insert.
            // The decoder trails the encoder by one dictionary entry — the
            // push for code N happens while processing code N+1 — so the
            // decoder has to compare against `dict.len() + 1` to bump width
            // at the same boundary the encoder did.
            if (dict.len() as u32).saturating_add(1) >= width_threshold(code_width)
                && code_width < MAX_WIDTH
            {
                code_width += 1;
            }
        }
        previous = Some(entry);
    }
    Ok(output)
}

/// MSB-first bit reader used by the LZW decoder. The PDF spec § 7.4.4.2
/// states codes are packed "with the high-order bit of each code
/// appearing first", so bytes are consumed from the front of the stream
/// and codes are shifted out from the top of an accumulating buffer.
/// When the backing data runs out mid-code, the remaining bits are
/// zero-padded — matching the encoder contract in § 7.4.4.3.
struct BitReader<'a> {
    data: &'a [u8],
    byte_index: usize,
    bit_buffer: u32,
    bit_count: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader {
            data,
            byte_index: 0,
            bit_buffer: 0,
            bit_count: 0,
        }
    }

    fn read_bits(&mut self, width: u32) -> Option<u32> {
        while self.bit_count < width {
            if self.byte_index >= self.data.len() {
                if self.bit_count == 0 {
                    return None;
                }
                // Pad with zero bits to flush the final partial code.
                let pad = width - self.bit_count;
                self.bit_buffer <<= pad;
                let mask = (1u32 << width) - 1;
                let code = self.bit_buffer & mask;
                self.bit_count = 0;
                self.bit_buffer = 0;
                return Some(code);
            }
            self.bit_buffer = (self.bit_buffer << 8) | u32::from(self.data[self.byte_index]);
            self.byte_index += 1;
            self.bit_count += 8;
        }
        self.bit_count -= width;
        let mask = (1u32 << width) - 1;
        let code = (self.bit_buffer >> self.bit_count) & mask;
        self.bit_buffer &= (1u32 << self.bit_count) - 1;
        Some(code)
    }
}

/// Decode an ASCII85-encoded byte run (PDF § 7.4.3). Whitespace is
/// ignored, `z` expands to four zero bytes, and `~>` terminates the
/// stream; a short final group is padded with `u` and the decoded
/// tail is truncated accordingly.
fn ascii85_decode(data: &[u8]) -> PdfResult<Vec<u8>> {
    let mut output = Vec::with_capacity(data.len());
    let mut group = [0u8; 5];
    let mut group_len = 0usize;

    for &byte in data {
        if byte == b'~' {
            break; // `~>` EOD marker; the `>` is allowed to follow or be absent.
        }
        if matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0C) {
            continue;
        }
        if byte == b'z' {
            if group_len != 0 {
                return Err(PdfError::Corrupt(
                    "ASCII85 'z' shortcut inside a partial group".to_string(),
                ));
            }
            output.extend_from_slice(&[0u8; 4]);
            continue;
        }
        if !(b'!'..=b'u').contains(&byte) {
            return Err(PdfError::Corrupt(format!(
                "invalid ASCII85 byte 0x{byte:02X}"
            )));
        }
        group[group_len] = byte - b'!';
        group_len += 1;
        if group_len == 5 {
            let value = (group[0] as u64) * 85u64.pow(4)
                + (group[1] as u64) * 85u64.pow(3)
                + (group[2] as u64) * 85u64.pow(2)
                + (group[3] as u64) * 85
                + (group[4] as u64);
            if value > u32::MAX as u64 {
                return Err(PdfError::Corrupt(
                    "ASCII85 group value exceeds 32 bits".to_string(),
                ));
            }
            output.extend_from_slice(&(value as u32).to_be_bytes());
            group_len = 0;
        }
    }

    if group_len > 0 {
        if group_len == 1 {
            return Err(PdfError::Corrupt(
                "ASCII85 final group contains a single byte".to_string(),
            ));
        }
        // Pad with the max digit so truncating yields the right tail.
        for entry in group.iter_mut().skip(group_len) {
            *entry = 84;
        }
        let value = (group[0] as u64) * 85u64.pow(4)
            + (group[1] as u64) * 85u64.pow(3)
            + (group[2] as u64) * 85u64.pow(2)
            + (group[3] as u64) * 85
            + (group[4] as u64);
        let bytes = (value as u32).to_be_bytes();
        output.extend_from_slice(&bytes[..group_len - 1]);
    }

    Ok(output)
}

/// Decode an ASCIIHex-encoded byte run (PDF § 7.4.2). Whitespace is
/// ignored, `>` terminates the stream, and a trailing odd nibble is
/// treated as if followed by `0`.
fn ascii_hex_decode(data: &[u8]) -> PdfResult<Vec<u8>> {
    let mut output = Vec::with_capacity(data.len() / 2 + 1);
    let mut high: Option<u8> = None;
    for &byte in data {
        if byte == b'>' {
            break;
        }
        if matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0C) {
            continue;
        }
        let nibble = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => {
                return Err(PdfError::Corrupt(format!(
                    "invalid ASCIIHex byte 0x{byte:02X}"
                )));
            }
        };
        match high.take() {
            None => high = Some(nibble),
            Some(h) => output.push((h << 4) | nibble),
        }
    }
    if let Some(h) = high {
        output.push(h << 4);
    }
    Ok(output)
}

/// Decode a RunLengthDecode byte run (PDF § 7.4.5). Each control byte `L`
/// either introduces a literal run (`0..=127` → copy `L+1` bytes), a
/// repeated byte (`129..=255` → repeat the next byte `257-L` times), or
/// the end-of-data marker (`128`). A stream that ends before the EOD
/// marker is accepted — some producers omit it — but truncated literal
/// or repeat runs are treated as corruption.
fn run_length_decode(data: &[u8]) -> PdfResult<Vec<u8>> {
    let mut output: Vec<u8> = Vec::with_capacity(data.len());
    let mut index = 0usize;
    while index < data.len() {
        let length_byte = data[index];
        index += 1;
        if length_byte == 128 {
            return Ok(output);
        }
        if length_byte < 128 {
            let run_len = usize::from(length_byte) + 1;
            let end = index
                .checked_add(run_len)
                .ok_or_else(|| PdfError::Corrupt("RunLengthDecode index overflow".to_string()))?;
            if end > data.len() {
                return Err(PdfError::Corrupt(
                    "RunLengthDecode literal run runs past end of stream".to_string(),
                ));
            }
            output.extend_from_slice(&data[index..end]);
            index = end;
        } else {
            let repeat = 257usize - usize::from(length_byte);
            if index >= data.len() {
                return Err(PdfError::Corrupt(
                    "RunLengthDecode repeat run is missing its payload byte".to_string(),
                ));
            }
            let byte = data[index];
            index += 1;
            output.extend(std::iter::repeat_n(byte, repeat));
        }
        if output.len() as u64 > MAX_DECOMPRESSED_SIZE {
            return Err(PdfError::Corrupt(
                "decompressed stream exceeds maximum allowed size".to_string(),
            ));
        }
    }
    Ok(output)
}

fn apply_predictor(data: &[u8], decode_parms: Option<&PdfValue>) -> PdfResult<Vec<u8>> {
    let parms = match decode_parms {
        None => return Ok(data.to_vec()),
        Some(PdfValue::Dictionary(dict)) => dict,
        Some(PdfValue::Null) => return Ok(data.to_vec()),
        Some(PdfValue::Array(_)) => {
            // Per-filter DecodeParms arrays are legal when multiple filters are
            // chained. We only support a single FlateDecode filter today so any
            // array-valued DecodeParms is unexpected.
            return Err(PdfError::Unsupported(
                "per-filter DecodeParms arrays are not supported".to_string(),
            ));
        }
        Some(_) => {
            return Err(PdfError::Corrupt(
                "DecodeParms is not a dictionary".to_string(),
            ));
        }
    };

    let predictor = parms
        .get("Predictor")
        .and_then(PdfValue::as_integer)
        .unwrap_or(1);
    match predictor {
        1 => Ok(data.to_vec()),
        2 => tiff_predictor_decode(data, parms),
        10..=15 => png_predictor_decode(data, parms),
        other => Err(PdfError::Unsupported(format!(
            "predictor {other} is not supported"
        ))),
    }
}

fn tiff_predictor_decode(data: &[u8], parms: &crate::types::PdfDictionary) -> PdfResult<Vec<u8>> {
    let columns = parms
        .get("Columns")
        .and_then(PdfValue::as_integer)
        .unwrap_or(1) as usize;
    let colors = parms
        .get("Colors")
        .and_then(PdfValue::as_integer)
        .unwrap_or(1) as usize;
    let bits_per_component = parms
        .get("BitsPerComponent")
        .and_then(PdfValue::as_integer)
        .unwrap_or(8) as usize;

    if bits_per_component != 8 {
        return Err(PdfError::Unsupported(format!(
            "TIFF predictor with BitsPerComponent {bits_per_component} is not supported"
        )));
    }
    if columns == 0 || colors == 0 {
        return Err(PdfError::Corrupt(
            "TIFF predictor Columns/Colors must be positive".to_string(),
        ));
    }
    let row_stride = columns * colors;
    if data.len() % row_stride != 0 {
        return Err(PdfError::Corrupt(format!(
            "TIFF predictor row length mismatch: data={} stride={row_stride}",
            data.len()
        )));
    }
    let mut output = Vec::with_capacity(data.len());
    for row in data.chunks_exact(row_stride) {
        for (component_index, byte) in row.iter().enumerate() {
            if component_index < colors {
                // First pixel in a row is stored as-is per component.
                output.push(*byte);
            } else {
                let previous = output[output.len() - colors];
                output.push(previous.wrapping_add(*byte));
            }
        }
    }
    Ok(output)
}

fn png_predictor_decode(data: &[u8], parms: &crate::types::PdfDictionary) -> PdfResult<Vec<u8>> {
    let columns = parms
        .get("Columns")
        .and_then(PdfValue::as_integer)
        .unwrap_or(1) as usize;
    let colors = parms
        .get("Colors")
        .and_then(PdfValue::as_integer)
        .unwrap_or(1) as usize;
    let bits_per_component = parms
        .get("BitsPerComponent")
        .and_then(PdfValue::as_integer)
        .unwrap_or(8) as usize;

    if bits_per_component != 8 {
        return Err(PdfError::Unsupported(format!(
            "PNG predictor with BitsPerComponent {bits_per_component} is not supported"
        )));
    }
    if columns == 0 || colors == 0 {
        return Err(PdfError::Corrupt(
            "PNG predictor Columns/Colors must be positive".to_string(),
        ));
    }
    let bytes_per_pixel = colors; // bits_per_component == 8
    let row_data_len = columns * bytes_per_pixel;
    let row_stride = row_data_len + 1; // leading filter byte

    if data.len() % row_stride != 0 {
        return Err(PdfError::Corrupt(format!(
            "PNG predictor row length mismatch: data={} stride={row_stride}",
            data.len()
        )));
    }
    let row_count = data.len() / row_stride;
    let mut output = Vec::with_capacity(row_count * row_data_len);
    let mut prev_row = vec![0u8; row_data_len];
    let mut row = vec![0u8; row_data_len];

    for r in 0..row_count {
        let base = r * row_stride;
        let filter = data[base];
        let src = &data[base + 1..base + row_stride];
        row.copy_from_slice(src);
        match filter {
            0 => {} // None
            1 => {
                // Sub
                for i in 0..row_data_len {
                    let left = if i >= bytes_per_pixel {
                        row[i - bytes_per_pixel]
                    } else {
                        0
                    };
                    row[i] = row[i].wrapping_add(left);
                }
            }
            2 => {
                // Up
                for i in 0..row_data_len {
                    row[i] = row[i].wrapping_add(prev_row[i]);
                }
            }
            3 => {
                // Average
                for i in 0..row_data_len {
                    let left = if i >= bytes_per_pixel {
                        row[i - bytes_per_pixel]
                    } else {
                        0
                    };
                    let up = prev_row[i];
                    let avg = ((left as u16 + up as u16) / 2) as u8;
                    row[i] = row[i].wrapping_add(avg);
                }
            }
            4 => {
                // Paeth
                for i in 0..row_data_len {
                    let left = if i >= bytes_per_pixel {
                        row[i - bytes_per_pixel]
                    } else {
                        0
                    };
                    let up = prev_row[i];
                    let up_left = if i >= bytes_per_pixel {
                        prev_row[i - bytes_per_pixel]
                    } else {
                        0
                    };
                    row[i] = row[i].wrapping_add(paeth(left, up, up_left));
                }
            }
            other => {
                return Err(PdfError::Corrupt(format!(
                    "unknown PNG row filter type {other}"
                )));
            }
        }
        output.extend_from_slice(&row);
        prev_row.copy_from_slice(&row);
    }

    Ok(output)
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i32 + b as i32 - c as i32;
    let pa = (p - a as i32).abs();
    let pb = (p - b as i32).abs();
    let pc = (p - c as i32).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{PdfDictionary, PdfStream, PdfValue};
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write;

    fn make_stream(dict: PdfDictionary, data: Vec<u8>) -> PdfStream {
        PdfStream { dict, data }
    }

    #[test]
    fn passthrough_when_no_filter() {
        let dict = PdfDictionary::new();
        let stream = make_stream(dict, vec![1, 2, 3]);
        assert_eq!(decode_stream(&stream).unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn inflates_flate_decode() {
        let raw = b"hello world";
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(raw).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("FlateDecode".into()));
        let stream = make_stream(dict, compressed);
        assert_eq!(decode_stream(&stream).unwrap(), raw.to_vec());
    }

    #[test]
    fn applies_png_up_predictor() {
        // Original 2 rows of 4 bytes each.
        let original: [u8; 8] = [10, 20, 30, 40, 15, 22, 33, 44];

        // Encode with filter type 2 (Up) on row 2, type 0 on row 1.
        let mut encoded = Vec::new();
        encoded.push(0); // row 0: None
        encoded.extend_from_slice(&original[0..4]);
        encoded.push(2); // row 1: Up
        let diff: Vec<u8> = original[4..8]
            .iter()
            .zip(original[0..4].iter())
            .map(|(v, up)| v.wrapping_sub(*up))
            .collect();
        encoded.extend_from_slice(&diff);

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&encoded).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("FlateDecode".into()));
        let mut parms = PdfDictionary::new();
        parms.insert("Predictor".to_string(), PdfValue::Integer(12));
        parms.insert("Columns".to_string(), PdfValue::Integer(4));
        dict.insert("DecodeParms".to_string(), PdfValue::Dictionary(parms));

        let stream = make_stream(dict, compressed);
        let decoded = decode_stream(&stream).expect("decode");
        assert_eq!(decoded, original.to_vec());
    }

    #[test]
    fn applies_tiff_predictor() {
        // Original 2 rows of 4 bytes each, 1 color, 8 bits per component.
        let original: [u8; 8] = [10, 20, 30, 40, 15, 22, 33, 44];

        // TIFF predictor encodes each row independently: first byte as-is,
        // subsequent bytes as (current - previous). No filter byte prefix.
        let mut encoded = Vec::new();
        for row in original.chunks(4) {
            encoded.push(row[0]);
            for index in 1..row.len() {
                encoded.push(row[index].wrapping_sub(row[index - 1]));
            }
        }

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&encoded).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("FlateDecode".into()));
        let mut parms = PdfDictionary::new();
        parms.insert("Predictor".to_string(), PdfValue::Integer(2));
        parms.insert("Columns".to_string(), PdfValue::Integer(4));
        dict.insert("DecodeParms".to_string(), PdfValue::Dictionary(parms));

        let stream = make_stream(dict, compressed);
        let decoded = decode_stream(&stream).expect("decode");
        assert_eq!(decoded, original.to_vec());
    }

    #[test]
    fn decodes_ascii85_full_group() {
        // Full 4-byte group "Man " → ASCII85 "9jqo^".
        let encoded = b"9jqo^~>".to_vec();
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("ASCII85Decode".into()));
        let stream = make_stream(dict, encoded);
        assert_eq!(decode_stream(&stream).unwrap(), b"Man ".to_vec());
    }

    #[test]
    fn decodes_ascii85_z_shortcut() {
        let encoded = b"z~>".to_vec();
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("ASCII85Decode".into()));
        let stream = make_stream(dict, encoded);
        assert_eq!(decode_stream(&stream).unwrap(), vec![0, 0, 0, 0]);
    }

    #[test]
    fn decodes_filter_chain_ascii85_then_flate() {
        // Encode plaintext with FlateDecode first, then ASCII85 wrap. The
        // order the filter list uses is the DECODE order, so reading the
        // stream applies ASCII85 first and FlateDecode second — the same
        // order we use to produce the bytes in reverse.
        let plaintext = b"PdfStreamFilterChainTest".to_vec();
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&plaintext).unwrap();
        let flate_bytes = encoder.finish().unwrap();

        // ASCII85 encode the FlateDecode payload.
        let mut ascii85 = String::new();
        for chunk in flate_bytes.chunks(4) {
            let mut buf = [0u8; 4];
            buf[..chunk.len()].copy_from_slice(chunk);
            let value = u32::from_be_bytes(buf);
            if chunk.len() == 4 && value == 0 {
                ascii85.push('z');
                continue;
            }
            let mut digits = [0u8; 5];
            let mut v = value as u64;
            for i in (0..5).rev() {
                digits[i] = (v % 85) as u8 + b'!';
                v /= 85;
            }
            let take = chunk.len() + 1;
            for &digit in &digits[..take] {
                ascii85.push(digit as char);
            }
        }
        ascii85.push_str("~>");

        let mut dict = PdfDictionary::new();
        dict.insert(
            "Filter".to_string(),
            PdfValue::Array(vec![
                PdfValue::Name("ASCII85Decode".into()),
                PdfValue::Name("FlateDecode".into()),
            ]),
        );
        let stream = make_stream(dict, ascii85.into_bytes());
        assert_eq!(decode_stream(&stream).unwrap(), plaintext);
    }

    #[test]
    fn decodes_ascii_hex() {
        let encoded = b"48656C6C6F>".to_vec();
        let mut dict = PdfDictionary::new();
        dict.insert(
            "Filter".to_string(),
            PdfValue::Name("ASCIIHexDecode".into()),
        );
        let stream = make_stream(dict, encoded);
        assert_eq!(decode_stream(&stream).unwrap(), b"Hello".to_vec());
    }

    #[test]
    fn rejects_unsupported_predictor() {
        let mut dict = PdfDictionary::new();
        let mut parms = PdfDictionary::new();
        parms.insert("Predictor".to_string(), PdfValue::Integer(3));
        dict.insert("DecodeParms".to_string(), PdfValue::Dictionary(parms));
        let stream = make_stream(dict, vec![0, 0, 0, 0]);
        match decode_stream(&stream) {
            Err(PdfError::Unsupported(msg)) => {
                assert!(msg.contains("predictor"), "got: {msg}")
            }
            other => panic!("expected Unsupported, got: {other:?}"),
        }
    }

    /// Encode `input` with the TIFF-compatible LZW variant used by PDF
    /// (9–12-bit codes, 256 = CLEAR, 257 = EOD). `early_change` mirrors
    /// `DecodeParms /EarlyChange`: `true` switches width one code earlier.
    fn encode_lzw(input: &[u8], early_change: bool) -> Vec<u8> {
        use std::collections::HashMap;

        let mut out: Vec<u8> = Vec::new();
        let mut bit_buffer: u64 = 0;
        let mut bit_count: u32 = 0;
        let flush_code = |code: u32,
                          width: u32,
                          bit_buffer: &mut u64,
                          bit_count: &mut u32,
                          out: &mut Vec<u8>| {
            *bit_buffer = (*bit_buffer << width) | u64::from(code);
            *bit_count += width;
            while *bit_count >= 8 {
                *bit_count -= 8;
                out.push(((*bit_buffer >> *bit_count) & 0xFF) as u8);
                *bit_buffer &= (1u64 << *bit_count) - 1;
            }
        };

        // Start every stream with CLEAR.
        flush_code(256, 9, &mut bit_buffer, &mut bit_count, &mut out);

        let mut dict: HashMap<Vec<u8>, u32> = HashMap::new();
        for b in 0u32..256 {
            dict.insert(vec![b as u8], b);
        }
        let mut next_code: u32 = 258;
        let mut code_width: u32 = 9;

        let mut buffer: Vec<u8> = Vec::new();
        for &byte in input {
            let mut extended = buffer.clone();
            extended.push(byte);
            if dict.contains_key(&extended) {
                buffer = extended;
            } else {
                let code = dict[&buffer];
                flush_code(code, code_width, &mut bit_buffer, &mut bit_count, &mut out);
                dict.insert(extended, next_code);
                next_code += 1;
                let threshold = if early_change {
                    (1u32 << code_width) - 1
                } else {
                    1u32 << code_width
                };
                if next_code >= threshold && code_width < 12 {
                    code_width += 1;
                }
                buffer = vec![byte];
            }
        }
        if !buffer.is_empty() {
            let code = dict[&buffer];
            flush_code(code, code_width, &mut bit_buffer, &mut bit_count, &mut out);
        }
        flush_code(257, code_width, &mut bit_buffer, &mut bit_count, &mut out);
        if bit_count > 0 {
            out.push(((bit_buffer << (8 - bit_count)) & 0xFF) as u8);
        }
        out
    }

    #[test]
    fn decodes_lzw_spec_example() {
        // PDF 1.7 spec § 7.4.4.3, Annex A.3: "-----A---B" encodes to the
        // 8 nine-bit codes 256, 45, 258, 258, 65, 259, 66, 257, which pack
        // MSB-first into these nine bytes.
        let data = vec![0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("LZWDecode".into()));
        let stream = make_stream(dict, data);
        assert_eq!(decode_stream(&stream).unwrap(), b"-----A---B".to_vec());
    }

    #[test]
    fn decodes_lzw_roundtrip_default_early_change() {
        let plaintext = b"the quick brown fox jumps over the lazy dog".to_vec();
        let encoded = encode_lzw(&plaintext, true);
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("LZWDecode".into()));
        let stream = make_stream(dict, encoded);
        assert_eq!(decode_stream(&stream).unwrap(), plaintext);
    }

    #[test]
    fn decodes_lzw_roundtrip_early_change_zero() {
        let plaintext = b"the quick brown fox jumps over the lazy dog".to_vec();
        let encoded = encode_lzw(&plaintext, false);
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("LZWDecode".into()));
        let mut parms = PdfDictionary::new();
        parms.insert("EarlyChange".to_string(), PdfValue::Integer(0));
        dict.insert("DecodeParms".to_string(), PdfValue::Dictionary(parms));
        let stream = make_stream(dict, encoded);
        assert_eq!(decode_stream(&stream).unwrap(), plaintext);
    }

    #[test]
    fn decodes_lzw_with_tiff_predictor() {
        // Original 2 rows × 4 bytes, 1 colour, 8 bits per component.
        // TIFF predictor leaves the first byte per row as-is and stores
        // the rest as delta from the previous byte. The LZW filter sits
        // on top: it compresses the predictor-encoded bytes.
        let original: [u8; 8] = [10, 20, 30, 40, 15, 22, 33, 44];
        let mut predictor_encoded = Vec::new();
        for row in original.chunks(4) {
            predictor_encoded.push(row[0]);
            for index in 1..row.len() {
                predictor_encoded.push(row[index].wrapping_sub(row[index - 1]));
            }
        }
        let lzw_bytes = encode_lzw(&predictor_encoded, true);
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("LZWDecode".into()));
        let mut parms = PdfDictionary::new();
        parms.insert("Predictor".to_string(), PdfValue::Integer(2));
        parms.insert("Columns".to_string(), PdfValue::Integer(4));
        dict.insert("DecodeParms".to_string(), PdfValue::Dictionary(parms));
        let stream = make_stream(dict, lzw_bytes);
        assert_eq!(decode_stream(&stream).unwrap(), original.to_vec());
    }

    #[test]
    fn decodes_lzw_exercises_code_width_transitions() {
        // Build an input long enough to force the dictionary past 511
        // entries so the decoder exercises the 9→10 and 10→11 bit width
        // transitions. ~1200 unique trigrams from a pangram-ish repeat
        // suffices.
        let mut plaintext = Vec::new();
        for i in 0u16..1200 {
            plaintext.push(b'a' + (i % 26) as u8);
            plaintext.push(b'A' + (i % 26) as u8);
            plaintext.push(b'0' + (i % 10) as u8);
        }
        let encoded = encode_lzw(&plaintext, true);
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("LZWDecode".into()));
        let stream = make_stream(dict, encoded);
        assert_eq!(decode_stream(&stream).unwrap(), plaintext);
    }

    #[test]
    fn decodes_run_length_literal_runs() {
        // Length byte 2 means "copy next 3 bytes literally". EOD = 128.
        let encoded = vec![2, b'A', b'B', b'C', 128];
        let mut dict = PdfDictionary::new();
        dict.insert(
            "Filter".to_string(),
            PdfValue::Name("RunLengthDecode".into()),
        );
        let stream = make_stream(dict, encoded);
        assert_eq!(decode_stream(&stream).unwrap(), b"ABC".to_vec());
    }

    #[test]
    fn decodes_run_length_repeat_runs() {
        // Length byte 0xFF (255) means "repeat next byte (257-255)=2 times".
        let encoded = vec![0xFF, b'Z', 128];
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("RL".into()));
        let stream = make_stream(dict, encoded);
        assert_eq!(decode_stream(&stream).unwrap(), b"ZZ".to_vec());
    }

    #[test]
    fn decodes_run_length_mixed_runs_without_eod() {
        // "ABBBCD" packed as literal A, repeat B x3, literal CD. No trailing
        // EOD byte — some producers omit it and we treat that as end of
        // stream rather than corruption.
        let encoded = vec![0, b'A', 0xFE, b'B', 1, b'C', b'D'];
        let mut dict = PdfDictionary::new();
        dict.insert(
            "Filter".to_string(),
            PdfValue::Name("RunLengthDecode".into()),
        );
        let stream = make_stream(dict, encoded);
        assert_eq!(decode_stream(&stream).unwrap(), b"ABBBCD".to_vec());
    }

    #[test]
    fn rejects_run_length_truncated_literal_run() {
        // Length byte 3 claims 4 bytes of literal but only 2 follow.
        let encoded = vec![3, b'A', b'B'];
        let mut dict = PdfDictionary::new();
        dict.insert(
            "Filter".to_string(),
            PdfValue::Name("RunLengthDecode".into()),
        );
        let stream = make_stream(dict, encoded);
        let err = decode_stream(&stream).unwrap_err();
        assert!(matches!(err, PdfError::Corrupt(_)), "got: {err:?}");
    }

    #[test]
    fn rejects_run_length_truncated_repeat_run() {
        // Length byte 200 implies a repeat with a payload byte, but the
        // payload is missing (stream ends immediately after the length).
        let encoded = vec![200];
        let mut dict = PdfDictionary::new();
        dict.insert(
            "Filter".to_string(),
            PdfValue::Name("RunLengthDecode".into()),
        );
        let stream = make_stream(dict, encoded);
        let err = decode_stream(&stream).unwrap_err();
        assert!(matches!(err, PdfError::Corrupt(_)), "got: {err:?}");
    }

    #[test]
    fn rejects_lzw_out_of_range_code() {
        // Single 9-bit code 0x1FF (= 511) after a CLEAR is outside the
        // still-256-entry dictionary and not equal to `next_code` yet,
        // so the decoder must refuse rather than silently emit.
        let mut out: Vec<u8> = Vec::new();
        let mut bit_buffer: u64 = 0;
        let mut bit_count: u32 = 0;
        let mut push = |code: u32, width: u32| {
            bit_buffer = (bit_buffer << width) | u64::from(code);
            bit_count += width;
            while bit_count >= 8 {
                bit_count -= 8;
                out.push(((bit_buffer >> bit_count) & 0xFF) as u8);
                bit_buffer &= (1u64 << bit_count) - 1;
            }
        };
        push(256, 9); // CLEAR
        push(511, 9); // invalid
        if bit_count > 0 {
            out.push(((bit_buffer << (8 - bit_count)) & 0xFF) as u8);
        }
        let mut dict = PdfDictionary::new();
        dict.insert("Filter".to_string(), PdfValue::Name("LZWDecode".into()));
        let stream = make_stream(dict, out);
        let err = decode_stream(&stream).unwrap_err();
        assert!(matches!(err, PdfError::Corrupt(_)), "got: {err:?}");
    }
}
