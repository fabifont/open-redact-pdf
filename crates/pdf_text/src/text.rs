use std::collections::{BTreeMap, BTreeSet};

use pdf_content::{Operation, ParsedPageContent, parse_page_contents};
use pdf_graphics::{Matrix, Quad, Rect};
use pdf_objects::{
    PageInfo, PdfError, PdfFile, PdfResult, PdfValue, decode_stream, document::get_stream,
};
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
pub enum GlyphLocation {
    Direct {
        operand_index: usize,
        byte_start: usize,
        byte_end: usize,
    },
    Array {
        operand_index: usize,
        element_index: usize,
        byte_start: usize,
        byte_end: usize,
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
    /// False when the glyph was rendered with Tr=3 (invisible mode).
    /// Invisible glyphs are still included for redaction but excluded from
    /// search results and extracted text items.
    pub visible: bool,
    /// Raw advance width in 1/1000 em font units. Used by kern-compensating
    /// redaction modes to preserve text positioning after byte removal.
    pub width_units: f64,
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
    normalized_text: String,
    normalized_to_display: Vec<usize>,
    display_chars: Vec<char>,
    display_to_glyph: Vec<Option<usize>>,
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
        let display_start = *index
            .normalized_to_display
            .get(normalized_start)
            .unwrap_or(&0);
        let display_end = index
            .normalized_to_display
            .get(normalized_end.saturating_sub(1))
            .copied()
            .unwrap_or(display_start)
            + 1;
        let glyph_indices = (display_start..display_end)
            .filter_map(|display_index| index.normalized_to_glyph(display_index))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let quads = coalesce_match_quads(
            &glyph_indices
                .iter()
                .filter_map(|glyph_index| page.glyphs.get(*glyph_index))
                .map(|glyph| glyph.quad)
                .collect::<Vec<_>>(),
        );
        if !quads.is_empty() {
            matches.push(TextMatch {
                text: index
                    .display_chars
                    .iter()
                    .skip(display_start)
                    .take(display_end.saturating_sub(display_start))
                    .collect::<String>()
                    .trim()
                    .to_string(),
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

fn coalesce_match_quads(quads: &[Quad]) -> Vec<Quad> {
    let mut rects = quads
        .iter()
        .map(|quad| quad.bounding_rect())
        .collect::<Vec<_>>();
    rects.sort_by(|left, right| {
        let y_delta = (left.y - right.y).abs();
        if y_delta > 1.5 {
            right
                .y
                .partial_cmp(&left.y)
                .unwrap_or(std::cmp::Ordering::Equal)
        } else {
            left.x
                .partial_cmp(&right.x)
                .unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    let mut merged = Vec::<Rect>::new();
    for rect in rects {
        let Some(previous) = merged.last_mut() else {
            merged.push(rect);
            continue;
        };
        if should_merge_match_rects(*previous, rect) {
            let next_x = previous.x.min(rect.x);
            let next_y = previous.y.min(rect.y);
            let next_max_x = previous.max_x().max(rect.max_x());
            let next_max_y = previous.max_y().max(rect.max_y());
            *previous = Rect {
                x: next_x,
                y: next_y,
                width: next_max_x - next_x,
                height: next_max_y - next_y,
            };
        } else {
            merged.push(rect);
        }
    }

    merged
        .into_iter()
        .map(expand_match_rect)
        .map(Rect::to_quad)
        .collect()
}

fn should_merge_match_rects(left: Rect, right: Rect) -> bool {
    let vertical_overlap = left.max_y().min(right.max_y()) - left.y.max(right.y);
    let minimum_height = left.height.min(right.height).max(1.0);
    let horizontal_gap = right.x - left.max_x();
    vertical_overlap >= minimum_height * 0.45 && horizontal_gap <= minimum_height * 0.8
}

fn expand_match_rect(rect: Rect) -> Rect {
    let padding_x = (rect.height * 0.08).max(0.6);
    let padding_y = (rect.height * 0.12).max(0.8);
    Rect {
        x: rect.x - padding_x,
        y: rect.y - padding_y,
        width: rect.width + padding_x * 2.0,
        height: rect.height + padding_y * 2.0,
    }
}

fn build_search_index(page: &ExtractedPageText) -> PageSearchIndex {
    let (display_chars, display_to_glyph) = build_visual_display(page);
    let mut normalized_text = String::new();
    let mut normalized_to_display = Vec::new();
    let mut previous_was_whitespace = false;
    for (display_index, character) in display_chars.iter().copied().enumerate() {
        if character.is_whitespace() {
            if !previous_was_whitespace {
                normalized_text.push(' ');
                normalized_to_display.push(display_index);
                previous_was_whitespace = true;
            }
        } else {
            for folded in character.to_lowercase() {
                normalized_text.push(folded);
                normalized_to_display.push(display_index);
            }
            previous_was_whitespace = false;
        }
    }
    PageSearchIndex {
        normalized_text,
        normalized_to_display,
        display_chars,
        display_to_glyph,
    }
}

impl PageSearchIndex {
    fn normalized_to_glyph(&self, display_index: usize) -> Option<usize> {
        self.display_to_glyph
            .get(display_index)
            .copied()
            .flatten()
    }
}

fn build_visual_display(page: &ExtractedPageText) -> (Vec<char>, Vec<Option<usize>>) {
    let mut lines = build_visual_lines(page);
    for line in &mut lines {
        line.sort_by(|left, right| {
            page.glyphs[*left]
                .bbox
                .x
                .partial_cmp(&page.glyphs[*right].bbox.x)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    lines.sort_by(|left, right| {
        let left_y = average_line_center_y(page, left);
        let right_y = average_line_center_y(page, right);
        right_y
            .partial_cmp(&left_y)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut display_chars = Vec::new();
    let mut display_to_glyph = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        if line_index > 0 {
            display_chars.push('\n');
            display_to_glyph.push(None);
        }
        let mut previous_rect: Option<Rect> = None;
        for glyph_index in line {
            let glyph = &page.glyphs[*glyph_index];
            if let Some(previous) = previous_rect {
                let gap = glyph.bbox.x - previous.max_x();
                let threshold = previous.height.min(glyph.bbox.height).max(1.0) * 0.3;
                if gap > threshold {
                    display_chars.push(' ');
                    display_to_glyph.push(None);
                }
            }
            display_chars.push(glyph.text);
            display_to_glyph.push(Some(*glyph_index));
            previous_rect = Some(glyph.bbox);
        }
    }
    (display_chars, display_to_glyph)
}

fn build_visual_lines(page: &ExtractedPageText) -> Vec<Vec<usize>> {
    let mut indices = (0..page.glyphs.len())
        .filter(|i| page.glyphs[*i].visible)
        .collect::<Vec<_>>();
    indices.sort_by(|left, right| {
        let left_center = glyph_center_y(&page.glyphs[*left]);
        let right_center = glyph_center_y(&page.glyphs[*right]);
        let y_delta = (left_center - right_center).abs();
        if y_delta > 1.5 {
            right_center
                .partial_cmp(&left_center)
                .unwrap_or(std::cmp::Ordering::Equal)
        } else {
            page.glyphs[*left]
                .bbox
                .x
                .partial_cmp(&page.glyphs[*right].bbox.x)
                .unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    let mut lines: Vec<Vec<usize>> = Vec::new();
    for glyph_index in indices {
        let Some(target_line) = lines
            .iter_mut()
            .find(|line| glyph_belongs_to_line(page, glyph_index, line))
        else {
            lines.push(vec![glyph_index]);
            continue;
        };
        target_line.push(glyph_index);
    }
    lines
}

fn glyph_belongs_to_line(page: &ExtractedPageText, glyph_index: usize, line: &[usize]) -> bool {
    let glyph = &page.glyphs[glyph_index];
    let glyph_center = glyph_center_y(glyph);
    let line_center = average_line_center_y(page, line);
    let line_height = average_line_height(page, line).max(glyph.bbox.height).max(1.0);
    let overlap = line_vertical_overlap(page, glyph, line);
    (glyph_center - line_center).abs() <= line_height * 0.55 || overlap >= line_height * 0.35
}

fn average_line_center_y(page: &ExtractedPageText, line: &[usize]) -> f64 {
    let total = line
        .iter()
        .map(|glyph_index| glyph_center_y(&page.glyphs[*glyph_index]))
        .sum::<f64>();
    total / line.len().max(1) as f64
}

fn average_line_height(page: &ExtractedPageText, line: &[usize]) -> f64 {
    let total = line
        .iter()
        .map(|glyph_index| page.glyphs[*glyph_index].bbox.height)
        .sum::<f64>();
    total / line.len().max(1) as f64
}

fn line_vertical_overlap(page: &ExtractedPageText, glyph: &Glyph, line: &[usize]) -> f64 {
    line.iter()
        .map(|glyph_index| {
            let candidate = &page.glyphs[*glyph_index];
            glyph.bbox.max_y().min(candidate.bbox.max_y()) - glyph.bbox.y.max(candidate.bbox.y)
        })
        .fold(0.0, f64::max)
}

fn glyph_center_y(glyph: &Glyph) -> f64 {
    glyph.bbox.y + glyph.bbox.height / 2.0
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
            for folded in character.to_lowercase() {
                output.push(folded);
            }
            previous_was_whitespace = false;
        }
    }
    output.trim().to_string()
}

fn interpret_page_text(
    page_index: usize,
    page: &PageInfo,
    parsed: &ParsedPageContent,
    fonts: &BTreeMap<String, LoadedFont>,
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
            "Tr" => {
                text_state.text_render_mode = operation
                    .operands
                    .first()
                    .and_then(PdfValue::as_integer)
                    .unwrap_or(0);
            }
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
struct CompositeFont {
    encoding: String,
    default_width: f64,
    widths: BTreeMap<u16, f64>,
    unicode_map: BTreeMap<u16, String>,
}

#[derive(Debug, Clone)]
enum LoadedFont {
    Simple(SimpleFont),
    Composite(CompositeFont),
}

#[derive(Debug, Clone)]
struct DecodedGlyph {
    text: String,
    width_units: f64,
    byte_start: usize,
    byte_end: usize,
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
    /// PDF text rendering mode. Mode 3 = invisible (used in OCR/PDF-A).
    text_render_mode: i64,
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
            text_render_mode: 0,
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
) -> PdfResult<BTreeMap<String, LoadedFont>> {
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
        let font = match subtype {
            "Type1" | "TrueType" => {
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
                LoadedFont::Simple(SimpleFont { widths, first_char })
            }
            "Type0" => LoadedFont::Composite(load_composite_font(file, font_dict)?),
            other => {
                return Err(PdfError::Unsupported(format!(
                    "font subtype {other} is not supported"
                )));
            }
        };
        fonts.insert(name.clone(), font);
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
    fonts: &BTreeMap<String, LoadedFont>,
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
    let item_start = context.text.chars().count();
    let mut item_quad: Option<Rect> = None;
    let mut item_text = String::new();
    let decoded_glyphs = decode_font_glyphs(font, &string.0)?;

    for decoded in decoded_glyphs {
        let advance = ((decoded.width_units / 1000.0) * text_state.font_size
            + text_state.character_spacing
            + if decoded.text == " " {
                text_state.word_spacing
            } else {
                0.0
            })
            * scaling;
        let text_to_page = text_state
            .text_matrix
            .multiply(ctm)
            .multiply(page_transform);
        // Use heuristic ascent/descent (80%/12% of em-square) instead of the
        // full font_size to avoid bboxes that span into adjacent text lines.
        let font_size = text_state.font_size.max(0.0);
        let local_rect = Rect {
            x: 0.0,
            y: text_state.text_rise - font_size * 0.12,
            width: advance.max(0.0),
            height: font_size * 0.8,
        };
        let quad = local_rect.to_quad().transform(text_to_page);
        let bbox = quad.bounding_rect();
        item_quad = Some(match item_quad {
            Some(existing) => existing.union(&bbox),
            None => bbox,
        });
        for character in decoded.text.chars() {
            let page_char_index = context.text.chars().count();
            let visible = text_state.text_render_mode != 3;
            context.glyphs.push(Glyph {
                text: character,
                bbox,
                quad,
                page_char_index,
                operation_index,
                location: match show_operand {
                    ShowOperand::Direct { operand_index } => GlyphLocation::Direct {
                        operand_index,
                        byte_start: decoded.byte_start,
                        byte_end: decoded.byte_end,
                    },
                    ShowOperand::Array {
                        operand_index,
                        element_index,
                    } => GlyphLocation::Array {
                        operand_index,
                        element_index,
                        byte_start: decoded.byte_start,
                        byte_end: decoded.byte_end,
                    },
                },
                visible,
                width_units: decoded.width_units,
            });
            if visible {
                context.text.push(character);
                item_text.push(character);
            }
        }
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

fn decode_font_glyphs(font: &LoadedFont, bytes: &[u8]) -> PdfResult<Vec<DecodedGlyph>> {
    match font {
        LoadedFont::Simple(font) => Ok(bytes
            .iter()
            .copied()
            .enumerate()
            .map(|(byte_index, byte)| {
                let width_units = font
                    .widths
                    .get(u16::from(byte).saturating_sub(font.first_char) as usize)
                    .copied()
                    .unwrap_or(600.0);
                DecodedGlyph {
                    text: decode_simple_byte(byte).to_string(),
                    width_units,
                    byte_start: byte_index,
                    byte_end: byte_index + 1,
                }
            })
            .collect()),
        LoadedFont::Composite(font) => decode_composite_glyphs(font, bytes),
    }
}

fn load_composite_font(
    file: &PdfFile,
    font_dict: &pdf_objects::PdfDictionary,
) -> PdfResult<CompositeFont> {
    let encoding = font_dict
        .get("Encoding")
        .and_then(PdfValue::as_name)
        .unwrap_or("Identity-H")
        .to_string();
    if encoding != "Identity-H" {
        return Err(PdfError::Unsupported(format!(
            "Type0 font encoding {encoding} is not supported"
        )));
    }

    let descendant = font_dict
        .get("DescendantFonts")
        .and_then(PdfValue::as_array)
        .and_then(|fonts| fonts.first())
        .ok_or_else(|| PdfError::Corrupt("Type0 font is missing DescendantFonts".to_string()))?;
    let descendant_dict = file.resolve_dict(descendant)?;
    let descendant_subtype = descendant_dict
        .get("Subtype")
        .and_then(PdfValue::as_name)
        .unwrap_or("");
    if !matches!(descendant_subtype, "CIDFontType0" | "CIDFontType2") {
        return Err(PdfError::Unsupported(format!(
            "descendant font subtype {descendant_subtype} is not supported"
        )));
    }

    let default_width = descendant_dict
        .get("DW")
        .and_then(PdfValue::as_number)
        .unwrap_or(1000.0);
    let widths = descendant_dict
        .get("W")
        .map(parse_cid_widths)
        .transpose()?
        .unwrap_or_default();
    let unicode_map = load_to_unicode_map(file, font_dict)?;

    Ok(CompositeFont {
        encoding,
        default_width,
        widths,
        unicode_map,
    })
}

fn parse_cid_widths(value: &PdfValue) -> PdfResult<BTreeMap<u16, f64>> {
    let array = value
        .as_array()
        .ok_or_else(|| PdfError::Corrupt("CID font W entry must be an array".to_string()))?;
    let mut widths = BTreeMap::new();
    let mut index = 0usize;
    while index < array.len() {
        let start_cid = array[index]
            .as_integer()
            .ok_or_else(|| PdfError::Corrupt("CID width entry is invalid".to_string()))?
            as u16;
        let next = array
            .get(index + 1)
            .ok_or_else(|| PdfError::Corrupt("CID width entry is truncated".to_string()))?;
        if let Some(width_array) = next.as_array() {
            for (offset, width) in width_array.iter().enumerate() {
                let cid = start_cid.checked_add(offset as u16).ok_or_else(|| {
                    PdfError::Corrupt("CID width index overflow".to_string())
                })?;
                widths.insert(
                    cid,
                    width.as_number().ok_or_else(|| {
                        PdfError::Corrupt("CID width array contains a non-number".to_string())
                    })?,
                );
            }
            index += 2;
        } else {
            let end_cid = next
                .as_integer()
                .ok_or_else(|| PdfError::Corrupt("CID width range is invalid".to_string()))?
                as u16;
            let width = array
                .get(index + 2)
                .and_then(PdfValue::as_number)
                .ok_or_else(|| PdfError::Corrupt("CID width range is truncated".to_string()))?;
            for cid in start_cid..=end_cid {
                widths.insert(cid, width);
            }
            index += 3;
        }
    }
    Ok(widths)
}

fn load_to_unicode_map(
    file: &PdfFile,
    font_dict: &pdf_objects::PdfDictionary,
) -> PdfResult<BTreeMap<u16, String>> {
    let Some(to_unicode_value) = font_dict.get("ToUnicode") else {
        return Ok(BTreeMap::new());
    };
    let to_unicode_ref = match to_unicode_value {
        PdfValue::Reference(reference) => *reference,
        _ => {
            return Err(PdfError::Unsupported(
                "direct ToUnicode streams are not supported".to_string(),
            ));
        }
    };
    let stream = get_stream(file, to_unicode_ref)?;
    let decoded = decode_stream(stream)?;
    parse_to_unicode_cmap(&decoded)
}

fn parse_to_unicode_cmap(data: &[u8]) -> PdfResult<BTreeMap<u16, String>> {
    let text = String::from_utf8_lossy(data);
    let mut mapping = BTreeMap::new();
    enum Mode {
        BfChar,
        BfRange,
    }
    let mut mode = None;
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line.ends_with("beginbfchar") {
            mode = Some(Mode::BfChar);
            continue;
        }
        if line.ends_with("endbfchar") {
            mode = None;
            continue;
        }
        if line.ends_with("beginbfrange") {
            mode = Some(Mode::BfRange);
            continue;
        }
        if line.ends_with("endbfrange") {
            mode = None;
            continue;
        }
        match mode {
            Some(Mode::BfChar) => parse_bfchar_line(line, &mut mapping)?,
            Some(Mode::BfRange) => parse_bfrange_line(line, &mut mapping)?,
            None => {}
        }
    }
    Ok(mapping)
}

fn parse_bfchar_line(line: &str, mapping: &mut BTreeMap<u16, String>) -> PdfResult<()> {
    let tokens = extract_hex_tokens(line);
    if tokens.len() < 2 {
        return Ok(());
    }
    mapping.insert(parse_cid_token(&tokens[0])?, decode_utf16be_lossy(&tokens[1]));
    Ok(())
}

fn parse_bfrange_line(line: &str, mapping: &mut BTreeMap<u16, String>) -> PdfResult<()> {
    let tokens = extract_hex_tokens(line);
    if tokens.len() < 3 {
        return Ok(());
    }
    let start = parse_cid_token(&tokens[0])?;
    let end = parse_cid_token(&tokens[1])?;
    if line.contains('[') {
        for (offset, destination) in tokens.iter().skip(2).enumerate() {
            let cid = start.saturating_add(offset as u16);
            if cid > end {
                break;
            }
            mapping.insert(cid, decode_utf16be_lossy(destination));
        }
        return Ok(());
    }

    let base = parse_unicode_scalar(&tokens[2])?;
    for cid in start..=end {
        let scalar = base + u32::from(cid - start);
        mapping.insert(
            cid,
            char::from_u32(scalar).unwrap_or('\u{FFFD}').to_string(),
        );
    }
    Ok(())
}

fn extract_hex_tokens(line: &str) -> Vec<Vec<u8>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_hex = false;
    for character in line.chars() {
        match character {
            '<' => {
                current.clear();
                in_hex = true;
            }
            '>' if in_hex => {
                if let Ok(bytes) = parse_hex_string_token(&current) {
                    tokens.push(bytes);
                }
                in_hex = false;
            }
            _ if in_hex => current.push(character),
            _ => {}
        }
    }
    tokens
}

fn parse_hex_string_token(token: &str) -> PdfResult<Vec<u8>> {
    let filtered = token.chars().filter(|character| !character.is_whitespace()).collect::<String>();
    let mut chars = filtered.chars().collect::<Vec<_>>();
    if chars.len() % 2 != 0 {
        chars.push('0');
    }
    let mut bytes = Vec::with_capacity(chars.len() / 2);
    for pair in chars.chunks(2) {
        let byte = u8::from_str_radix(&pair.iter().collect::<String>(), 16)
            .map_err(|_| PdfError::Corrupt("invalid ToUnicode hex token".to_string()))?;
        bytes.push(byte);
    }
    Ok(bytes)
}

fn parse_cid_token(bytes: &[u8]) -> PdfResult<u16> {
    match bytes {
        [single] => Ok(u16::from(*single)),
        [high, low] => Ok(u16::from_be_bytes([*high, *low])),
        _ => Err(PdfError::Unsupported(
            "only one-byte and two-byte CIDs are supported".to_string(),
        )),
    }
}

fn parse_unicode_scalar(bytes: &[u8]) -> PdfResult<u32> {
    match bytes {
        [high, low] => Ok(u32::from(u16::from_be_bytes([*high, *low]))),
        [0, 0, high, low] => Ok(u32::from(u16::from_be_bytes([*high, *low]))),
        _ => Err(PdfError::Unsupported(
            "sequential ToUnicode ranges must use a single UTF-16 code unit".to_string(),
        )),
    }
}

fn decode_utf16be_lossy(bytes: &[u8]) -> String {
    let mut units = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        let high = bytes[index];
        let low = *bytes.get(index + 1).unwrap_or(&0);
        units.push(u16::from_be_bytes([high, low]));
        index += 2;
    }
    String::from_utf16_lossy(&units)
}

fn decode_composite_glyphs(font: &CompositeFont, bytes: &[u8]) -> PdfResult<Vec<DecodedGlyph>> {
    if font.encoding != "Identity-H" {
        return Err(PdfError::Unsupported(format!(
            "Type0 font encoding {} is not supported",
            font.encoding
        )));
    }
    if bytes.len() % 2 != 0 {
        return Err(PdfError::Corrupt(
            "Identity-H strings must contain an even number of bytes".to_string(),
        ));
    }
    let mut glyphs = Vec::new();
    let mut byte_index = 0usize;
    while byte_index < bytes.len() {
        let cid = u16::from_be_bytes([bytes[byte_index], bytes[byte_index + 1]]);
        let text = font
            .unicode_map
            .get(&cid)
            .cloned()
            .unwrap_or_else(|| decode_fallback_cid(cid));
        let width_units = font.widths.get(&cid).copied().unwrap_or(font.default_width);
        glyphs.push(DecodedGlyph {
            text,
            width_units,
            byte_start: byte_index,
            byte_end: byte_index + 2,
        });
        byte_index += 2;
    }
    Ok(glyphs)
}

fn decode_fallback_cid(cid: u16) -> String {
    if cid <= 0x7f {
        char::from_u32(u32::from(cid)).unwrap_or('\u{FFFD}').to_string()
    } else {
        '\u{FFFD}'.to_string()
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

#[cfg(test)]
mod tests {
    use super::coalesce_match_quads;
    use pdf_graphics::{Point, Quad};

    #[test]
    fn coalesces_adjacent_glyph_quads_into_match_regions() {
        let quads = vec![
            Quad {
                points: [
                    Point { x: 10.0, y: 20.0 },
                    Point { x: 14.0, y: 20.0 },
                    Point { x: 14.0, y: 30.0 },
                    Point { x: 10.0, y: 30.0 },
                ],
            },
            Quad {
                points: [
                    Point { x: 14.3, y: 20.0 },
                    Point { x: 18.5, y: 20.0 },
                    Point { x: 18.5, y: 30.0 },
                    Point { x: 14.3, y: 30.0 },
                ],
            },
            Quad {
                points: [
                    Point { x: 50.0, y: 5.0 },
                    Point { x: 54.0, y: 5.0 },
                    Point { x: 54.0, y: 15.0 },
                    Point { x: 50.0, y: 15.0 },
                    ],
            },
        ];

        let merged = coalesce_match_quads(&quads);
        assert_eq!(merged.len(), 2);
        let first = merged[0].bounding_rect();
        assert!(first.x < 10.0);
        assert!(first.max_x() > 18.5);
    }
}
