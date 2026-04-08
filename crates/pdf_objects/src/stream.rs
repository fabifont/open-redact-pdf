use std::io::Read;

use flate2::read::ZlibDecoder;

use crate::error::{PdfError, PdfResult};
use crate::types::{PdfStream, PdfValue};

pub fn decode_stream(stream: &PdfStream) -> PdfResult<Vec<u8>> {
    match stream.dict.get("Filter") {
        None => Ok(stream.data.clone()),
        Some(PdfValue::Name(name)) if name == "FlateDecode" => inflate(stream.data.as_slice()),
        Some(PdfValue::Array(filters)) if filters.len() == 1 => match filters.first() {
            Some(PdfValue::Name(name)) if name == "FlateDecode" => inflate(stream.data.as_slice()),
            _ => Err(PdfError::Unsupported(
                "only a single FlateDecode filter is supported".to_string(),
            )),
        },
        Some(_) => Err(PdfError::Unsupported(
            "unsupported stream filter configuration".to_string(),
        )),
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
