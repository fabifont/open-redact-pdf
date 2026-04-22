use std::collections::{BTreeMap, BTreeSet};

use pdf_content::{Operation, ParsedPageContent, parse_content_stream, parse_page_contents};
use pdf_graphics::{Matrix, Quad, Rect};
use pdf_objects::{
    ObjectRef, PageInfo, PdfDictionary, PdfError, PdfFile, PdfResult, PdfValue, decode_stream,
    document::get_stream,
};
use serde::{Deserialize, Serialize};

/// Maximum Form XObject recursion depth. Prevents pathological or adversarial
/// PDFs from driving the interpreter into unbounded recursion.
const MAX_FORM_XOBJECT_DEPTH: usize = 16;

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
    /// When `Some`, the glyph originated inside a Form XObject invoked by
    /// `Do` on the page's content stream. `operation_index` and `location`
    /// then refer to positions inside that Form's content stream, not the
    /// page's — callers that want to rewrite the bytes must look up the
    /// corresponding Form object, decode its stream, and mutate the
    /// operations of the *Form*, not the page.
    pub source_form: Option<ObjectRef>,
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
    let (fonts, extgstate_fonts) = load_fonts(file, &page.resources)?;
    interpret_page_text(file, page_index, page, &parsed, &fonts, &extgstate_fonts)
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
                // One entry per UTF-8 byte so byte offsets from str::find() map correctly
                for _ in 0..' '.len_utf8() {
                    normalized_to_display.push(display_index);
                }
                previous_was_whitespace = true;
            }
        } else {
            for folded in character.to_lowercase() {
                normalized_text.push(folded);
                for _ in 0..folded.len_utf8() {
                    normalized_to_display.push(display_index);
                }
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
        self.display_to_glyph.get(display_index).copied().flatten()
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
    // Deterministic sort: primary y descending, secondary x ascending,
    // with no threshold-based bucketing. A tolerant bucket (e.g. 1.5 pt)
    // produced an intransitive comparator — two close-but-distinct lines
    // would interleave in the sorted output, scrambling their characters
    // once per-line x-sorting ran. Line grouping itself is handled by
    // `glyph_belongs_to_line`, which still tolerates intra-line y
    // jitter from subscripts or baseline adjustments.
    indices.sort_by(|left, right| {
        let left_center = glyph_center_y(&page.glyphs[*left]);
        let right_center = glyph_center_y(&page.glyphs[*right]);
        right_center
            .partial_cmp(&left_center)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                page.glyphs[*left]
                    .bbox
                    .x
                    .partial_cmp(&page.glyphs[*right].bbox.x)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
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
    let line_height = average_line_height(page, line)
        .max(glyph.bbox.height)
        .max(1.0);
    // Y tolerance with an absolute cap. Glyph bbox heights fall back
    // to ~80% of the font em-square when font-metrics parsing is not
    // available; for dense layouts (bank statements, address blocks,
    // tax forms) the actual inter-line spacing is a fraction of that
    // estimate, so a purely ratio-based tolerance over-merges adjacent
    // rows. Capping the tolerance at 1 pt in user space lets typical
    // per-line jitter through while keeping 1-2 pt baseline steps on
    // separate lines.
    let y_tolerance = (line_height * 0.3).min(1.0);
    if (glyph_center - line_center).abs() > y_tolerance {
        return false;
    }

    // X-monotonicity guard. A row of text has glyphs whose starting
    // x coordinates ascend monotonically; a new glyph whose start x
    // sits meaningfully before the last-encountered glyph's start x
    // is almost certainly on a different visual row that happens to
    // be only a point or two apart in y. Use `bbox.x` of each glyph
    // (not `bbox.max_x()`) because aggressive TJ-array kerning in
    // compressed text blocks makes successive glyph bboxes overlap
    // heavily; the start-x of the most-recently appended glyph is a
    // reliable "line watermark" thanks to the (y-desc, x-asc) feeder
    // sort.
    if let Some(last_on_line) = line.last().copied().map(|i| &page.glyphs[i]) {
        let kerning_slack = 1.0;
        if glyph.bbox.x + kerning_slack < last_on_line.bbox.x {
            return false;
        }
    }
    true
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
    file: &PdfFile,
    page_index: usize,
    page: &PageInfo,
    parsed: &ParsedPageContent,
    fonts: &BTreeMap<String, LoadedFont>,
    extgstate_fonts: &ExtGStateFontMap,
) -> PdfResult<ExtractedPageText> {
    let page_transform = page.page_box.normalized_transform();
    let mut context = TextContext::new(page_index);
    let mut ctm = Matrix::identity();
    let mut ctm_stack: Vec<(Matrix, RuntimeTextState)> = Vec::new();
    let mut text_state = RuntimeTextState::default();
    let mut xobject_stack: BTreeSet<ObjectRef> = BTreeSet::new();

    run_operations(
        file,
        &parsed.operations,
        fonts,
        extgstate_fonts,
        &page.resources,
        page_transform,
        &mut ctm,
        &mut ctm_stack,
        &mut text_state,
        &mut context,
        &mut xobject_stack,
        0,
    )?;

    Ok(ExtractedPageText {
        page_index,
        text: context.text,
        items: context.items,
        glyphs: context.glyphs,
    })
}

#[allow(clippy::too_many_arguments)]
fn run_operations(
    file: &PdfFile,
    operations: &[Operation],
    fonts: &BTreeMap<String, LoadedFont>,
    extgstate_fonts: &ExtGStateFontMap,
    resources: &PdfDictionary,
    page_transform: Matrix,
    ctm: &mut Matrix,
    ctm_stack: &mut Vec<(Matrix, RuntimeTextState)>,
    text_state: &mut RuntimeTextState,
    context: &mut TextContext,
    xobject_stack: &mut BTreeSet<ObjectRef>,
    depth: usize,
) -> PdfResult<()> {
    for (operation_index, operation) in operations.iter().enumerate() {
        match operation.operator.as_str() {
            "q" => ctm_stack.push((*ctm, text_state.clone())),
            "Q" => {
                let (saved_ctm, saved_text_state) = ctm_stack
                    .pop()
                    .unwrap_or((Matrix::identity(), RuntimeTextState::default()));
                *ctm = saved_ctm;
                *text_state = saved_text_state;
            }
            "gs" => {
                let gs_name = operand_name(operation, 0)?;
                if let Some((font_key, font_size)) = extgstate_fonts.get(gs_name) {
                    text_state.font = Some(font_key.clone());
                    text_state.font_size = *font_size;
                }
            }
            "cm" => {
                let matrix = matrix_from_operands(&operation.operands)?;
                *ctm = matrix.multiply(*ctm);
            }
            "BT" => {
                text_state.text_matrix = Matrix::identity();
                text_state.line_matrix = Matrix::identity();
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
                // Text/line matrix updates compose a text-space translation
                // BEFORE the current matrix (row-vector: `translate * Tm`),
                // not after. With scaled text matrices (Tm like 9.5 0 0 9.5 x y)
                // the previous `Tm * translate` form added tx/ty directly to
                // page coordinates instead of applying them in text space.
                text_state.line_matrix = Matrix::translate(tx, ty).multiply(text_state.line_matrix);
                text_state.text_matrix = text_state.line_matrix;
                if ty.abs() > f64::EPSILON {
                    context.pending_line_break = true;
                }
            }
            "TD" => {
                let tx = operand_number(operation, 0)?;
                let ty = operand_number(operation, 1)?;
                text_state.leading = -ty;
                text_state.line_matrix = Matrix::translate(tx, ty).multiply(text_state.line_matrix);
                text_state.text_matrix = text_state.line_matrix;
                context.pending_line_break = true;
            }
            "T*" => {
                text_state.line_matrix = Matrix::translate(0.0, -text_state.leading)
                    .multiply(text_state.line_matrix);
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
                    context,
                    operation_index,
                    ShowOperand::Direct { operand_index: 0 },
                    string,
                    text_state,
                    fonts,
                    *ctm,
                    page_transform,
                )?;
            }
            "'" => {
                text_state.line_matrix = Matrix::translate(0.0, -text_state.leading)
                    .multiply(text_state.line_matrix);
                text_state.text_matrix = text_state.line_matrix;
                context.pending_line_break = true;
                let string = operand_string(operation, 0)?;
                show_text(
                    context,
                    operation_index,
                    ShowOperand::Direct { operand_index: 0 },
                    string,
                    text_state,
                    fonts,
                    *ctm,
                    page_transform,
                )?;
            }
            "\"" => {
                text_state.word_spacing = operand_number(operation, 0)?;
                text_state.character_spacing = operand_number(operation, 1)?;
                text_state.line_matrix = Matrix::translate(0.0, -text_state.leading)
                    .multiply(text_state.line_matrix);
                text_state.text_matrix = text_state.line_matrix;
                context.pending_line_break = true;
                let string = operand_string(operation, 2)?;
                show_text(
                    context,
                    operation_index,
                    ShowOperand::Direct { operand_index: 2 },
                    string,
                    text_state,
                    fonts,
                    *ctm,
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
                            context,
                            operation_index,
                            ShowOperand::Array {
                                operand_index: 0,
                                element_index,
                            },
                            string,
                            text_state,
                            fonts,
                            *ctm,
                            page_transform,
                        )?,
                        value => {
                            let adjustment = value.as_number().ok_or_else(|| {
                                PdfError::Corrupt("TJ array contains unsupported value".to_string())
                            })?;
                            let scaled = -(adjustment / 1000.0)
                                * text_state.font_size
                                * (text_state.horizontal_scaling / 100.0);
                            text_state.text_matrix = Matrix::translate(scaled, 0.0)
                                .multiply(text_state.text_matrix);
                        }
                    }
                }
            }
            "Do" => {
                let name = operand_name(operation, 0)?;
                enter_form_xobject(
                    file,
                    name,
                    fonts,
                    extgstate_fonts,
                    resources,
                    page_transform,
                    ctm,
                    ctm_stack,
                    text_state,
                    context,
                    xobject_stack,
                    depth,
                )?;
            }
            _ => {}
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn enter_form_xobject(
    file: &PdfFile,
    name: &str,
    outer_fonts: &BTreeMap<String, LoadedFont>,
    outer_extgstate: &ExtGStateFontMap,
    outer_resources: &PdfDictionary,
    page_transform: Matrix,
    ctm: &mut Matrix,
    ctm_stack: &mut Vec<(Matrix, RuntimeTextState)>,
    text_state: &mut RuntimeTextState,
    context: &mut TextContext,
    xobject_stack: &mut BTreeSet<ObjectRef>,
    depth: usize,
) -> PdfResult<()> {
    if depth + 1 > MAX_FORM_XOBJECT_DEPTH {
        return Ok(());
    }
    let Some(xobject_ref) = lookup_xobject_ref(outer_resources, name) else {
        return Ok(());
    };
    if !xobject_stack.insert(xobject_ref) {
        // Cycle detected — bail out of this branch without an error.
        return Ok(());
    }

    let result = (|| -> PdfResult<()> {
        let stream = get_stream(file, xobject_ref)?;
        if stream.dict.get("Subtype").and_then(PdfValue::as_name) != Some("Form") {
            // Image XObjects (or unknown subtypes) carry no text — skip silently.
            return Ok(());
        }

        let form_matrix = stream
            .dict
            .get("Matrix")
            .and_then(PdfValue::as_array)
            .map(matrix_from_pdf_values)
            .transpose()?
            .unwrap_or_else(Matrix::identity);

        let form_resources_owned: PdfDictionary = stream
            .dict
            .get("Resources")
            .map(|value| file.resolve_dict(value).cloned())
            .transpose()?
            .unwrap_or_else(|| outer_resources.clone());

        let (form_fonts, form_extgstate) = load_fonts(file, &form_resources_owned)?;
        // Inherit from the caller's maps when the Form's own Resources did not
        // declare a given resource name. This matches PDF 32000-1 § 7.8.3: the
        // Form's Resources are preferred, but the parent scope is a fall-back
        // when the Form omits an entry.
        let mut effective_fonts: BTreeMap<String, LoadedFont> = outer_fonts.clone();
        for (key, value) in form_fonts {
            effective_fonts.insert(key, value);
        }
        let mut effective_extgstate: ExtGStateFontMap = outer_extgstate.clone();
        for (key, value) in form_extgstate {
            effective_extgstate.insert(key, value);
        }

        let decoded = decode_stream(stream)?;
        let form_operations = parse_content_stream(&decoded)?.operations;

        // Form invocation is bracketed by an implicit q/Q. Save CTM and
        // text_state, pre-multiply the Form's /Matrix into the CTM, and
        // restore both on exit. The current_form marker on the context lets
        // callers (redact) tell the difference between glyphs produced by
        // the page's own content stream and glyphs produced inside this
        // Form — their operation_index and location refer to different
        // byte streams.
        let saved_ctm = *ctm;
        let saved_text_state = text_state.clone();
        let saved_form = context.current_form;
        *ctm = form_matrix.multiply(saved_ctm);
        context.current_form = Some(xobject_ref);

        run_operations(
            file,
            &form_operations,
            &effective_fonts,
            &effective_extgstate,
            &form_resources_owned,
            page_transform,
            ctm,
            ctm_stack,
            text_state,
            context,
            xobject_stack,
            depth + 1,
        )?;

        *ctm = saved_ctm;
        *text_state = saved_text_state;
        context.current_form = saved_form;
        Ok(())
    })();

    xobject_stack.remove(&xobject_ref);
    result
}

fn lookup_xobject_ref(resources: &PdfDictionary, name: &str) -> Option<ObjectRef> {
    let xobjects = match resources.get("XObject")? {
        PdfValue::Dictionary(dict) => dict,
        _ => return None,
    };
    match xobjects.get(name)? {
        PdfValue::Reference(object_ref) => Some(*object_ref),
        _ => None,
    }
}

fn matrix_from_pdf_values(values: &[PdfValue]) -> PdfResult<Matrix> {
    if values.len() != 6 {
        return Err(PdfError::Corrupt(
            "Matrix array must have six numeric entries".to_string(),
        ));
    }
    let mut numbers = [0.0; 6];
    for (slot, value) in numbers.iter_mut().zip(values.iter()) {
        *slot = value
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("Matrix entry is not a number".to_string()))?;
    }
    Ok(Matrix {
        a: numbers[0],
        b: numbers[1],
        c: numbers[2],
        d: numbers[3],
        e: numbers[4],
        f: numbers[5],
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
    unicode_map: BTreeMap<u16, String>,
    encoding: SimpleEncoding,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct SimpleEncoding {
    /// Base named encoding. When unset the fallback is the identity path
    /// (ASCII only).
    base: SimpleEncodingBase,
    /// Byte-level overrides from an `/Encoding` dictionary's `/Differences`
    /// array. Keys are byte codes; values are PDF glyph names (e.g. `"Adieresis"`).
    differences: BTreeMap<u8, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SimpleEncodingBase {
    /// Fallback: treat bytes as identity if they map to ASCII, otherwise
    /// emit the Unicode replacement character.
    #[default]
    Identity,
    /// PDF `WinAnsiEncoding` — a superset of ISO-8859-1 with Windows-1252
    /// punctuation and symbols in the `0x80..=0x9F` range.
    WinAnsi,
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
    /// The Form XObject reference whose content is currently being
    /// interpreted. `None` while the page's own content stream is active.
    /// Glyphs pushed while this is `Some(_)` carry the same reference in
    /// their `source_form` field so downstream consumers (redact, in
    /// particular) can identify which stream holds the glyph's bytes.
    current_form: Option<ObjectRef>,
}

impl TextContext {
    fn new(page_index: usize) -> Self {
        let _ = page_index;
        Self {
            text: String::new(),
            items: Vec::new(),
            glyphs: Vec::new(),
            pending_line_break: false,
            current_form: None,
        }
    }
}

/// Maps ExtGState resource name → (synthetic font key in the fonts map, font size).
type ExtGStateFontMap = BTreeMap<String, (String, f64)>;

fn load_single_font(
    file: &PdfFile,
    font_dict: &pdf_objects::PdfDictionary,
) -> PdfResult<LoadedFont> {
    let subtype = font_dict
        .get("Subtype")
        .and_then(PdfValue::as_name)
        .unwrap_or("");
    match subtype {
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
            let unicode_map = load_to_unicode_map(file, font_dict)?;
            let encoding = parse_simple_encoding(file, font_dict);
            Ok(LoadedFont::Simple(SimpleFont {
                widths,
                first_char,
                unicode_map,
                encoding,
            }))
        }
        "Type0" => Ok(LoadedFont::Composite(load_composite_font(file, font_dict)?)),
        other => Err(PdfError::Unsupported(format!(
            "font subtype {other} is not supported"
        ))),
    }
}

fn load_fonts(
    file: &PdfFile,
    resources: &pdf_objects::PdfDictionary,
) -> PdfResult<(BTreeMap<String, LoadedFont>, ExtGStateFontMap)> {
    let mut fonts = BTreeMap::new();
    let mut extgstate_fonts = BTreeMap::new();

    // Load fonts from the Font resource dictionary
    if let Some(fonts_value) = resources.get("Font") {
        let fonts_dict = file.resolve_dict(fonts_value)?;
        for (name, font_value) in fonts_dict {
            let font_dict = file.resolve_dict(font_value)?;
            let font = load_single_font(file, font_dict)?;
            fonts.insert(name.clone(), font);
        }
    }

    // Scan ExtGState entries for Font arrays
    if let Some(extgstate_value) = resources.get("ExtGState") {
        if let Ok(extgstate_dict) = file.resolve_dict(extgstate_value) {
            for (gs_name, gs_value) in extgstate_dict {
                let Ok(gs_dict) = file.resolve_dict(gs_value) else {
                    continue;
                };
                let Some(font_array) = gs_dict.get("Font").and_then(PdfValue::as_array) else {
                    continue;
                };
                if font_array.len() < 2 {
                    continue;
                }
                let font_size = font_array[1].as_number().unwrap_or(12.0);
                let Ok(font_dict) = file.resolve_dict(&font_array[0]) else {
                    continue;
                };
                let Ok(font) = load_single_font(file, font_dict) else {
                    continue;
                };
                let synthetic_key = format!("__gs:{gs_name}");
                fonts.insert(synthetic_key.clone(), font);
                extgstate_fonts.insert(gs_name.clone(), (synthetic_key, font_size));
            }
        }
    }

    Ok((fonts, extgstate_fonts))
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
                source_form: context.current_form,
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
        text_state.text_matrix =
            Matrix::translate(advance, 0.0).multiply(text_state.text_matrix);
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

fn decode_simple_byte(font: &SimpleFont, byte: u8) -> String {
    if let Some(mapped) = font.unicode_map.get(&u16::from(byte)) {
        if !mapped.is_empty() {
            return mapped.clone();
        }
    }
    if let Some(glyph_name) = font.encoding.differences.get(&byte) {
        if let Some(character) = glyph_name_to_char(glyph_name) {
            return character.to_string();
        }
        return '\u{FFFD}'.to_string();
    }
    let character = match font.encoding.base {
        SimpleEncodingBase::WinAnsi => winansi_byte_to_char(byte),
        SimpleEncodingBase::Identity => identity_byte_to_char(byte),
    };
    character
        .map(|character| character.to_string())
        .unwrap_or_else(|| '\u{FFFD}'.to_string())
}

fn identity_byte_to_char(byte: u8) -> Option<char> {
    if byte.is_ascii() {
        Some(byte as char)
    } else {
        None
    }
}

/// Minimal Adobe Glyph List subset: maps PDF glyph names commonly appearing
/// in `/Encoding` `Differences` arrays to their Unicode equivalents. Covers
/// the WinAnsi repertoire plus common additions (Latin supplement, Latin
/// Extended-A, a few math and punctuation glyphs). Names not present here
/// fall through to `U+FFFD` — the caller handles that.
fn glyph_name_to_char(name: &str) -> Option<char> {
    let mapped = match name {
        "space" => ' ',
        "exclam" => '!',
        "quotedbl" => '"',
        "numbersign" => '#',
        "dollar" => '$',
        "percent" => '%',
        "ampersand" => '&',
        "quotesingle" => '\'',
        "parenleft" => '(',
        "parenright" => ')',
        "asterisk" => '*',
        "plus" => '+',
        "comma" => ',',
        "hyphen" => '-',
        "period" => '.',
        "slash" => '/',
        "zero" => '0',
        "one" => '1',
        "two" => '2',
        "three" => '3',
        "four" => '4',
        "five" => '5',
        "six" => '6',
        "seven" => '7',
        "eight" => '8',
        "nine" => '9',
        "colon" => ':',
        "semicolon" => ';',
        "less" => '<',
        "equal" => '=',
        "greater" => '>',
        "question" => '?',
        "at" => '@',
        "bracketleft" => '[',
        "backslash" => '\\',
        "bracketright" => ']',
        "asciicircum" => '^',
        "underscore" => '_',
        "grave" => '`',
        "braceleft" => '{',
        "bar" => '|',
        "braceright" => '}',
        "asciitilde" => '~',
        // Common uppercase Latin
        name if name.len() == 1 && name.chars().next().unwrap().is_ascii_alphabetic() => {
            name.chars().next().unwrap()
        }
        // Windows-1252 / WinAnsi punctuation
        "bullet" => '\u{2022}',
        "Euro" => '\u{20AC}',
        "endash" => '\u{2013}',
        "emdash" => '\u{2014}',
        "quoteleft" => '\u{2018}',
        "quoteright" => '\u{2019}',
        "quotesinglbase" => '\u{201A}',
        "quotedblleft" => '\u{201C}',
        "quotedblright" => '\u{201D}',
        "quotedblbase" => '\u{201E}',
        "ellipsis" => '\u{2026}',
        "dagger" => '\u{2020}',
        "daggerdbl" => '\u{2021}',
        "perthousand" => '\u{2030}',
        "guilsinglleft" => '\u{2039}',
        "guilsinglright" => '\u{203A}',
        "florin" => '\u{0192}',
        "trademark" => '\u{2122}',
        "circumflex" => '\u{02C6}',
        "tilde" => '\u{02DC}',
        "macron" => '\u{00AF}',
        "breve" => '\u{02D8}',
        "dotaccent" => '\u{02D9}',
        "ring" => '\u{02DA}',
        "ogonek" => '\u{02DB}',
        "caron" => '\u{02C7}',
        "cedilla" => '\u{00B8}',
        "dieresis" => '\u{00A8}',
        "acute" => '\u{00B4}',
        // Latin-1 supplement
        "exclamdown" => '\u{00A1}',
        "cent" => '\u{00A2}',
        "sterling" => '\u{00A3}',
        "currency" => '\u{00A4}',
        "yen" => '\u{00A5}',
        "brokenbar" => '\u{00A6}',
        "section" => '\u{00A7}',
        "copyright" => '\u{00A9}',
        "ordfeminine" => '\u{00AA}',
        "guillemotleft" => '\u{00AB}',
        "logicalnot" => '\u{00AC}',
        "registered" => '\u{00AE}',
        "degree" => '\u{00B0}',
        "plusminus" => '\u{00B1}',
        "twosuperior" => '\u{00B2}',
        "threesuperior" => '\u{00B3}',
        "mu" => '\u{00B5}',
        "paragraph" => '\u{00B6}',
        "periodcentered" => '\u{00B7}',
        "onesuperior" => '\u{00B9}',
        "ordmasculine" => '\u{00BA}',
        "guillemotright" => '\u{00BB}',
        "onequarter" => '\u{00BC}',
        "onehalf" => '\u{00BD}',
        "threequarters" => '\u{00BE}',
        "questiondown" => '\u{00BF}',
        // Latin-1 letters
        "Agrave" => '\u{00C0}',
        "Aacute" => '\u{00C1}',
        "Acircumflex" => '\u{00C2}',
        "Atilde" => '\u{00C3}',
        "Adieresis" => '\u{00C4}',
        "Aring" => '\u{00C5}',
        "AE" => '\u{00C6}',
        "Ccedilla" => '\u{00C7}',
        "Egrave" => '\u{00C8}',
        "Eacute" => '\u{00C9}',
        "Ecircumflex" => '\u{00CA}',
        "Edieresis" => '\u{00CB}',
        "Igrave" => '\u{00CC}',
        "Iacute" => '\u{00CD}',
        "Icircumflex" => '\u{00CE}',
        "Idieresis" => '\u{00CF}',
        "Eth" => '\u{00D0}',
        "Ntilde" => '\u{00D1}',
        "Ograve" => '\u{00D2}',
        "Oacute" => '\u{00D3}',
        "Ocircumflex" => '\u{00D4}',
        "Otilde" => '\u{00D5}',
        "Odieresis" => '\u{00D6}',
        "multiply" => '\u{00D7}',
        "Oslash" => '\u{00D8}',
        "Ugrave" => '\u{00D9}',
        "Uacute" => '\u{00DA}',
        "Ucircumflex" => '\u{00DB}',
        "Udieresis" => '\u{00DC}',
        "Yacute" => '\u{00DD}',
        "Thorn" => '\u{00DE}',
        "germandbls" => '\u{00DF}',
        "agrave" => '\u{00E0}',
        "aacute" => '\u{00E1}',
        "acircumflex" => '\u{00E2}',
        "atilde" => '\u{00E3}',
        "adieresis" => '\u{00E4}',
        "aring" => '\u{00E5}',
        "ae" => '\u{00E6}',
        "ccedilla" => '\u{00E7}',
        "egrave" => '\u{00E8}',
        "eacute" => '\u{00E9}',
        "ecircumflex" => '\u{00EA}',
        "edieresis" => '\u{00EB}',
        "igrave" => '\u{00EC}',
        "iacute" => '\u{00ED}',
        "icircumflex" => '\u{00EE}',
        "idieresis" => '\u{00EF}',
        "eth" => '\u{00F0}',
        "ntilde" => '\u{00F1}',
        "ograve" => '\u{00F2}',
        "oacute" => '\u{00F3}',
        "ocircumflex" => '\u{00F4}',
        "otilde" => '\u{00F5}',
        "odieresis" => '\u{00F6}',
        "divide" => '\u{00F7}',
        "oslash" => '\u{00F8}',
        "ugrave" => '\u{00F9}',
        "uacute" => '\u{00FA}',
        "ucircumflex" => '\u{00FB}',
        "udieresis" => '\u{00FC}',
        "yacute" => '\u{00FD}',
        "thorn" => '\u{00FE}',
        "ydieresis" => '\u{00FF}',
        // Latin Extended-A common entries
        "OE" => '\u{0152}',
        "oe" => '\u{0153}',
        "Scaron" => '\u{0160}',
        "scaron" => '\u{0161}',
        "Ydieresis" => '\u{0178}',
        "Zcaron" => '\u{017D}',
        "zcaron" => '\u{017E}',
        "Lslash" => '\u{0141}',
        "lslash" => '\u{0142}',
        "Idot" | "Idotaccent" => '\u{0130}',
        "dotlessi" => '\u{0131}',
        "fi" => '\u{FB01}',
        "fl" => '\u{FB02}',
        "ff" => '\u{FB00}',
        "ffi" => '\u{FB03}',
        "ffl" => '\u{FB04}',
        _ => return None,
    };
    Some(mapped)
}

/// Decode a single byte under the PDF `WinAnsiEncoding` table.
///
/// Codes `0x20..=0x7E` are plain ASCII. Codes `0x80..=0x9F` carry the
/// Windows-1252 punctuation repertoire (smart quotes, the Euro sign, the
/// em/en dashes, and so on). Codes `0xA0..=0xFF` are ISO-8859-1.
/// Undefined codes return `None`, which callers render as `U+FFFD`.
fn winansi_byte_to_char(byte: u8) -> Option<char> {
    match byte {
        0x20..=0x7E => Some(byte as char),
        0x80 => Some('\u{20AC}'),
        0x82 => Some('\u{201A}'),
        0x83 => Some('\u{0192}'),
        0x84 => Some('\u{201E}'),
        0x85 => Some('\u{2026}'),
        0x86 => Some('\u{2020}'),
        0x87 => Some('\u{2021}'),
        0x88 => Some('\u{02C6}'),
        0x89 => Some('\u{2030}'),
        0x8A => Some('\u{0160}'),
        0x8B => Some('\u{2039}'),
        0x8C => Some('\u{0152}'),
        0x8E => Some('\u{017D}'),
        0x91 => Some('\u{2018}'),
        0x92 => Some('\u{2019}'),
        0x93 => Some('\u{201C}'),
        0x94 => Some('\u{201D}'),
        0x95 => Some('\u{2022}'),
        0x96 => Some('\u{2013}'),
        0x97 => Some('\u{2014}'),
        0x98 => Some('\u{02DC}'),
        0x99 => Some('\u{2122}'),
        0x9A => Some('\u{0161}'),
        0x9B => Some('\u{203A}'),
        0x9C => Some('\u{0153}'),
        0x9E => Some('\u{017E}'),
        0x9F => Some('\u{0178}'),
        0xA0..=0xFF => Some(byte as char),
        _ => None,
    }
}

fn parse_simple_encoding(file: &PdfFile, font_dict: &pdf_objects::PdfDictionary) -> SimpleEncoding {
    match font_dict.get("Encoding") {
        Some(PdfValue::Name(name)) => SimpleEncoding {
            base: simple_encoding_base_from_name(name),
            differences: BTreeMap::new(),
        },
        Some(value @ PdfValue::Reference(_)) | Some(value @ PdfValue::Dictionary(_)) => {
            match file.resolve_dict(value) {
                Ok(dict) => {
                    let base = dict
                        .get("BaseEncoding")
                        .and_then(PdfValue::as_name)
                        .map(simple_encoding_base_from_name)
                        .unwrap_or_default();
                    let differences = dict
                        .get("Differences")
                        .and_then(PdfValue::as_array)
                        .map(parse_differences_array)
                        .unwrap_or_default();
                    SimpleEncoding { base, differences }
                }
                Err(_) => SimpleEncoding::default(),
            }
        }
        _ => SimpleEncoding::default(),
    }
}

fn simple_encoding_base_from_name(name: &str) -> SimpleEncodingBase {
    match name {
        "WinAnsiEncoding" => SimpleEncodingBase::WinAnsi,
        _ => SimpleEncodingBase::Identity,
    }
}

fn parse_differences_array(entries: &[PdfValue]) -> BTreeMap<u8, String> {
    let mut output = BTreeMap::new();
    let mut code: u16 = 0;
    let mut have_code = false;
    for entry in entries {
        match entry {
            PdfValue::Integer(value) => {
                if *value >= 0 && *value <= 255 {
                    code = *value as u16;
                    have_code = true;
                } else {
                    have_code = false;
                }
            }
            PdfValue::Number(value) => {
                let rounded = value.round() as i64;
                if (0..=255).contains(&rounded) {
                    code = rounded as u16;
                    have_code = true;
                } else {
                    have_code = false;
                }
            }
            PdfValue::Name(name) => {
                if have_code && code <= 255 {
                    output.insert(code as u8, name.clone());
                    code += 1;
                    if code > 255 {
                        have_code = false;
                    }
                }
            }
            _ => {}
        }
    }
    output
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
                    text: decode_simple_byte(font, byte),
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
                let cid = start_cid
                    .checked_add(offset as u16)
                    .ok_or_else(|| PdfError::Corrupt("CID width index overflow".to_string()))?;
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
    mapping.insert(
        parse_cid_token(&tokens[0])?,
        decode_utf16be_lossy(&tokens[1]),
    );
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
    let filtered = token
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
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
        char::from_u32(u32::from(cid))
            .unwrap_or('\u{FFFD}')
            .to_string()
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
    use super::{
        ExtractedPageText, Glyph, build_search_index, coalesce_match_quads, search_page_text,
    };
    use pdf_graphics::{Point, Quad, Rect};

    fn make_glyph(text: char, x: f64, y: f64) -> Glyph {
        let rect = Rect {
            x,
            y,
            width: 8.0,
            height: 12.0,
        };
        Glyph {
            text,
            bbox: rect,
            quad: rect.to_quad(),
            page_char_index: 0,
            operation_index: 0,
            location: super::GlyphLocation::Direct {
                operand_index: 0,
                byte_start: 0,
                byte_end: 1,
            },
            visible: true,
            width_units: 600.0,
            source_form: None,
        }
    }

    #[test]
    fn search_handles_multibyte_utf8_characters() {
        // Text: "café and tea" — 'é' is 2 bytes in UTF-8, which used to cause
        // byte-vs-char offset mismatch in normalized_to_display.
        let glyphs: Vec<Glyph> = "café and tea"
            .chars()
            .enumerate()
            .map(|(i, c)| make_glyph(c, i as f64 * 10.0, 100.0))
            .collect();
        let page = ExtractedPageText {
            page_index: 0,
            text: "café and tea".to_string(),
            items: Vec::new(),
            glyphs,
        };
        let matches = search_page_text(&page, "and");
        assert_eq!(matches.len(), 1, "should find exactly one 'and'");
        assert_eq!(
            matches[0].text, "and",
            "match text should be 'and', not a shifted substring"
        );
    }

    #[test]
    fn search_index_byte_alignment() {
        // Verify that normalized_to_display has exactly one entry per byte
        let glyphs: Vec<Glyph> = "aé b"
            .chars()
            .enumerate()
            .map(|(i, c)| make_glyph(c, i as f64 * 10.0, 100.0))
            .collect();
        let page = ExtractedPageText {
            page_index: 0,
            text: "aé b".to_string(),
            items: Vec::new(),
            glyphs,
        };
        let index = build_search_index(&page);
        assert_eq!(
            index.normalized_to_display.len(),
            index.normalized_text.len(),
            "normalized_to_display should have one entry per byte of normalized_text"
        );
    }

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
