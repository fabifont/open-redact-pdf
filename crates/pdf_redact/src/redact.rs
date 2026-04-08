use std::collections::{BTreeMap, BTreeSet};

use pdf_content::{Operation, PaintOperator, PathSegment, parse_page_contents};
use pdf_graphics::{Color, Matrix, Point, Rect};
use pdf_objects::{
    PageInfo, PdfDictionary, PdfError, PdfFile, PdfObject, PdfResult, PdfStream, PdfString,
    PdfValue, serialize_value,
};
use pdf_targets::{NormalizedPageTarget, NormalizedRedactionPlan};
use pdf_text::{Glyph, GlyphLocation, analyze_page_text};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApplyReport {
    pub pages_touched: usize,
    pub text_glyphs_removed: usize,
    pub path_paints_removed: usize,
    pub image_draws_removed: usize,
    pub annotations_removed: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PageRedactionPlan {
    pub page_index: usize,
    pub target_count: usize,
}

pub fn apply_redactions(
    file: &mut PdfFile,
    pages: &mut [PageInfo],
    plan: &NormalizedRedactionPlan,
) -> PdfResult<ApplyReport> {
    let mut report = ApplyReport::default();
    let page_targets = group_targets(plan);

    if plan.strip_metadata {
        strip_metadata(file)?;
    }
    if plan.strip_attachments {
        strip_attachments(file)?;
    }

    for (page_index, targets) in page_targets {
        let page = pages
            .get(page_index)
            .cloned()
            .ok_or(PdfError::InvalidPageIndex(page_index))?;
        let page_transform = page.page_box.normalized_transform();
        let extracted = analyze_page_text(file, page_index, &page)?;
        let parsed = parse_page_contents(file, &page)?;
        ensure_supported_operators(&parsed.operations)?;
        let xobjects = load_xobjects(file, &page.resources)?;
        let glyph_removals = collect_glyph_removals(&extracted.glyphs, &targets);
        let mut operations = parsed.operations.clone();
        let text_removed = count_removed_glyphs(&glyph_removals);
        rewrite_text_operations(&mut operations, &glyph_removals);
        report.text_glyphs_removed += text_removed;

        let vector_removed =
            neutralize_vector_operations(&mut operations, &targets, page_transform)?;
        report.path_paints_removed += vector_removed;

        let image_removed =
            neutralize_image_operations(&mut operations, &targets, page_transform, &xobjects)?;
        report.image_draws_removed += image_removed;
        if image_removed > 0 {
            report.warnings.push(format!(
                "page {page_index}: intersecting images were removed at invocation level"
            ));
        }

        let annotation_removed = if plan.remove_intersecting_annotations {
            remove_annotations(file, &page, &targets, page_transform)?
        } else {
            0
        };
        report.annotations_removed += annotation_removed;

        let overlay = overlay_stream_bytes(&targets, plan.fill_color);
        let mut content_bytes = serialize_operations(&operations);
        content_bytes.extend_from_slice(&overlay);
        write_page_contents(file, pages, page_index, content_bytes)?;
        report.pages_touched += 1;
    }

    Ok(report)
}

fn group_targets(plan: &NormalizedRedactionPlan) -> BTreeMap<usize, Vec<&NormalizedPageTarget>> {
    let mut pages = BTreeMap::new();
    for target in &plan.targets {
        pages
            .entry(target.page_index)
            .or_insert_with(Vec::new)
            .push(target);
    }
    pages
}

fn strip_metadata(file: &mut PdfFile) -> PdfResult<()> {
    file.trailer.remove("Info");
    let root_ref = match file.trailer.get("Root") {
        Some(PdfValue::Reference(object_ref)) => *object_ref,
        _ => return Err(PdfError::Corrupt("trailer Root is missing".to_string())),
    };
    if let PdfObject::Value(PdfValue::Dictionary(dictionary)) = file.get_object_mut(root_ref)? {
        dictionary.remove("Metadata");
    }
    Ok(())
}

fn strip_attachments(file: &mut PdfFile) -> PdfResult<()> {
    let root_ref = match file.trailer.get("Root") {
        Some(PdfValue::Reference(object_ref)) => *object_ref,
        _ => return Err(PdfError::Corrupt("trailer Root is missing".to_string())),
    };
    let names_ref = match file.get_dictionary(root_ref)?.get("Names") {
        Some(PdfValue::Reference(names_ref)) => Some(*names_ref),
        _ => None,
    };
    if let PdfObject::Value(PdfValue::Dictionary(dictionary)) = file.get_object_mut(root_ref)? {
        let _ = dictionary;
    }
    if let Some(names_ref) = names_ref {
        if let PdfObject::Value(PdfValue::Dictionary(names)) = file.get_object_mut(names_ref)? {
            names.remove("EmbeddedFiles");
        }
    }
    Ok(())
}

fn ensure_supported_operators(operations: &[Operation]) -> PdfResult<()> {
    for operation in operations {
        let supported = matches!(
            operation.operator.as_str(),
            "q" | "Q"
                | "cm"
                | "BT"
                | "ET"
                | "Tf"
                | "Tm"
                | "Td"
                | "TD"
                | "T*"
                | "Tc"
                | "Tw"
                | "TL"
                | "Ts"
                | "Tz"
                | "Tr"
                | "Tj"
                | "TJ"
                | "'"
                | "\""
                | "m"
                | "l"
                | "c"
                | "h"
                | "re"
                | "S"
                | "s"
                | "f"
                | "F"
                | "f*"
                | "B"
                | "B*"
                | "b"
                | "b*"
                | "n"
                | "w"
                | "rg"
                | "RG"
                | "g"
                | "G"
                | "Do"
        );
        if !supported {
            return Err(PdfError::Unsupported(format!(
                "operator {} is not supported on redacted pages",
                operation.operator
            )));
        }
    }
    Ok(())
}

fn load_xobjects(
    file: &PdfFile,
    resources: &PdfDictionary,
) -> PdfResult<BTreeMap<String, XObjectKind>> {
    let mut output = BTreeMap::new();
    let Some(xobjects_value) = resources.get("XObject") else {
        return Ok(output);
    };
    let xobjects = file.resolve_dict(xobjects_value)?;
    for (name, value) in xobjects {
        let object_ref = match value {
            PdfValue::Reference(object_ref) => *object_ref,
            _ => {
                return Err(PdfError::Unsupported(
                    "direct XObjects are not supported".to_string(),
                ));
            }
        };
        let stream = match file.get_object(object_ref)? {
            PdfObject::Stream(stream) => stream,
            _ => {
                return Err(PdfError::Corrupt(
                    "XObject reference does not point to a stream".to_string(),
                ));
            }
        };
        let subtype = stream
            .dict
            .get("Subtype")
            .and_then(PdfValue::as_name)
            .unwrap_or("");
        let kind = match subtype {
            "Image" => XObjectKind::Image,
            "Form" => XObjectKind::Form,
            other => {
                return Err(PdfError::Unsupported(format!(
                    "XObject subtype {other} is not supported"
                )));
            }
        };
        output.insert(name.clone(), kind);
    }
    Ok(output)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XObjectKind {
    Image,
    Form,
}

type GlyphRemovalMap = BTreeMap<usize, Vec<GlyphByteRef>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct GlyphByteRef {
    operand_index: usize,
    element_index: Option<usize>,
    byte_start: usize,
    byte_end: usize,
}

fn collect_glyph_removals(glyphs: &[Glyph], targets: &[&NormalizedPageTarget]) -> GlyphRemovalMap {
    let mut output: GlyphRemovalMap = BTreeMap::new();
    for glyph in glyphs {
        if targets
            .iter()
            .any(|target| target.intersects_quad(&glyph.quad))
        {
            let entry = output.entry(glyph.operation_index).or_default();
            match glyph.location {
                GlyphLocation::Direct {
                    operand_index,
                    byte_start,
                    byte_end,
                } => entry.push(GlyphByteRef {
                    operand_index,
                    element_index: None,
                    byte_start,
                    byte_end,
                }),
                GlyphLocation::Array {
                    operand_index,
                    element_index,
                    byte_start,
                    byte_end,
                } => entry.push(GlyphByteRef {
                    operand_index,
                    element_index: Some(element_index),
                    byte_start,
                    byte_end,
                }),
            }
        }
    }
    output
}

fn rewrite_text_operations(operations: &mut [Operation], removals: &GlyphRemovalMap) -> usize {
    let mut removed = 0usize;
    for (operation_index, refs) in removals {
        let Some(operation) = operations.get_mut(*operation_index) else {
            continue;
        };
        let direct_groups = refs
            .iter()
            .filter(|entry| entry.element_index.is_none())
            .fold(
                BTreeMap::<usize, BTreeSet<usize>>::new(),
                |mut map, entry| {
                    let bytes = map.entry(entry.operand_index).or_default();
                    for byte_index in entry.byte_start..entry.byte_end {
                        bytes.insert(byte_index);
                    }
                    map
                },
            );
        for (operand_index, bytes) in direct_groups {
            if let Some(PdfValue::String(string)) = operation.operands.get_mut(operand_index) {
                removed += remove_bytes_from_string(string, &bytes);
            }
        }

        let array_groups = refs
            .iter()
            .filter_map(|entry| {
                entry
                    .element_index
                    .map(|element_index| {
                        (
                            entry.operand_index,
                            element_index,
                            entry.byte_start,
                            entry.byte_end,
                        )
                    })
            })
            .fold(
                BTreeMap::<(usize, usize), BTreeSet<usize>>::new(),
                |mut map, (operand_index, element_index, byte_start, byte_end)| {
                    let bytes = map.entry((operand_index, element_index)).or_default();
                    for byte_index in byte_start..byte_end {
                        bytes.insert(byte_index);
                    }
                    map
                },
            );
        for ((operand_index, element_index), bytes) in array_groups {
            if let Some(PdfValue::Array(items)) = operation.operands.get_mut(operand_index) {
                if let Some(PdfValue::String(string)) = items.get_mut(element_index) {
                    removed += remove_bytes_from_string(string, &bytes);
                }
            }
        }
    }
    removed
}

fn count_removed_glyphs(removals: &GlyphRemovalMap) -> usize {
    removals
        .values()
        .map(|entries| entries.iter().copied().collect::<BTreeSet<_>>().len())
        .sum()
}

fn remove_bytes_from_string(string: &mut PdfString, bytes: &BTreeSet<usize>) -> usize {
    let original = string.0.len();
    string.0 = string
        .0
        .iter()
        .copied()
        .enumerate()
        .filter_map(|(index, byte)| (!bytes.contains(&index)).then_some(byte))
        .collect();
    original.saturating_sub(string.0.len())
}

fn neutralize_vector_operations(
    operations: &mut [Operation],
    targets: &[&NormalizedPageTarget],
    page_transform: Matrix,
) -> PdfResult<usize> {
    let mut removed = 0usize;
    let mut ctm = Matrix::identity();
    let mut ctm_stack = Vec::new();
    let mut stroke_width = 1.0f64;
    let mut path_segments = Vec::<PathSegment>::new();

    for operation in operations.iter_mut() {
        match operation.operator.as_str() {
            "q" => ctm_stack.push((ctm, stroke_width)),
            "Q" => {
                if let Some((saved_ctm, saved_width)) = ctm_stack.pop() {
                    ctm = saved_ctm;
                    stroke_width = saved_width;
                }
            }
            "cm" => ctm = ctm.multiply(matrix_from_operands(&operation.operands)?),
            "w" => stroke_width = operand_number(operation, 0)?,
            "m" => path_segments.push(PathSegment::MoveTo(transform_point_pair(
                &operation.operands,
                0,
                ctm,
                page_transform,
            )?)),
            "l" => path_segments.push(PathSegment::LineTo(transform_point_pair(
                &operation.operands,
                0,
                ctm,
                page_transform,
            )?)),
            "c" => {
                let first = transform_point_pair(&operation.operands, 0, ctm, page_transform)?;
                let second = transform_point_pair(&operation.operands, 2, ctm, page_transform)?;
                let third = transform_point_pair(&operation.operands, 4, ctm, page_transform)?;
                path_segments.push(PathSegment::CurveTo(first, second, third));
            }
            "h" => path_segments.push(PathSegment::ClosePath),
            "re" => {
                let rect = Rect {
                    x: operand_number(operation, 0)?,
                    y: operand_number(operation, 1)?,
                    width: operand_number(operation, 2)?,
                    height: operand_number(operation, 3)?,
                }
                .normalize();
                let transformed = ctm.transform_rect(rect);
                path_segments.push(PathSegment::Rect(
                    page_transform.transform_rect(transformed),
                ));
            }
            operator if PaintOperator::from_operator(operator).is_some() => {
                if PaintOperator::from_operator(operator) != Some(PaintOperator::NoPaint) {
                    if let Some(bounds) = path_bounds(&path_segments, stroke_width) {
                        if targets.iter().any(|target| target.intersects_rect(&bounds)) {
                            operation.operator = "n".to_string();
                            operation.operands.clear();
                            removed += 1;
                        }
                    }
                }
                path_segments.clear();
            }
            _ => {}
        }
    }

    Ok(removed)
}

fn neutralize_image_operations(
    operations: &mut [Operation],
    targets: &[&NormalizedPageTarget],
    page_transform: Matrix,
    xobjects: &BTreeMap<String, XObjectKind>,
) -> PdfResult<usize> {
    let mut removed = 0usize;
    let mut ctm = Matrix::identity();
    let mut ctm_stack = Vec::new();
    for operation in operations.iter_mut() {
        match operation.operator.as_str() {
            "q" => ctm_stack.push(ctm),
            "Q" => ctm = ctm_stack.pop().unwrap_or(Matrix::identity()),
            "cm" => ctm = ctm.multiply(matrix_from_operands(&operation.operands)?),
            "Do" => {
                let name = operand_name(operation, 0)?;
                match xobjects.get(name) {
                    Some(XObjectKind::Image) => {
                        let quad = Rect {
                            x: 0.0,
                            y: 0.0,
                            width: 1.0,
                            height: 1.0,
                        }
                        .to_quad()
                        .transform(ctm.multiply(page_transform));
                        if targets.iter().any(|target| target.intersects_quad(&quad)) {
                            operation.operator = "n".to_string();
                            operation.operands.clear();
                            removed += 1;
                        }
                    }
                    Some(XObjectKind::Form) => {
                        return Err(PdfError::Unsupported(
                            "Form XObjects are not supported on redacted pages".to_string(),
                        ));
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }
    Ok(removed)
}

fn remove_annotations(
    file: &mut PdfFile,
    page: &PageInfo,
    targets: &[&NormalizedPageTarget],
    page_transform: Matrix,
) -> PdfResult<usize> {
    let mut removed = 0usize;
    let mut retained = Vec::new();
    for annotation_ref in &page.annotation_refs {
        let dict = file.get_dictionary(*annotation_ref)?;
        let rect = dict
            .get("Rect")
            .map(parse_rect)
            .transpose()?
            .map(|rect| page_transform.transform_rect(rect));
        let intersects = rect
            .map(|rect| targets.iter().any(|target| target.intersects_rect(&rect)))
            .unwrap_or(false);
        let subtype = dict
            .get("Subtype")
            .and_then(PdfValue::as_name)
            .unwrap_or("");
        let is_attachment = subtype == "FileAttachment";
        if intersects || is_attachment {
            removed += 1;
        } else {
            retained.push(PdfValue::Reference(*annotation_ref));
        }
    }

    if removed > 0 {
        if let PdfObject::Value(PdfValue::Dictionary(dictionary)) =
            file.get_object_mut(page.page_ref)?
        {
            if retained.is_empty() {
                dictionary.remove("Annots");
            } else {
                dictionary.insert("Annots".to_string(), PdfValue::Array(retained));
            }
        }
    }

    Ok(removed)
}

fn write_page_contents(
    file: &mut PdfFile,
    pages: &mut [PageInfo],
    page_index: usize,
    bytes: Vec<u8>,
) -> PdfResult<()> {
    let page = pages
        .get_mut(page_index)
        .ok_or(PdfError::InvalidPageIndex(page_index))?;
    let content_ref = file.allocate_object_ref();
    file.insert_object(
        content_ref,
        PdfObject::Stream(PdfStream {
            dict: PdfDictionary::new(),
            data: bytes,
        }),
    );
    if let PdfObject::Value(PdfValue::Dictionary(dictionary)) =
        file.get_object_mut(page.page_ref)?
    {
        dictionary.insert("Contents".to_string(), PdfValue::Reference(content_ref));
    }
    page.content_refs = vec![content_ref];
    Ok(())
}

fn overlay_stream_bytes(targets: &[&NormalizedPageTarget], color: Color) -> Vec<u8> {
    let red = f64::from(color.r) / 255.0;
    let green = f64::from(color.g) / 255.0;
    let blue = f64::from(color.b) / 255.0;
    let mut output = String::new();
    output.push_str("q\n");
    output.push_str(&format!("{red:.3} {green:.3} {blue:.3} rg\n"));
    for target in targets {
        for quad in &target.quads {
            let [a, b, c, d] = quad.points;
            output.push_str(&format!(
                "{} {} m\n{} {} l\n{} {} l\n{} {} l\nh\nf\n",
                format_number(a.x),
                format_number(a.y),
                format_number(b.x),
                format_number(b.y),
                format_number(c.x),
                format_number(c.y),
                format_number(d.x),
                format_number(d.y)
            ));
        }
    }
    output.push_str("Q\n");
    output.into_bytes()
}

fn serialize_operations(operations: &[Operation]) -> Vec<u8> {
    let mut output = String::new();
    for operation in operations {
        for operand in &operation.operands {
            output.push_str(&serialize_value(operand));
            output.push(' ');
        }
        output.push_str(&operation.operator);
        output.push('\n');
    }
    output.into_bytes()
}

fn path_bounds(path_segments: &[PathSegment], stroke_width: f64) -> Option<Rect> {
    let mut bounds: Option<Rect> = None;
    for segment in path_segments {
        let rect = match segment {
            PathSegment::MoveTo(point) | PathSegment::LineTo(point) => Rect {
                x: point.x,
                y: point.y,
                width: 0.0,
                height: 0.0,
            },
            PathSegment::CurveTo(first, second, third) => {
                let min_x = first.x.min(second.x).min(third.x);
                let min_y = first.y.min(second.y).min(third.y);
                let max_x = first.x.max(second.x).max(third.x);
                let max_y = first.y.max(second.y).max(third.y);
                Rect {
                    x: min_x,
                    y: min_y,
                    width: max_x - min_x,
                    height: max_y - min_y,
                }
            }
            PathSegment::Rect(rect) => *rect,
            PathSegment::ClosePath => continue,
        };
        bounds = Some(match bounds {
            Some(existing) => existing.union(&rect),
            None => rect,
        });
    }
    bounds.map(|rect| Rect {
        x: rect.x - stroke_width / 2.0,
        y: rect.y - stroke_width / 2.0,
        width: rect.width + stroke_width,
        height: rect.height + stroke_width,
    })
}

fn transform_point_pair(
    operands: &[PdfValue],
    start: usize,
    ctm: Matrix,
    page_transform: Matrix,
) -> PdfResult<Point> {
    let x = operands
        .get(start)
        .and_then(PdfValue::as_number)
        .ok_or_else(|| PdfError::Corrupt("path operand is not numeric".to_string()))?;
    let y = operands
        .get(start + 1)
        .and_then(PdfValue::as_number)
        .ok_or_else(|| PdfError::Corrupt("path operand is not numeric".to_string()))?;
    Ok(page_transform.transform_point(ctm.transform_point(Point { x, y })))
}

fn matrix_from_operands(operands: &[PdfValue]) -> PdfResult<Matrix> {
    if operands.len() != 6 {
        return Err(PdfError::Corrupt("cm expects six operands".to_string()));
    }
    Ok(Matrix {
        a: operands[0]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("cm operand is not numeric".to_string()))?,
        b: operands[1]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("cm operand is not numeric".to_string()))?,
        c: operands[2]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("cm operand is not numeric".to_string()))?,
        d: operands[3]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("cm operand is not numeric".to_string()))?,
        e: operands[4]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("cm operand is not numeric".to_string()))?,
        f: operands[5]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("cm operand is not numeric".to_string()))?,
    })
}

