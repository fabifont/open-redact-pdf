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

fn inflate(data: &[u8]) -> PdfResult<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(data);
    let mut output = Vec::new();
    decoder
        .read_to_end(&mut output)
        .map_err(|error| PdfError::Corrupt(format!("failed to decode flate stream: {error}")))?;
    Ok(output)
}
