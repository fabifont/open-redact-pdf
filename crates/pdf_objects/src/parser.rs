use std::collections::BTreeMap;

use crate::document::build_document;
use crate::error::{PdfError, PdfResult};
use crate::types::{
    ObjectRef, PdfDictionary, PdfFile, PdfObject, PdfStream, PdfString, PdfValue, XrefEntry,
};

pub fn parse_pdf(bytes: &[u8]) -> PdfResult<crate::document::ParsedDocument> {
    let version = parse_header(bytes)?;
    let startxref = find_startxref(bytes)?;
    let (xref, trailer) = parse_xref_table(bytes, startxref)?;
    if trailer.contains_key("XRefStm") {
        return Err(PdfError::Unsupported(
            "xref streams are not supported".to_string(),
        ));
    }

    let mut objects = BTreeMap::new();
    let mut max_object_number = 0;
    for (object_ref, entry) in xref {
        if !entry.in_use {
            continue;
        }
        if object_ref.object_number == 0 {
            continue;
        }
        let object = parse_indirect_object(bytes, entry.offset)?;
        max_object_number = max_object_number.max(object_ref.object_number);
        objects.insert(object_ref, object);
    }
    let file = PdfFile {
        version,
        objects,
        trailer,
        max_object_number,
    };
    build_document(file)
}

fn parse_header(bytes: &[u8]) -> PdfResult<String> {
    if !bytes.starts_with(b"%PDF-") {
        return Err(PdfError::Parse("missing PDF header".to_string()));
    }
    let line_end = bytes
        .iter()
        .position(|byte| *byte == b'\n' || *byte == b'\r')
        .ok_or_else(|| PdfError::Parse("unterminated header".to_string()))?;
    Ok(String::from_utf8_lossy(&bytes[5..line_end])
        .trim()
        .to_string())
}

fn find_startxref(bytes: &[u8]) -> PdfResult<usize> {
    let marker = b"startxref";
    let position = bytes
        .windows(marker.len())
        .rposition(|window| window == marker)
        .ok_or_else(|| PdfError::Parse("missing startxref".to_string()))?;
    let mut parser = Cursor::new(bytes, position + marker.len());
    parser.skip_ws_and_comments();
    parser.parse_usize()
}

fn parse_xref_table(
    bytes: &[u8],
    offset: usize,
) -> PdfResult<(BTreeMap<ObjectRef, XrefEntry>, PdfDictionary)> {
    let mut cursor = Cursor::new(bytes, offset);
    cursor.expect_keyword("xref")?;
    let mut entries = BTreeMap::new();
    loop {
        cursor.skip_ws_and_comments();
        if cursor.peek_keyword("trailer") {
            break;
        }
        let start = cursor.parse_u32()?;
        cursor.skip_ws_and_comments();
        let count = cursor.parse_u32()?;
        cursor.skip_line_breaks();
        for index in 0..count {
            let line = cursor.read_line()?;
            if line.len() < 17 {
                return Err(PdfError::Parse("invalid xref entry".to_string()));
            }
            let parts = String::from_utf8_lossy(line).trim().to_string();
            let mut fields = parts.split_whitespace();
            let offset = fields
                .next()
                .ok_or_else(|| PdfError::Parse("invalid xref entry offset".to_string()))?
                .parse::<usize>()
                .map_err(|_| PdfError::Parse("invalid xref entry offset".to_string()))?;
            let generation = fields
                .next()
                .ok_or_else(|| PdfError::Parse("invalid xref generation".to_string()))?
                .parse::<u16>()
                .map_err(|_| PdfError::Parse("invalid xref generation".to_string()))?;
            let flag = fields
                .next()
                .ok_or_else(|| PdfError::Parse("invalid xref flag".to_string()))?;
            let object_number = start
                .checked_add(index)
                .ok_or_else(|| PdfError::Parse("xref object number overflow".to_string()))?;
            entries.insert(
                ObjectRef::new(object_number, generation),
                XrefEntry {
                    offset,
                    generation,
                    in_use: flag == "n",
                },
            );
        }
    }
    cursor.expect_keyword("trailer")?;
    let trailer = match cursor.parse_value()? {
        PdfValue::Dictionary(dictionary) => dictionary,
        _ => return Err(PdfError::Parse("trailer is not a dictionary".to_string())),
    };
    if trailer.contains_key("Prev") {
        return Err(PdfError::Unsupported(
            "incremental update chains are not supported".to_string(),
        ));
    }
    Ok((entries, trailer))
}