fn operand_number(operation: &Operation, index: usize) -> PdfResult<f64> {
    operation
        .operands
        .get(index)
        .and_then(PdfValue::as_number)
        .ok_or_else(|| PdfError::Corrupt(format!("operand {index} is not numeric")))
}

fn operand_name(operation: &Operation, index: usize) -> PdfResult<&str> {
    operation
        .operands
        .get(index)
        .and_then(PdfValue::as_name)
        .ok_or_else(|| PdfError::Corrupt(format!("operand {index} is not a name")))
}

fn parse_rect(value: &PdfValue) -> PdfResult<Rect> {
    let array = value
        .as_array()
        .ok_or_else(|| PdfError::Corrupt("annotation Rect is not an array".to_string()))?;
    if array.len() != 4 {
        return Err(PdfError::Corrupt(
            "annotation Rect must contain four numbers".to_string(),
        ));
    }
    Ok(Rect {
        x: array[0]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("annotation rect is invalid".to_string()))?,
        y: array[1]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("annotation rect is invalid".to_string()))?,
        width: array[2]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("annotation rect is invalid".to_string()))?
            - array[0]
                .as_number()
                .ok_or_else(|| PdfError::Corrupt("annotation rect is invalid".to_string()))?,
        height: array[3]
            .as_number()
            .ok_or_else(|| PdfError::Corrupt("annotation rect is invalid".to_string()))?
            - array[1]
                .as_number()
                .ok_or_else(|| PdfError::Corrupt("annotation rect is invalid".to_string()))?,
    }
    .normalize())
}

fn format_number(value: f64) -> String {
    if value.fract().abs() < 1e-6 {
        format!("{:.0}", value)
    } else {
        let mut string = format!("{value:.4}");
        while string.ends_with('0') {
            string.pop();
        }
        if string.ends_with('.') {
            string.pop();
        }
        string
    }
}
