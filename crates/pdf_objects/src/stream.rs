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
    let mut decoded = stream.data.clone();
    for filter_name in &filter_names {
        decoded = apply_filter(filter_name, &decoded)?;
    }
    apply_predictor(&decoded, stream.dict.get("DecodeParms"))
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
        other => Err(PdfError::Unsupported(format!(
            "stream filter /{other} is not supported"
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
        dict.insert(
            "Filter".to_string(),
            PdfValue::Name("ASCII85Decode".into()),
        );
        let stream = make_stream(dict, encoded);
        assert_eq!(decode_stream(&stream).unwrap(), b"Man ".to_vec());
    }

    #[test]
    fn decodes_ascii85_z_shortcut() {
        let encoded = b"z~>".to_vec();
        let mut dict = PdfDictionary::new();
        dict.insert(
            "Filter".to_string(),
            PdfValue::Name("ASCII85Decode".into()),
        );
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
}
