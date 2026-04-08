use std::collections::BTreeMap;

use pdf_content::{Operation, ParsedPageContent, parse_page_contents};
use pdf_graphics::{Matrix, Quad, Rect};
use pdf_objects::{PageInfo, PdfError, PdfFile, PdfResult, PdfValue};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextItem {
    pub text: String,
    pub bbox: Rect,
    pub quad: Option<Quad>,
    pub char_start: Option<usize>,
    pub char_end: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextMatch {
    pub text: String,
    pub page_index: usize,
    pub quads: Vec<Quad>,
}

#[derive(Debug, Clone)]
pub struct GlyphRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone)]
pub enum GlyphLocation {
    Direct {
        operand_index: usize,
        byte_index: usize,
    },
    Array {
        operand_index: usize,
        element_index: usize,
        byte_index: usize,
    },
}

#[derive(Debug, Clone)]
pub struct Glyph {
    pub text: char,
    pub bbox: Rect,
    pub quad: Quad,
    pub page_char_index: usize,
    pub operation_index: usize,
    pub location: GlyphLocation,
}

#[derive(Debug, Clone)]
pub struct ExtractedPageText {
    pub page_index: usize,
    pub text: String,
    pub items: Vec<TextItem>,
    pub glyphs: Vec<Glyph>,
}

#[derive(Debug, Clone)]
pub struct PageSearchIndex {
    pub page: ExtractedPageText,
    normalized_text: String,
    normalized_to_raw: Vec<usize>,
}

pub fn analyze_page_text(
    file: &PdfFile,
    page_index: usize,
    page: &PageInfo,
) -> PdfResult<ExtractedPageText> {
    let parsed = parse_page_contents(file, page)?;
    let fonts = load_fonts(file, &page.resources)?;
    interpret_page_text(page_index, page, &parsed, &fonts)
}

pub fn search_page_text(page: &ExtractedPageText, query: &str) -> Vec<TextMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    let index = build_search_index(page);
    let normalized_query = normalize_search_text(query);
    if normalized_query.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    let mut search_offset = 0usize;
    while let Some(position) = index.normalized_text[search_offset..].find(&normalized_query) {
        let normalized_start = search_offset + position;
        let normalized_end = normalized_start + normalized_query.len();
        let raw_start = *index.normalized_to_raw.get(normalized_start).unwrap_or(&0);
        let raw_end = index
            .normalized_to_raw
            .get(normalized_end.saturating_sub(1))
            .copied()
            .unwrap_or(raw_start)
            + 1;
        let quads = page
            .glyphs
            .iter()
            .filter(|glyph| glyph.page_char_index >= raw_start && glyph.page_char_index < raw_end)
            .map(|glyph| glyph.quad)
            .collect::<Vec<_>>();
        if !quads.is_empty() {
            matches.push(TextMatch {
                text: page
                    .text
                    .chars()
                    .skip(raw_start)
                    .take(raw_end.saturating_sub(raw_start))
                    .collect(),
                page_index: page.page_index,
                quads,
            });
        }
        search_offset = normalized_end;
        if search_offset >= index.normalized_text.len() {
            break;
        }
    }
    matches
}

fn build_search_index(page: &ExtractedPageText) -> PageSearchIndex {
    let mut normalized_text = String::new();
    let mut normalized_to_raw = Vec::new();
    let mut previous_was_whitespace = false;
    for (raw_index, character) in page.text.chars().enumerate() {
        if character.is_whitespace() {
            if !previous_was_whitespace {
                normalized_text.push(' ');
                normalized_to_raw.push(raw_index);
                previous_was_whitespace = true;
            }
        } else {
            normalized_text.push(character);
            normalized_to_raw.push(raw_index);
            previous_was_whitespace = false;
        }
    }
    PageSearchIndex {
        page: page.clone(),
        normalized_text,
        normalized_to_raw,
    }
}

fn normalize_search_text(input: &str) -> String {
    let mut output = String::new();
    let mut previous_was_whitespace = false;
    for character in input.chars() {
        if character.is_whitespace() {
            if !previous_was_whitespace {
                output.push(' ');
                previous_was_whitespace = true;
            }
        } else {
            output.push(character);
            previous_was_whitespace = false;
        }
    }
    output.trim().to_string()
}

