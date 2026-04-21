use std::collections::{BTreeMap, BTreeSet};

use crate::document::build_document;
use crate::error::{PdfError, PdfResult};
use crate::stream::decode_stream;
use crate::types::{
    ObjectRef, PdfDictionary, PdfFile, PdfObject, PdfStream, PdfString, PdfValue, XrefEntry,
};

pub fn parse_pdf(bytes: &[u8]) -> PdfResult<crate::document::ParsedDocument> {
    let version = parse_header(bytes)?;
    let startxref = find_startxref(bytes)?;
    let (xref, trailer) = parse_xref_table(bytes, startxref)?;

    let mut objects = BTreeMap::new();
    let mut max_object_number = 0;
    let mut compressed: Vec<(ObjectRef, u32, u32)> = Vec::new();

    for (object_ref, entry) in &xref {
        match entry {
            XrefEntry::Free => {}
            XrefEntry::Uncompressed { offset, .. } => {
                if object_ref.object_number == 0 {
                    continue;
                }
                let object = parse_indirect_object(bytes, *offset)?;
                max_object_number = max_object_number.max(object_ref.object_number);
                objects.insert(*object_ref, object);
            }
            XrefEntry::Compressed {
                stream_object_number,
                index,
            } => {
                compressed.push((*object_ref, *stream_object_number, *index));
            }
        }
    }

    materialize_object_streams(&mut objects, &mut max_object_number, &compressed)?;

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
    start_offset: usize,
) -> PdfResult<(BTreeMap<ObjectRef, XrefEntry>, PdfDictionary)> {
    let mut merged_entries: BTreeMap<ObjectRef, XrefEntry> = BTreeMap::new();
    let mut newest_trailer: Option<PdfDictionary> = None;
    let mut visited = BTreeSet::new();
    let mut pending: Vec<usize> = vec![start_offset];

    while let Some(offset) = pending.pop() {
        if !visited.insert(offset) {
            continue;
        }
        let section = parse_xref_section_at(bytes, offset)?;

        // Newest-first: only insert entries not already present
        for (object_ref, entry) in section.entries {
            merged_entries.entry(object_ref).or_insert(entry);
        }

        if newest_trailer.is_none() {
            newest_trailer = Some(section.trailer.clone());
        }

        if let Some(stm_offset) = section
            .trailer
            .get("XRefStm")
            .and_then(PdfValue::as_integer)
        {
            pending.push(stm_offset as usize);
        }
        if let Some(prev_offset) = section.trailer.get("Prev").and_then(PdfValue::as_integer) {
            pending.push(prev_offset as usize);
        }
    }

    let trailer = newest_trailer
        .ok_or_else(|| PdfError::Parse("xref chain produced no trailer".to_string()))?;
    Ok((merged_entries, trailer))
}

struct XrefSection {
    entries: BTreeMap<ObjectRef, XrefEntry>,
    trailer: PdfDictionary,
}

fn parse_xref_section_at(bytes: &[u8], offset: usize) -> PdfResult<XrefSection> {
    let mut probe = Cursor::new(bytes, offset);
    probe.skip_ws_and_comments();
    if probe.peek_keyword("xref") {
        parse_classic_xref_section(bytes, offset)
    } else {
        parse_xref_stream_section(bytes, offset)
    }
}

fn parse_classic_xref_section(bytes: &[u8], offset: usize) -> PdfResult<XrefSection> {
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
            let entry_offset = fields
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
            let entry = if flag == "n" {
                XrefEntry::Uncompressed {
                    offset: entry_offset,
                    generation,
                }
            } else {
                XrefEntry::Free
            };
            entries.insert(ObjectRef::new(object_number, generation), entry);
        }
    }
    cursor.expect_keyword("trailer")?;
    let trailer = match cursor.parse_value()? {
        PdfValue::Dictionary(dictionary) => dictionary,
        _ => return Err(PdfError::Parse("trailer is not a dictionary".to_string())),
    };
    Ok(XrefSection { entries, trailer })
}