fn parse_indirect_object(bytes: &[u8], offset: usize) -> PdfResult<PdfObject> {
    let mut cursor = Cursor::new(bytes, offset);
    let _object_number = cursor.parse_u32()?;
    cursor.skip_ws_and_comments();
    let _generation = cursor.parse_u16()?;
    cursor.skip_ws_and_comments();
    cursor.expect_keyword("obj")?;
    cursor.skip_ws_and_comments();

    let value = cursor.parse_value()?;
    cursor.skip_ws_and_comments();
    if matches!(value, PdfValue::Dictionary(_)) && cursor.peek_keyword("stream") {
        let dict = match value {
            PdfValue::Dictionary(dict) => dict,
            _ => unreachable!(),
        };
        cursor.expect_keyword("stream")?;
        cursor.consume_stream_line_break();
        let stream_start = cursor.position;
        // Prefer the Length entry from the stream dictionary to determine the
        // data boundary.  This prevents binary stream data that happens to
        // contain the literal bytes "endstream" from being truncated.
        // Fall back to scanning for `endstream` when Length is absent,
        // an indirect reference (can't resolve yet), or past EOF.
        let length_hint = dict
            .get("Length")
            .and_then(PdfValue::as_integer)
            .filter(|&len| len >= 0)
            .map(|len| len as usize);
        let (data, endstream_pos) = match length_hint {
            Some(len) if stream_start + len <= bytes.len() => {
                // Verify the endstream keyword follows at the expected offset.
                // Tolerate trailing EOL between data and keyword per PDF spec.
                let mut check = stream_start + len;
                while check < bytes.len() && matches!(bytes[check], b'\r' | b'\n') {
                    check += 1;
                }
                if bytes.get(check..check + 9) == Some(b"endstream") {
                    (bytes[stream_start..stream_start + len].to_vec(), check)
                } else {
                    // Length is wrong; fall back to scanning
                    let pos = find_keyword(bytes, stream_start, b"endstream")
                        .ok_or_else(|| PdfError::Parse("stream missing endstream".to_string()))?;
                    (bytes[stream_start..pos].to_vec(), pos)
                }
            }
            _ => {
                let pos = find_keyword(bytes, stream_start, b"endstream")
                    .ok_or_else(|| PdfError::Parse("stream missing endstream".to_string()))?;
                (bytes[stream_start..pos].to_vec(), pos)
            }
        };
        cursor.position = endstream_pos;
        cursor.expect_keyword("endstream")?;
        cursor.skip_ws_and_comments();
        cursor.expect_keyword("endobj")?;
        Ok(PdfObject::Stream(PdfStream { dict, data }))
    } else {
        cursor.expect_keyword("endobj")?;
        Ok(PdfObject::Value(value))
    }
}