fn interpret_page_text(
    page_index: usize,
    page: &PageInfo,
    parsed: &ParsedPageContent,
    fonts: &BTreeMap<String, SimpleFont>,
) -> PdfResult<ExtractedPageText> {
    let page_transform = page.page_box.normalized_transform();
    let mut context = TextContext::new(page_index);
    let mut ctm = Matrix::identity();
    let mut ctm_stack = Vec::new();
    let mut text_state = RuntimeTextState::default();

    for (operation_index, operation) in parsed.operations.iter().enumerate() {
        match operation.operator.as_str() {
            "q" => ctm_stack.push(ctm),
            "Q" => ctm = ctm_stack.pop().unwrap_or(Matrix::identity()),
            "cm" => {
                let matrix = matrix_from_operands(&operation.operands)?;
                ctm = ctm.multiply(matrix);
            }
            "BT" => {
                text_state = RuntimeTextState::default();
            }
            "ET" => {}
            "Tf" => {
                let resource_name = operand_name(operation, 0)?;
                text_state.font = Some(resource_name.to_string());
                text_state.font_size = operand_number(operation, 1)?;
            }
            "Tm" => {
                text_state.text_matrix = matrix_from_operands(&operation.operands)?;
                text_state.line_matrix = text_state.text_matrix;
            }
            "Td" => {
                let tx = operand_number(operation, 0)?;
                let ty = operand_number(operation, 1)?;
                text_state.line_matrix = text_state.line_matrix.multiply(Matrix::translate(tx, ty));
                text_state.text_matrix = text_state.line_matrix;
                if ty.abs() > f64::EPSILON {
                    context.pending_line_break = true;
                }
            }
            "TD" => {
                let tx = operand_number(operation, 0)?;
                let ty = operand_number(operation, 1)?;
                text_state.leading = -ty;
                text_state.line_matrix = text_state.line_matrix.multiply(Matrix::translate(tx, ty));
                text_state.text_matrix = text_state.line_matrix;
                context.pending_line_break = true;
            }
            "T*" => {
                text_state.line_matrix = text_state
                    .line_matrix
                    .multiply(Matrix::translate(0.0, -text_state.leading));
                text_state.text_matrix = text_state.line_matrix;
                context.pending_line_break = true;
            }
            "Tc" => text_state.character_spacing = operand_number(operation, 0)?,
            "Tw" => text_state.word_spacing = operand_number(operation, 0)?,
            "TL" => text_state.leading = operand_number(operation, 0)?,
            "Ts" => text_state.text_rise = operand_number(operation, 0)?,
            "Tz" => text_state.horizontal_scaling = operand_number(operation, 0)?,
            "Tj" => {
                let string = operand_string(operation, 0)?;
                show_text(
                    &mut context,
                    operation_index,
                    ShowOperand::Direct { operand_index: 0 },
                    string,
                    &mut text_state,
                    fonts,
                    ctm,
                    page_transform,
                )?;
            }
            "'" => {
                text_state.line_matrix = text_state
                    .line_matrix
                    .multiply(Matrix::translate(0.0, -text_state.leading));
                text_state.text_matrix = text_state.line_matrix;
                context.pending_line_break = true;
                let string = operand_string(operation, 0)?;
                show_text(
                    &mut context,
                    operation_index,
                    ShowOperand::Direct { operand_index: 0 },
                    string,
                    &mut text_state,
                    fonts,
                    ctm,
                    page_transform,
                )?;
            }
            "\"" => {
                text_state.word_spacing = operand_number(operation, 0)?;
                text_state.character_spacing = operand_number(operation, 1)?;
                text_state.line_matrix = text_state
                    .line_matrix
                    .multiply(Matrix::translate(0.0, -text_state.leading));
                text_state.text_matrix = text_state.line_matrix;
                context.pending_line_break = true;
                let string = operand_string(operation, 2)?;
                show_text(
                    &mut context,
                    operation_index,
                    ShowOperand::Direct { operand_index: 2 },
                    string,
                    &mut text_state,
                    fonts,
                    ctm,
                    page_transform,
                )?;
            }
            "TJ" => {
                let segments = operation
                    .operands
                    .first()
                    .and_then(PdfValue::as_array)
                    .ok_or_else(|| PdfError::Corrupt("TJ expects an array operand".to_string()))?;
                for (element_index, segment) in segments.iter().enumerate() {
                    match segment {
                        PdfValue::String(string) => show_text(
                            &mut context,
                            operation_index,
                            ShowOperand::Array {
                                operand_index: 0,
                                element_index,
                            },
                            string,
                            &mut text_state,
                            fonts,
                            ctm,
                            page_transform,
                        )?,
                        value => {
                            let adjustment = value.as_number().ok_or_else(|| {
                                PdfError::Corrupt("TJ array contains unsupported value".to_string())
                            })?;
                            let scaled = -(adjustment / 1000.0)
                                * text_state.font_size
                                * (text_state.horizontal_scaling / 100.0);
                            text_state.text_matrix = text_state
                                .text_matrix
                                .multiply(Matrix::translate(scaled, 0.0));
                        }
                    }
                }
            }
            "Do" => {
                let _ = operand_name(operation, 0)?;
            }
            _ => {}
        }
    }

    Ok(ExtractedPageText {
        page_index,
        text: context.text,
        items: context.items,
        glyphs: context.glyphs,
    })
}