fn parse_xref_stream_section(bytes: &[u8], offset: usize) -> PdfResult<XrefSection> {
    let object = parse_indirect_object(bytes, offset)?;
    let stream = match object {
        PdfObject::Stream(stream) => stream,
        PdfObject::Value(_) => {
            return Err(PdfError::Parse(
                "expected xref stream object at startxref offset".to_string(),
            ));
        }
    };
    if stream.dict.get("Type").and_then(PdfValue::as_name) != Some("XRef") {
        return Err(PdfError::Parse(
            "xref stream object has wrong Type".to_string(),
        ));
    }

    let size = stream
        .dict
        .get("Size")
        .and_then(PdfValue::as_integer)
        .ok_or_else(|| PdfError::Corrupt("xref stream missing Size".to_string()))?
        as u32;

    let w = stream
        .dict
        .get("W")
        .and_then(PdfValue::as_array)
        .ok_or_else(|| PdfError::Corrupt("xref stream missing W".to_string()))?;
    if w.len() != 3 {
        return Err(PdfError::Corrupt(
            "xref stream W must have three entries".to_string(),
        ));
    }
    let w0 = w[0]
        .as_integer()
        .ok_or_else(|| PdfError::Corrupt("invalid W[0]".to_string()))? as usize;
    let w1 = w[1]
        .as_integer()
        .ok_or_else(|| PdfError::Corrupt("invalid W[1]".to_string()))? as usize;
    let w2 = w[2]
        .as_integer()
        .ok_or_else(|| PdfError::Corrupt("invalid W[2]".to_string()))? as usize;
    let row_len = w0 + w1 + w2;
    if row_len == 0 {
        return Err(PdfError::Corrupt(
            "xref stream row width is zero".to_string(),
        ));
    }

    let index: Vec<(u32, u32)> = match stream.dict.get("Index") {
        Some(PdfValue::Array(entries)) => {
            if entries.len() % 2 != 0 {
                return Err(PdfError::Corrupt(
                    "xref stream Index must have an even number of entries".to_string(),
                ));
            }
            let mut pairs = Vec::with_capacity(entries.len() / 2);
            for chunk in entries.chunks(2) {
                let first = chunk[0]
                    .as_integer()
                    .ok_or_else(|| PdfError::Corrupt("invalid Index entry".to_string()))?
                    as u32;
                let count = chunk[1]
                    .as_integer()
                    .ok_or_else(|| PdfError::Corrupt("invalid Index entry".to_string()))?
                    as u32;
                pairs.push((first, count));
            }
            pairs
        }
        Some(_) => {
            return Err(PdfError::Corrupt(
                "xref stream Index is not an array".to_string(),
            ));
        }
        None => vec![(0, size)],
    };

    let decoded = decode_stream(&stream)?;
    let expected_rows: u32 = index.iter().map(|(_, count)| *count).sum();
    if decoded.len() < expected_rows as usize * row_len {
        return Err(PdfError::Corrupt(
            "xref stream body is shorter than declared entries".to_string(),
        ));
    }

    let mut entries: BTreeMap<ObjectRef, XrefEntry> = BTreeMap::new();
    let mut cursor = 0usize;
    for (first, count) in index {
        for i in 0..count {
            let row = &decoded[cursor..cursor + row_len];
            cursor += row_len;
            let field_type = if w0 == 0 { 1u64 } else { read_be(&row[..w0])? };
            let f2 = read_be(&row[w0..w0 + w1])?;
            let f3 = read_be(&row[w0 + w1..])?;
            let object_number = first + i;
            let entry = match field_type {
                0 => XrefEntry::Free,
                1 => XrefEntry::Uncompressed {
                    offset: f2 as usize,
                    generation: f3 as u16,
                },
                2 => XrefEntry::Compressed {
                    stream_object_number: f2 as u32,
                    index: f3 as u32,
                },
                other => {
                    return Err(PdfError::Unsupported(format!(
                        "xref stream entry type {other} is not supported"
                    )));
                }
            };
            let generation = match entry {
                XrefEntry::Uncompressed { generation, .. } => generation,
                _ => 0,
            };
            entries.insert(ObjectRef::new(object_number, generation), entry);
        }
    }

    Ok(XrefSection {
        entries,
        trailer: stream.dict,
    })
}