fn find_keyword(bytes: &[u8], start: usize, keyword: &[u8]) -> Option<usize> {
    bytes[start..]
        .windows(keyword.len())
        .position(|window| window == keyword)
        .map(|relative| start + relative)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8], position: usize) -> Self {
        Self { bytes, position }
    }

    fn eof(&self) -> bool {
        self.position >= self.bytes.len()
    }

    fn current(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn skip_ws_and_comments(&mut self) {
        while let Some(byte) = self.current() {
            match byte {
                b' ' | b'\t' | b'\n' | b'\r' | 0x0C | 0x00 => self.position += 1,
                b'%' => {
                    while let Some(next) = self.current() {
                        self.position += 1;
                        if next == b'\n' || next == b'\r' {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
    }

    fn skip_line_breaks(&mut self) {
        while matches!(self.current(), Some(b'\n' | b'\r')) {
            self.position += 1;
        }
    }

    fn read_line(&mut self) -> PdfResult<&'a [u8]> {
        if self.eof() {
            return Err(PdfError::Parse("unexpected end of file".to_string()));
        }
        let start = self.position;
        while let Some(byte) = self.current() {
            if byte == b'\n' || byte == b'\r' {
                let end = self.position;
                self.skip_line_breaks();
                return Ok(&self.bytes[start..end]);
            }
            self.position += 1;
        }
        Ok(&self.bytes[start..self.position])
    }

    fn peek_keyword(&self, keyword: &str) -> bool {
        self.bytes
            .get(self.position..self.position + keyword.len())
            .map(|slice| slice == keyword.as_bytes())
            .unwrap_or(false)
    }

    fn expect_keyword(&mut self, keyword: &str) -> PdfResult<()> {
        self.skip_ws_and_comments();
        if self.peek_keyword(keyword) {
            self.position += keyword.len();
            Ok(())
        } else {
            Err(PdfError::Parse(format!("expected keyword {keyword}")))
        }
    }

    fn consume_stream_line_break(&mut self) {
        if self.current() == Some(b'\r') {
            self.position += 1;
        }
        if self.current() == Some(b'\n') {
            self.position += 1;
        }
    }

    fn parse_u32(&mut self) -> PdfResult<u32> {
        let token = self.parse_token()?;
        token
            .parse::<u32>()
            .map_err(|_| PdfError::Parse(format!("invalid integer token: {token}")))
    }

    fn parse_u16(&mut self) -> PdfResult<u16> {
        let token = self.parse_token()?;
        token
            .parse::<u16>()
            .map_err(|_| PdfError::Parse(format!("invalid integer token: {token}")))
    }

    fn parse_usize(&mut self) -> PdfResult<usize> {
        let token = self.parse_token()?;
        token
            .parse::<usize>()
            .map_err(|_| PdfError::Parse(format!("invalid offset token: {token}")))
    }

    fn parse_token(&mut self) -> PdfResult<String> {
        self.skip_ws_and_comments();
        let start = self.position;
        while let Some(byte) = self.current() {
            if is_delimiter(byte) || is_whitespace(byte) {
                break;
            }
            self.position += 1;
        }
        if self.position == start {
            return Err(PdfError::Parse("expected token".to_string()));
        }
        Ok(String::from_utf8_lossy(&self.bytes[start..self.position]).to_string())
    }

    fn parse_value(&mut self) -> PdfResult<PdfValue> {
        self.skip_ws_and_comments();
        match self.current() {
            Some(b'/') => self.parse_name(),
            Some(b'(') => self.parse_literal_string(),
            Some(b'[') => self.parse_array(),
            Some(b'<') if self.bytes.get(self.position + 1) == Some(&b'<') => {
                self.parse_dictionary()
            }
            Some(b'<') => self.parse_hex_string(),
            Some(b't') if self.peek_keyword("true") => {
                self.position += 4;
                Ok(PdfValue::Bool(true))
            }
            Some(b'f') if self.peek_keyword("false") => {
                self.position += 5;
                Ok(PdfValue::Bool(false))
            }
            Some(b'n') if self.peek_keyword("null") => {
                self.position += 4;
                Ok(PdfValue::Null)
            }
            Some(_) => self.parse_number_or_reference(),
            None => Err(PdfError::Parse("unexpected end of file".to_string())),
        }
    }

    fn parse_name(&mut self) -> PdfResult<PdfValue> {
        self.position += 1;
        let mut raw = Vec::new();
        while let Some(byte) = self.current() {
            if is_delimiter(byte) || is_whitespace(byte) {
                break;
            }
            if byte == b'#' {
                let high = self
                    .bytes
                    .get(self.position + 1)
                    .copied()
                    .ok_or_else(|| PdfError::Parse("truncated #XX escape in name".to_string()))?;
                let low = self
                    .bytes
                    .get(self.position + 2)
                    .copied()
                    .ok_or_else(|| PdfError::Parse("truncated #XX escape in name".to_string()))?;
                let decoded = u8::from_str_radix(
                    &format!("{}{}", high as char, low as char),
                    16,
                )
                .map_err(|_| PdfError::Parse("invalid #XX hex escape in name".to_string()))?;
                raw.push(decoded);
                self.position += 3;
            } else {
                raw.push(byte);
                self.position += 1;
            }
        }
        Ok(PdfValue::Name(
            String::from_utf8_lossy(&raw).to_string(),
        ))
    }

    fn parse_literal_string(&mut self) -> PdfResult<PdfValue> {
        self.position += 1;
        let mut output = Vec::new();
        let mut depth = 1usize;
        while let Some(byte) = self.current() {
            self.position += 1;
            match byte {
                b'\\' => {
                    let escaped = self
                        .current()
                        .ok_or_else(|| PdfError::Parse("unterminated string escape".to_string()))?;
                    self.position += 1;
                    match escaped {
                        b'n' => output.push(b'\n'),
                        b'r' => output.push(b'\r'),
                        b't' => output.push(b'\t'),
                        b'b' => output.push(0x08),
                        b'f' => output.push(0x0C),
                        b'(' | b')' | b'\\' => output.push(escaped),
                        b'\n' => {}
                        b'\r' => {
                            if self.current() == Some(b'\n') {
                                self.position += 1;
                            }
                        }
                        b'0'..=b'7' => {
                            let mut octal = vec![escaped];
                            for _ in 0..2 {
                                match self.current() {
                                    Some(next @ b'0'..=b'7') => {
                                        octal.push(next);
                                        self.position += 1;
                                    }
                                    _ => break,
                                }
                            }
                            // PDF spec: octal value is taken modulo 256
                            let value =
                                u16::from_str_radix(std::str::from_utf8(&octal).unwrap_or("0"), 8)
                                    .unwrap_or(0);
                            output.push((value % 256) as u8);
                        }
                        other => output.push(other),
                    }
                }
                b'(' => {
                    depth += 1;
                    output.push(byte);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(PdfValue::String(PdfString(output)));
                    }
                    output.push(byte);
                }
                _ => output.push(byte),
            }
        }
        Err(PdfError::Parse("unterminated literal string".to_string()))
    }

    fn parse_hex_string(&mut self) -> PdfResult<PdfValue> {
        self.position += 1;
        let start = self.position;
        while self.current() != Some(b'>') {
            if self.eof() {
                return Err(PdfError::Parse("unterminated hex string".to_string()));
            }
            self.position += 1;
        }
        let raw = String::from_utf8_lossy(&self.bytes[start..self.position])
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        self.position += 1;
        let mut chars = raw.chars().collect::<Vec<_>>();
        if chars.len() % 2 != 0 {
            chars.push('0');
        }
        let mut bytes = Vec::with_capacity(chars.len() / 2);
        for pair in chars.chunks(2) {
            let value = u8::from_str_radix(&pair.iter().collect::<String>(), 16)
                .map_err(|_| PdfError::Parse("invalid hex string".to_string()))?;
            bytes.push(value);
        }
        Ok(PdfValue::String(PdfString(bytes)))
    }

    fn parse_array(&mut self) -> PdfResult<PdfValue> {
        self.position += 1;
        let mut values = Vec::new();
        loop {
            self.skip_ws_and_comments();
            match self.current() {
                Some(b']') => {
                    self.position += 1;
                    break;
                }
                Some(_) => values.push(self.parse_value()?),
                None => return Err(PdfError::Parse("unterminated array".to_string())),
            }
        }
        Ok(PdfValue::Array(values))
    }

    fn parse_dictionary(&mut self) -> PdfResult<PdfValue> {
        self.position += 2;
        let mut dictionary = PdfDictionary::new();
        loop {
            self.skip_ws_and_comments();
            if self.current() == Some(b'>') && self.bytes.get(self.position + 1) == Some(&b'>') {
                self.position += 2;
                break;
            }
            let key = match self.parse_name()? {
                PdfValue::Name(name) => name,
                _ => unreachable!(),
            };
            let value = self.parse_value()?;
            dictionary.insert(key, value);
        }
        Ok(PdfValue::Dictionary(dictionary))
    }

    fn parse_number_or_reference(&mut self) -> PdfResult<PdfValue> {
        let first_token = self.parse_token()?;
        if first_token.contains('.') || first_token.contains(['e', 'E']) {
            return first_token
                .parse::<f64>()
                .map(PdfValue::Number)
                .map_err(|_| PdfError::Parse(format!("invalid number token: {first_token}")));
        }

        let checkpoint = self.position;
        self.skip_ws_and_comments();
        if let Ok(second_token) = self.parse_token() {
            self.skip_ws_and_comments();
            if self.current() == Some(b'R')
                && second_token
                    .chars()
                    .all(|character| character.is_ascii_digit())
            {
                self.position += 1;
                return Ok(PdfValue::Reference(ObjectRef::new(
                    first_token
                        .parse::<u32>()
                        .map_err(|_| PdfError::Parse("invalid reference object".to_string()))?,
                    second_token
                        .parse::<u16>()
                        .map_err(|_| PdfError::Parse("invalid reference generation".to_string()))?,
                )));
            }
        }
        self.position = checkpoint;
        first_token
            .parse::<i64>()
            .map(PdfValue::Integer)
            .or_else(|_| first_token.parse::<f64>().map(PdfValue::Number))
            .map_err(|_| PdfError::Parse(format!("invalid number token: {first_token}")))
    }
}

fn is_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0C | 0x00)
}

fn is_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

#[cfg(test)]
mod tests {
    use super::parse_pdf;

    #[test]
    fn parses_simple_pdf_fixture() {
        let bytes = include_bytes!("../../../tests/fixtures/simple-text.pdf");
        let document = parse_pdf(bytes).expect("fixture should parse");
        assert_eq!(document.pages.len(), 1);
    }
}