#[derive(Debug, Clone, Copy)]
enum ShowOperand {
    Direct {
        operand_index: usize,
    },
    Array {
        operand_index: usize,
        element_index: usize,
    },
}

#[derive(Debug, Clone)]
struct SimpleFont {
    widths: Vec<f64>,
    first_char: u16,
}

#[derive(Debug, Clone)]
struct RuntimeTextState {
    text_matrix: Matrix,
    line_matrix: Matrix,
    font_size: f64,
    character_spacing: f64,
    word_spacing: f64,
    text_rise: f64,
    horizontal_scaling: f64,
    leading: f64,
    font: Option<String>,
}

impl Default for RuntimeTextState {
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
            font: None,
        }
    }
}

struct TextContext {
    text: String,
    items: Vec<TextItem>,
    glyphs: Vec<Glyph>,
    pending_line_break: bool,
}

impl TextContext {
    fn new(page_index: usize) -> Self {
        let _ = page_index;
        Self {
            text: String::new(),
            items: Vec::new(),
            glyphs: Vec::new(),
            pending_line_break: false,
        }
    }
}

fn load_fonts(
    file: &PdfFile,
    resources: &pdf_objects::PdfDictionary,
) -> PdfResult<BTreeMap<String, SimpleFont>> {
    let mut fonts = BTreeMap::new();
    let Some(fonts_value) = resources.get("Font") else {
        return Ok(fonts);
    };
    let fonts_dict = file.resolve_dict(fonts_value)?;
    for (name, font_value) in fonts_dict {
        let font_dict = file.resolve_dict(font_value)?;
        let subtype = font_dict
            .get("Subtype")
            .and_then(PdfValue::as_name)
            .unwrap_or("");
        if !matches!(subtype, "Type1" | "TrueType") {
            return Err(PdfError::Unsupported(format!(
                "font subtype {subtype} is not supported"
            )));
        }
        let first_char = font_dict
            .get("FirstChar")
            .and_then(PdfValue::as_integer)
            .unwrap_or(0) as u16;
        let widths = font_dict
            .get("Widths")
            .and_then(PdfValue::as_array)
            .map(|widths| {
                widths
                    .iter()
                    .map(|value| value.as_number().unwrap_or(600.0))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        fonts.insert(name.clone(), SimpleFont { widths, first_char });
    }
    Ok(fonts)
}

#[allow(clippy::too_many_arguments)]
fn show_text(
    context: &mut TextContext,
    operation_index: usize,
    show_operand: ShowOperand,
    string: &pdf_objects::PdfString,
    text_state: &mut RuntimeTextState,
    fonts: &BTreeMap<String, SimpleFont>,
    ctm: Matrix,
    page_transform: Matrix,
) -> PdfResult<()> {
    if context.pending_line_break && !context.text.is_empty() {
        context.text.push('\n');
        context.pending_line_break = false;
    }

    let font_name = text_state.font.clone().ok_or_else(|| {
        PdfError::Unsupported("text-showing operator used without selected font".to_string())
    })?;
    let font = fonts.get(&font_name).ok_or_else(|| {
        PdfError::Unsupported(format!("font resource /{font_name} could not be resolved"))
    })?;
    let scaling = text_state.horizontal_scaling / 100.0;
    let text_to_page = text_state
        .text_matrix
        .multiply(ctm)
        .multiply(page_transform);
    let item_start = context.text.chars().count();
    let mut item_quad: Option<Rect> = None;
    let mut item_text = String::new();

    for (byte_index, byte) in string.0.iter().copied().enumerate() {
        let character = decode_simple_byte(byte);
        let width_units = font
            .widths
            .get(byte.saturating_sub(font.first_char as u8) as usize)
            .copied()
            .unwrap_or(600.0);
        let advance = ((width_units / 1000.0) * text_state.font_size
            + text_state.character_spacing
            + if byte == b' ' {
                text_state.word_spacing
            } else {
                0.0
            })
            * scaling;
        let local_rect = Rect {
            x: 0.0,
            y: text_state.text_rise,
            width: advance.max(0.0),
            height: text_state.font_size.max(0.0),
        };
        let quad = local_rect.to_quad().transform(text_to_page);
        let bbox = quad.bounding_rect();
        let page_char_index = context.text.chars().count();
        context.glyphs.push(Glyph {
            text: character,
            bbox,
            quad,
            page_char_index,
            operation_index,
            location: match show_operand {
                ShowOperand::Direct { operand_index } => GlyphLocation::Direct {
                    operand_index,
                    byte_index,
                },
                ShowOperand::Array {
                    operand_index,
                    element_index,
                } => GlyphLocation::Array {
                    operand_index,
                    element_index,
                    byte_index,
                },
            },
        });
        item_quad = Some(match item_quad {
            Some(existing) => existing.union(&bbox),
            None => bbox,
        });
        context.text.push(character);
        item_text.push(character);
        text_state.text_matrix = text_state
            .text_matrix
            .multiply(Matrix::translate(advance, 0.0));
    }

    if !item_text.is_empty() {
        let item_end = context.text.chars().count();
        context.items.push(TextItem {
            text: item_text,
            bbox: item_quad.unwrap_or(Rect {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            }),
            quad: item_quad.map(Rect::to_quad),
            char_start: Some(item_start),
            char_end: Some(item_end),
        });
    }
    Ok(())
}

fn decode_simple_byte(byte: u8) -> char {
    if byte.is_ascii() {
        byte as char
    } else {
        '\u{FFFD}'
    }
}

fn matrix_from_operands(operands: &[PdfValue]) -> PdfResult<Matrix> {
    if operands.len() != 6 {
        return Err(PdfError::Corrupt(
            "matrix operator expects six operands".to_string(),
        ));
    }
    Ok(Matrix {
        a: operands[0]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("matrix operand is not numeric".to_string()))?,
        b: operands[1]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("matrix operand is not numeric".to_string()))?,
        c: operands[2]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("matrix operand is not numeric".to_string()))?,
        d: operands[3]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("matrix operand is not numeric".to_string()))?,
        e: operands[4]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("matrix operand is not numeric".to_string()))?,
        f: operands[5]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("matrix operand is not numeric".to_string()))?,
    })
}

fn operand_name(operation: &Operation, index: usize) -> PdfResult<&str> {
    operation
        .operands
        .get(index)
        .and_then(PdfValue::as_name)
        .ok_or_else(|| PdfError::Corrupt(format!("operand {index} is not a name")))
}

fn operand_number(operation: &Operation, index: usize) -> PdfResult<f64> {
    operation
        .operands
        .get(index)
        .and_then(PdfValue::as_number)
        .ok_or_else(|| PdfError::Corrupt(format!("operand {index} is not numeric")))
}

fn operand_string(operation: &Operation, index: usize) -> PdfResult<&pdf_objects::PdfString> {
    match operation.operands.get(index) {
        Some(PdfValue::String(string)) => Ok(string),
        _ => Err(PdfError::Corrupt(format!(
            "operand {index} is not a string"
        ))),
    }
}