fn read_be(bytes: &[u8]) -> PdfResult<u64> {
    if bytes.len() > 8 {
        return Err(PdfError::Corrupt(
            "xref stream field width exceeds 8 bytes".to_string(),
        ));
    }
    let mut value: u64 = 0;
    for byte in bytes {
        value = (value << 8) | *byte as u64;
    }
    Ok(value)
}

fn materialize_object_streams(
    objects: &mut BTreeMap<ObjectRef, PdfObject>,
    max_object_number: &mut u32,
    compressed: &[(ObjectRef, u32, u32)],
) -> PdfResult<()> {
    if compressed.is_empty() {
        return Ok(());
    }

    let mut by_stream: BTreeMap<u32, Vec<(ObjectRef, u32)>> = BTreeMap::new();
    for (object_ref, stream_obj_num, index) in compressed {
        by_stream
            .entry(*stream_obj_num)
            .or_default()
            .push((*object_ref, *index));
    }

    for (stream_obj_num, mut members) in by_stream {
        let stream_ref = ObjectRef::new(stream_obj_num, 0);
        let stream = match objects.get(&stream_ref) {
            Some(PdfObject::Stream(stream)) => stream.clone(),
            Some(PdfObject::Value(_)) => {
                return Err(PdfError::Corrupt(format!(
                    "object stream {stream_obj_num} is not a stream"
                )));
            }
            None => {
                return Err(PdfError::Corrupt(format!(
                    "compressed entry references missing object stream {stream_obj_num}"
                )));
            }
        };
        if stream.dict.get("Type").and_then(PdfValue::as_name) != Some("ObjStm") {
            return Err(PdfError::Corrupt(format!(
                "object {stream_obj_num} is not marked as ObjStm"
            )));
        }
        let n = stream
            .dict
            .get("N")
            .and_then(PdfValue::as_integer)
            .ok_or_else(|| PdfError::Corrupt("ObjStm missing N".to_string()))?
            as usize;
        let first = stream
            .dict
            .get("First")
            .and_then(PdfValue::as_integer)
            .ok_or_else(|| PdfError::Corrupt("ObjStm missing First".to_string()))?
            as usize;

        let decoded = decode_stream(&stream)?;
        if first > decoded.len() {
            return Err(PdfError::Corrupt(
                "ObjStm First offset is past end of decoded data".to_string(),
            ));
        }

        let header = &decoded[..first];
        let mut header_cursor = Cursor::new(header, 0);
        let mut entries: Vec<(u32, usize)> = Vec::with_capacity(n);
        for _ in 0..n {
            header_cursor.skip_ws_and_comments();
            let obj_num = header_cursor.parse_u32()?;
            header_cursor.skip_ws_and_comments();
            let rel_offset = header_cursor.parse_usize()?;
            entries.push((obj_num, rel_offset));
        }

        // Guard: a compressed entry's index must be in range.
        members.sort_by_key(|(_, index)| *index);
        for (member_ref, index) in members {
            let idx = index as usize;
            if idx >= entries.len() {
                return Err(PdfError::Corrupt(format!(
                    "ObjStm {stream_obj_num} has no index {idx}"
                )));
            }
            let (declared_number, rel_offset) = entries[idx];
            if declared_number != member_ref.object_number {
                return Err(PdfError::Corrupt(format!(
                    "ObjStm {stream_obj_num} index {idx} has number {declared_number} but xref expected {}",
                    member_ref.object_number
                )));
            }
            let absolute_offset = first
                .checked_add(rel_offset)
                .ok_or_else(|| PdfError::Corrupt("ObjStm offset overflow".to_string()))?;
            if absolute_offset > decoded.len() {
                return Err(PdfError::Corrupt(
                    "ObjStm member offset is past end of decoded data".to_string(),
                ));
            }
            let mut value_cursor = Cursor::new(&decoded, absolute_offset);
            let value = value_cursor.parse_value()?;
            if let PdfValue::Dictionary(dict) = &value {
                if dict.get("Type").and_then(PdfValue::as_name) == Some("ObjStm") {
                    return Err(PdfError::Unsupported(
                        "nested object streams are not supported".to_string(),
                    ));
                }
            }
            *max_object_number = (*max_object_number).max(member_ref.object_number);
            objects.insert(member_ref, PdfObject::Value(value));
        }
    }

    Ok(())
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
                let high =
                    self.bytes.get(self.position + 1).copied().ok_or_else(|| {
                        PdfError::Parse("truncated #XX escape in name".to_string())
                    })?;
                let low =
                    self.bytes.get(self.position + 2).copied().ok_or_else(|| {
                        PdfError::Parse("truncated #XX escape in name".to_string())
                    })?;
                let decoded = u8::from_str_radix(&format!("{}{}", high as char, low as char), 16)
                    .map_err(|_| {
                    PdfError::Parse("invalid #XX hex escape in name".to_string())
                })?;
                raw.push(decoded);
                self.position += 3;
            } else {
                raw.push(byte);
                self.position += 1;
            }
        }
        Ok(PdfValue::Name(String::from_utf8_lossy(&raw).to_string()))
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
    use crate::error::PdfError;
    use crate::types::PdfObject;

    #[test]
    fn parses_simple_pdf_fixture() {
        let bytes = include_bytes!("../../../tests/fixtures/simple-text.pdf");
        let document = parse_pdf(bytes).expect("fixture should parse");
        assert_eq!(document.pages.len(), 1);
    }

    #[test]
    fn parses_incremental_update_fixture() {
        let bytes = include_bytes!("../../../tests/fixtures/incremental-update.pdf");
        let document = parse_pdf(bytes).expect("incremental fixture should parse");
        assert_eq!(document.pages.len(), 1);

        // The updated content stream (object 4) should contain "Updated Secret",
        // not "Original Secret"
        let content_refs = &document.pages[0].content_refs;
        assert!(!content_refs.is_empty());
        let content_obj = document.file.objects.get(&content_refs[0]).unwrap();
        let stream_data = match content_obj {
            PdfObject::Stream(stream) => String::from_utf8_lossy(&stream.data),
            _ => panic!("expected stream object for page content"),
        };
        assert!(
            stream_data.contains("Updated Secret"),
            "content stream should contain updated text"
        );
        assert!(
            !stream_data.contains("Original Secret"),
            "content stream should not contain original text"
        );
    }

    #[test]
    fn circular_prev_chain_does_not_loop() {
        // Build a minimal PDF where Prev points back to the same xref offset.
        // The parser should de-duplicate the offset via its visited-set and
        // parse the tree successfully instead of returning an error.
        let mut pdf = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.4\n");

        // Object 1: catalog
        let obj1_offset = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        // Object 2: pages
        let obj2_offset = pdf.len();
        pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Count 0 /Kids [] >>\nendobj\n");

        let xref_offset = pdf.len();
        pdf.extend_from_slice(b"xref\n0 3\n");
        pdf.extend_from_slice(b"0000000000 65535 f \n");
        pdf.extend_from_slice(format!("{:010} 00000 n \n", obj1_offset).as_bytes());
        pdf.extend_from_slice(format!("{:010} 00000 n \n", obj2_offset).as_bytes());
        pdf.extend_from_slice(b"trailer\n");
        // Prev points back to this same xref offset — circular
        pdf.extend_from_slice(
            format!("<< /Size 3 /Root 1 0 R /Prev {} >>\n", xref_offset).as_bytes(),
        );
        pdf.extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref_offset).as_bytes());

        let document = parse_pdf(&pdf).expect("circular Prev should be tolerated");
        assert_eq!(document.pages.len(), 0);
    }

    #[test]
    fn parses_uncompressed_xref_stream() {
        // Minimal PDF using an xref stream with no filters and no predictor.
        // W = [1 2 1] means type(1) + offset(2) + generation(1).
        let mut pdf: Vec<u8> = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.5\n");

        let obj1_offset = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let obj2_offset = pdf.len();
        pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Count 0 /Kids [] >>\nendobj\n");

        // Build the xref stream body: four 4-byte rows for objects 0..3.
        // Row layout: type(1) | offset(2) | generation(1).
        let row_for = |t: u8, off: u16, generation: u8| {
            let mut row = [0u8; 4];
            row[0] = t;
            row[1] = (off >> 8) as u8;
            row[2] = off as u8;
            row[3] = generation;
            row
        };
        let mut body = Vec::new();
        body.extend_from_slice(&row_for(0, 0, 0xFF)); // object 0 free
        body.extend_from_slice(&row_for(1, obj1_offset as u16, 0));
        body.extend_from_slice(&row_for(1, obj2_offset as u16, 0));
        body.extend_from_slice(&row_for(1, 0, 0)); // self (object 3), placeholder; we will overwrite after knowing offset

        let xref_obj_offset = pdf.len();
        // Overwrite object 3 self-offset in body now that we know it.
        let self_offset = xref_obj_offset as u16;
        body[12] = 1;
        body[13] = (self_offset >> 8) as u8;
        body[14] = self_offset as u8;
        body[15] = 0;

        let stream_dict = format!(
            "<< /Type /XRef /Size 4 /W [1 2 1] /Root 1 0 R /Length {} >>",
            body.len()
        );
        pdf.extend_from_slice(format!("3 0 obj\n{stream_dict}\nstream\n").as_bytes());
        pdf.extend_from_slice(&body);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");
        pdf.extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref_obj_offset).as_bytes());

        let document = parse_pdf(&pdf).expect("xref stream fixture should parse");
        assert_eq!(document.pages.len(), 0);
        // Object 1 and 2 must be materialized.
        assert!(document.file.objects.len() >= 2);
    }

    #[test]
    fn parses_object_stream_via_xref_stream() {
        use flate2::{Compression, write::ZlibEncoder};
        use std::io::Write;

        // Pages tree is compressed inside an ObjStm.
        // Layout:
        //   1: Catalog (uncompressed)
        //   2: Pages (compressed in ObjStm 3, index 0)
        //   3: ObjStm (uncompressed, flate-compressed body)
        //   4: xref stream (uncompressed)
        let mut pdf: Vec<u8> = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.5\n");

        let obj1_offset = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        // Object 3 is an ObjStm holding object 2.
        let member_payload = b"<< /Type /Pages /Count 0 /Kids [] >>";
        let header = b"2 0 ";
        let first = header.len();
        let mut decompressed = Vec::new();
        decompressed.extend_from_slice(header);
        decompressed.extend_from_slice(member_payload);

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&decompressed).unwrap();
        let compressed = encoder.finish().unwrap();

        let obj3_offset = pdf.len();
        let objstm_dict = format!(
            "<< /Type /ObjStm /N 1 /First {} /Filter /FlateDecode /Length {} >>",
            first,
            compressed.len()
        );
        pdf.extend_from_slice(format!("3 0 obj\n{objstm_dict}\nstream\n").as_bytes());
        pdf.extend_from_slice(&compressed);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");

        // Build xref stream entries for objects 0..5:
        // 0 free, 1 uncompressed, 2 compressed (stream=3, index=0),
        // 3 uncompressed (ObjStm), 4 uncompressed (xref stream itself).
        let row_for = |t: u8, a: u32, b: u16| {
            let mut row = [0u8; 5];
            row[0] = t;
            row[1] = (a >> 16) as u8;
            row[2] = (a >> 8) as u8;
            row[3] = a as u8;
            row[4] = b as u8;
            row
        };

        let obj4_offset = pdf.len();
        let mut body = Vec::new();
        body.extend_from_slice(&row_for(0, 0, 0xFF));
        body.extend_from_slice(&row_for(1, obj1_offset as u32, 0));
        body.extend_from_slice(&row_for(2, 3, 0));
        body.extend_from_slice(&row_for(1, obj3_offset as u32, 0));
        body.extend_from_slice(&row_for(1, obj4_offset as u32, 0));

        let stream_dict = format!(
            "<< /Type /XRef /Size 5 /W [1 3 1] /Root 1 0 R /Length {} >>",
            body.len()
        );
        pdf.extend_from_slice(format!("4 0 obj\n{stream_dict}\nstream\n").as_bytes());
        pdf.extend_from_slice(&body);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");
        pdf.extend_from_slice(format!("startxref\n{}\n%%EOF\n", obj4_offset).as_bytes());

        let document = parse_pdf(&pdf).expect("ObjStm fixture should parse");
        assert_eq!(document.pages.len(), 0);
        // Pages dictionary should be materialized.
        let pages_ref = document.catalog.pages_ref;
        let pages_dict = document.file.get_dictionary(pages_ref).unwrap();
        assert_eq!(pages_dict.get("Type").and_then(|v| v.as_name()), Some("Pages"));
    }

    #[test]
    fn rejects_nested_object_stream() {
        use flate2::{Compression, write::ZlibEncoder};
        use std::io::Write;

        // A compressed member is itself an ObjStm dictionary → must fail.
        let mut pdf: Vec<u8> = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.5\n");

        let obj1_offset = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        let member_payload = b"<< /Type /ObjStm /N 0 /First 0 /Length 0 >>";
        let header = b"2 0 ";
        let first = header.len();
        let mut decompressed = Vec::new();
        decompressed.extend_from_slice(header);
        decompressed.extend_from_slice(member_payload);

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&decompressed).unwrap();
        let compressed = encoder.finish().unwrap();

        let obj3_offset = pdf.len();
        let objstm_dict = format!(
            "<< /Type /ObjStm /N 1 /First {} /Filter /FlateDecode /Length {} >>",
            first,
            compressed.len()
        );
        pdf.extend_from_slice(format!("3 0 obj\n{objstm_dict}\nstream\n").as_bytes());
        pdf.extend_from_slice(&compressed);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");

        let row_for = |t: u8, a: u32, b: u16| {
            let mut row = [0u8; 5];
            row[0] = t;
            row[1] = (a >> 16) as u8;
            row[2] = (a >> 8) as u8;
            row[3] = a as u8;
            row[4] = b as u8;
            row
        };

        let obj4_offset = pdf.len();
        let mut body = Vec::new();
        body.extend_from_slice(&row_for(0, 0, 0xFF));
        body.extend_from_slice(&row_for(1, obj1_offset as u32, 0));
        body.extend_from_slice(&row_for(2, 3, 0));
        body.extend_from_slice(&row_for(1, obj3_offset as u32, 0));
        body.extend_from_slice(&row_for(1, obj4_offset as u32, 0));

        let stream_dict = format!(
            "<< /Type /XRef /Size 5 /W [1 3 1] /Root 1 0 R /Length {} >>",
            body.len()
        );
        pdf.extend_from_slice(format!("4 0 obj\n{stream_dict}\nstream\n").as_bytes());
        pdf.extend_from_slice(&body);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");
        pdf.extend_from_slice(format!("startxref\n{}\n%%EOF\n", obj4_offset).as_bytes());

        match parse_pdf(&pdf) {
            Err(PdfError::Unsupported(message)) => assert!(
                message.contains("nested object streams"),
                "got: {message}"
            ),
            other => panic!("expected Unsupported, got: {other:?}"),
        }
    }
}
