use pdf_graphics::{Matrix, Point, Rect};
use pdf_objects::{
    PageInfo, PdfError, PdfFile, PdfResult, PdfString, PdfValue, decode_stream,
    document::get_stream,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Operation {
    pub operator: String,
    pub operands: Vec<PdfValue>,
}

#[derive(Debug, Clone)]
pub struct ContentStream {
    pub decoded: Vec<u8>,
    pub operations: Vec<Operation>,
}

#[derive(Debug, Clone)]
pub struct ParsedPageContent {
    pub bytes: Vec<u8>,
    pub operations: Vec<Operation>,
}

#[derive(Debug, Clone)]
pub struct OperandString {
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaintOperator {
    Stroke,
    Fill,
    FillEvenOdd,
    StrokeFill,
    StrokeFillEvenOdd,
    CloseStroke,
    CloseFill,
    CloseFillEvenOdd,
    NoPaint,
}

impl PaintOperator {
    pub fn from_operator(operator: &str) -> Option<Self> {
        match operator {
            "S" => Some(Self::Stroke),
            "s" => Some(Self::CloseStroke),
            "f" | "F" => Some(Self::Fill),
            "f*" => Some(Self::FillEvenOdd),
            "B" => Some(Self::StrokeFill),
            "B*" => Some(Self::StrokeFillEvenOdd),
            "b" => Some(Self::CloseFill),
            "b*" => Some(Self::CloseFillEvenOdd),
            "n" => Some(Self::NoPaint),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum PathSegment {
    MoveTo(Point),
    LineTo(Point),
    CurveTo(Point, Point, Point),
    Rect(Rect),
    ClosePath,
}

#[derive(Debug, Clone, Copy)]
pub struct TextState {
    pub text_matrix: Matrix,
    pub line_matrix: Matrix,
    pub font_size: f64,
    pub character_spacing: f64,
    pub word_spacing: f64,
    pub text_rise: f64,
    pub horizontal_scaling: f64,
    pub leading: f64,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            text_matrix: Matrix::identity(),
            line_matrix: Matrix::identity(),
            font_size: 12.0,
            character_spacing: 0.0,
            word_spacing: 0.0,
            text_rise: 0.0,
            horizontal_scaling: 100.0,
            leading: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GraphicsState {
    pub ctm: Matrix,
    pub stroke_width: f64,
    pub text_state: TextState,
}

impl Default for GraphicsState {
    fn default() -> Self {
        Self {
            ctm: Matrix::identity(),
            stroke_width: 1.0,
            text_state: TextState::default(),
        }
    }
}

pub fn parse_page_contents(file: &PdfFile, page: &PageInfo) -> PdfResult<ParsedPageContent> {
    let mut bytes = Vec::new();
    for content_ref in &page.content_refs {
        let stream = get_stream(file, *content_ref)?;
        let decoded = decode_stream(stream)?;
        bytes.extend_from_slice(&decoded);
        if !decoded.ends_with(b"\n") {
            bytes.push(b'\n');
        }
    }
    let stream = parse_content_stream(&bytes)?;
    Ok(ParsedPageContent {
        bytes,
        operations: stream.operations,
    })
}

pub fn parse_content_stream(bytes: &[u8]) -> PdfResult<ContentStream> {
    let mut parser = ContentParser::new(bytes);
    let mut operations = Vec::new();
    let mut operands = Vec::new();
    while !parser.eof() {
        parser.skip_ws_and_comments();
        if parser.eof() {
            break;
        }
        if let Some(value) = parser.try_parse_operand()? {
            operands.push(value);
            continue;
        }
        let operator = parser.parse_operator()?;
        operations.push(Operation {
            operator,
            operands: std::mem::take(&mut operands),
        });
    }
    if !operands.is_empty() {
        return Err(PdfError::Parse(
            "content stream ended with dangling operands".to_string(),
        ));
    }
    Ok(ContentStream {
        decoded: bytes.to_vec(),
        operations,
    })
}

struct ContentParser<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> ContentParser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
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
                b' ' | b'\t' | b'\r' | b'\n' | 0x0C | 0x00 => self.position += 1,
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

    fn try_parse_operand(&mut self) -> PdfResult<Option<PdfValue>> {
        self.skip_ws_and_comments();
        match self.current() {
            Some(b'/') => self.parse_name().map(Some),
            Some(b'(') => self.parse_literal_string().map(Some),
            Some(b'[') => self.parse_array().map(Some),
            Some(b'<') if self.bytes.get(self.position + 1) != Some(&b'<') => {
                self.parse_hex_string().map(Some)
            }
            Some(b't') if self.peek_keyword("true") => {
                self.position += 4;
                Ok(Some(PdfValue::Bool(true)))
            }
            Some(b'f') if self.peek_keyword("false") => {
                self.position += 5;
                Ok(Some(PdfValue::Bool(false)))
            }
            Some(b'n') if self.peek_keyword("null") => {
                self.position += 4;
                Ok(Some(PdfValue::Null))
            }
            Some(byte) if byte == b'+' || byte == b'-' || byte.is_ascii_digit() || byte == b'.' => {
                self.parse_number().map(Some)
            }
            _ => Ok(None),
        }
    }

    fn parse_operator(&mut self) -> PdfResult<String> {
        self.skip_ws_and_comments();
        let start = self.position;
        while let Some(byte) = self.current() {
            if is_whitespace(byte) || is_delimiter(byte) {
                break;
            }
            self.position += 1;
        }
        if self.position == start {
            return Err(PdfError::Parse("expected operator token".to_string()));
        }
        Ok(String::from_utf8_lossy(&self.bytes[start..self.position]).to_string())
    }

    fn parse_name(&mut self) -> PdfResult<PdfValue> {
        self.position += 1;
        let start = self.position;
        while let Some(byte) = self.current() {
            if is_whitespace(byte) || is_delimiter(byte) {
                break;
            }
            self.position += 1;
        }
        Ok(PdfValue::Name(
            String::from_utf8_lossy(&self.bytes[start..self.position]).to_string(),
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
        let mut bytes = Vec::new();
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
                Some(_) => values.push(
                    self.try_parse_operand()?
                        .ok_or_else(|| PdfError::Parse("unsupported array value".to_string()))?,
                ),
                None => return Err(PdfError::Parse("unterminated array".to_string())),
            }
        }
        Ok(PdfValue::Array(values))
    }

    fn parse_number(&mut self) -> PdfResult<PdfValue> {
        let start = self.position;
        if matches!(self.current(), Some(b'+' | b'-')) {
            self.position += 1;
        }
        while let Some(byte) = self.current() {
            if !(byte.is_ascii_digit() || byte == b'.') {
                break;
            }
            self.position += 1;
        }
        let token = String::from_utf8_lossy(&self.bytes[start..self.position]).to_string();
        if token.contains('.') {
            token
                .parse::<f64>()
                .map(PdfValue::Number)
                .map_err(|_| PdfError::Parse(format!("invalid content number: {token}")))
        } else {
            token
                .parse::<i64>()
                .map(PdfValue::Integer)
                .map_err(|_| PdfError::Parse(format!("invalid content integer: {token}")))
        }
    }

    fn peek_keyword(&self, keyword: &str) -> bool {
        self.bytes
            .get(self.position..self.position + keyword.len())
            .map(|slice| slice == keyword.as_bytes())
            .unwrap_or(false)
    }
}

fn is_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | 0x0C | 0x00)
}

fn is_delimiter(byte: u8) -> bool {
    matches!(byte, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'/' | b'%')
}
