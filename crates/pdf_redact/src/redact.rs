use std::collections::{BTreeMap, BTreeSet};

use pdf_content::{Operation, PaintOperator, PathSegment, parse_page_contents};
use pdf_graphics::{Color, Matrix, Point, Rect};
use pdf_objects::{
    ObjectRef, PageInfo, PdfDictionary, PdfError, PdfFile, PdfObject, PdfResult, PdfStream,
    PdfString, PdfValue, flate_encode, serialize_value,
};
use pdf_targets::{NormalizedPageTarget, NormalizedRedactionPlan, RedactionMode};
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

    reject_hidden_optional_content(file)?;

    if plan.strip_metadata {
        strip_metadata(file)?;
    }
    if plan.strip_attachments {
        strip_attachments(file)?;
    }

    // Collect object refs to remove AFTER the loop so shared objects (e.g. a
    // logo XObject used on every page) remain available while processing all
    // pages.
    let mut deferred_xobject_removals: Vec<(PdfDictionary, Vec<String>)> = Vec::new();
    let mut deferred_content_removals: Vec<ObjectRef> = Vec::new();

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
        rewrite_text_operations(
            &mut operations,
            &glyph_removals,
            plan.mode,
            &mut report.warnings,
        );
        report.text_glyphs_removed += text_removed;

        let vector_removed =
            neutralize_vector_operations(&mut operations, &targets, page_transform)?;
        report.path_paints_removed += vector_removed;

        let (image_removed, neutralized_xobjects) =
            neutralize_image_operations(&mut operations, &targets, page_transform, &xobjects)?;
        report.image_draws_removed += image_removed;
        if image_removed > 0 {
            report.warnings.push(format!(
                "page {page_index}: intersecting images were removed at invocation level"
            ));
        }

        // Defer XObject removal so shared objects stay available for other pages
        if !neutralized_xobjects.is_empty() {
            deferred_xobject_removals.push((page.resources.clone(), neutralized_xobjects));
        }

        let annotation_removed = if plan.remove_intersecting_annotations {
            remove_annotations(file, &page, &targets, page_transform)?
        } else {
            0
        };
        report.annotations_removed += annotation_removed;

        let mut content_bytes = serialize_operations(&operations);
        if plan.mode == RedactionMode::Redact {
            let overlay = overlay_stream_bytes(
                &targets,
                plan.fill_color,
                page.page_box.normalized_transform(),
                final_page_ctm(&operations)?,
            )?;
            content_bytes.extend_from_slice(&overlay);
        }

        // Defer old content stream removal; write new content immediately
        let old_content_refs = std::mem::take(&mut pages[page_index].content_refs);
        deferred_content_removals.extend(old_content_refs);
        write_page_contents_without_removal(file, pages, page_index, content_bytes)?;
        report.pages_touched += 1;
    }

    // Now remove deferred objects — no page will need them again
    for (resources, names) in &deferred_xobject_removals {
        remove_neutralized_xobjects(file, resources, names)?;
    }
    let mut removed_refs = BTreeSet::new();
    for old_ref in deferred_content_removals {
        if removed_refs.insert(old_ref) {
            file.objects.remove(&old_ref);
        }
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
    // Resolve the Info reference before removing it so we can delete the object
    let info_ref = match file.trailer.get("Info") {
        Some(PdfValue::Reference(object_ref)) => Some(*object_ref),
        _ => None,
    };
    file.trailer.remove("Info");
    if let Some(info_ref) = info_ref {
        file.objects.remove(&info_ref);
    }

    let root_ref = match file.trailer.get("Root") {
        Some(PdfValue::Reference(object_ref)) => *object_ref,
        _ => return Err(PdfError::Corrupt("trailer Root is missing".to_string())),
    };

    // Resolve the Metadata stream reference before removing it
    let metadata_ref = file
        .get_dictionary(root_ref)?
        .get("Metadata")
        .and_then(|v| match v {
            PdfValue::Reference(r) => Some(*r),
            _ => None,
        });

    if let PdfObject::Value(PdfValue::Dictionary(dictionary)) = file.get_object_mut(root_ref)? {
        dictionary.remove("Metadata");
    }
    if let Some(metadata_ref) = metadata_ref {
        file.objects.remove(&metadata_ref);
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

    // Collect all object refs reachable from EmbeddedFiles so we can remove them
    let mut refs_to_remove = Vec::new();
    if let Some(names_ref) = names_ref {
        if let Ok(names_dict) = file.get_dictionary(names_ref) {
            if let Some(ef_value) = names_dict.get("EmbeddedFiles").cloned() {
                collect_reachable_refs(file, &ef_value, &mut refs_to_remove, &mut BTreeSet::new());
            }
        }
    }

    // Remove Names key from the root catalog
    if let PdfObject::Value(PdfValue::Dictionary(dictionary)) = file.get_object_mut(root_ref)? {
        dictionary.remove("Names");
    }

    // Also remove EmbeddedFiles from the Names dictionary itself
    if let Some(names_ref) = names_ref {
        if let Ok(PdfObject::Value(PdfValue::Dictionary(names))) = file.get_object_mut(names_ref) {
            names.remove("EmbeddedFiles");
        }
    }

    // Remove all collected attachment objects from the file
    for obj_ref in refs_to_remove {
        file.objects.remove(&obj_ref);
    }
    Ok(())
}

/// Recursively collects all ObjectRefs reachable from a PdfValue tree.
fn collect_reachable_refs(
    file: &PdfFile,
    value: &PdfValue,
    refs: &mut Vec<ObjectRef>,
    visited: &mut BTreeSet<ObjectRef>,
) {
    match value {
        PdfValue::Reference(object_ref) => {
            if visited.insert(*object_ref) {
                refs.push(*object_ref);
                if let Ok(object) = file.get_object(*object_ref) {
                    match object.clone() {
                        PdfObject::Value(v) => {
                            collect_reachable_refs(file, &v, refs, visited);
                        }
                        PdfObject::Stream(s) => {
                            for v in s.dict.values() {
                                collect_reachable_refs(file, v, refs, visited);
                            }
                        }
                    }
                }
            }
        }
        PdfValue::Array(items) => {
            for item in items {
                collect_reachable_refs(file, item, refs, visited);
            }
        }
        PdfValue::Dictionary(dict) => {
            for v in dict.values() {
                collect_reachable_refs(file, v, refs, visited);
            }
        }
        _ => {}
    }
}

/// Removes the underlying stream objects for neutralized XObjects.
fn remove_neutralized_xobjects(
    file: &mut PdfFile,
    resources: &PdfDictionary,
    neutralized_names: &[String],
) -> PdfResult<()> {
    if neutralized_names.is_empty() {
        return Ok(());
    }
    let Some(xobjects_value) = resources.get("XObject") else {
        return Ok(());
    };
    let xobjects = file.resolve_dict(xobjects_value)?;
    let refs_to_remove: Vec<ObjectRef> = neutralized_names
        .iter()
        .filter_map(|name| match xobjects.get(name) {
            Some(PdfValue::Reference(object_ref)) => Some(*object_ref),
            _ => None,
        })
        .collect();
    for obj_ref in refs_to_remove {
        file.objects.remove(&obj_ref);
    }
    Ok(())
}

fn ensure_supported_operators(operations: &[Operation]) -> PdfResult<()> {
    for operation in operations {
        let supported = matches!(
            operation.operator.as_str(),
            // Graphics state
            "q" | "Q" | "cm" | "gs" | "w" | "J" | "j" | "M" | "d" | "ri" | "i"
            // Text state & operators
            | "BT" | "ET" | "Tf" | "Tm" | "Td" | "TD" | "T*"
            | "Tc" | "Tw" | "TL" | "Ts" | "Tz" | "Tr"
            | "Tj" | "TJ" | "'" | "\""
            // Path construction
            | "m" | "l" | "c" | "v" | "y" | "h" | "re"
            // Path painting
            | "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "n"
            // Clipping
            | "W" | "W*"
            // Color (device + general)
            | "rg" | "RG" | "g" | "G" | "k" | "K"
            | "cs" | "CS" | "sc" | "SC" | "scn" | "SCN"
            // XObjects
            | "Do"
            // Marked content (safe passthrough)
            | "BMC" | "BDC" | "EMC" | "MP" | "DP"
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
            "Form" => XObjectKind::Form {
                bbox: parse_form_bbox(&stream.dict),
                matrix: parse_form_matrix(&stream.dict),
            },
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

fn parse_form_bbox(dict: &PdfDictionary) -> Rect {
    // Default to the unit square when the BBox is missing or malformed. Any
    // intersection test against a user target will either correctly return
    // true (when the target covers the origin) or correctly return false
    // otherwise, which is the most conservative safe fallback.
    let Some(PdfValue::Array(values)) = dict.get("BBox") else {
        return Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        };
    };
    if values.len() != 4 {
        return Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        };
    }
    let numbers: Option<[f64; 4]> = {
        let mut nums = [0.0; 4];
        for (slot, value) in nums.iter_mut().zip(values.iter()) {
            match value.as_number() {
                Some(n) => *slot = n,
                None => return Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 1.0,
                    height: 1.0,
                },
            }
        }
        Some(nums)
    };
    let [x1, y1, x2, y2] = numbers.unwrap();
    Rect {
        x: x1.min(x2),
        y: y1.min(y2),
        width: (x2 - x1).abs(),
        height: (y2 - y1).abs(),
    }
}

fn parse_form_matrix(dict: &PdfDictionary) -> Matrix {
    let Some(PdfValue::Array(values)) = dict.get("Matrix") else {
        return Matrix::identity();
    };
    if values.len() != 6 {
        return Matrix::identity();
    }
    let mut nums = [0.0; 6];
    for (slot, value) in nums.iter_mut().zip(values.iter()) {
        match value.as_number() {
            Some(n) => *slot = n,
            None => return Matrix::identity(),
        }
    }
    Matrix {
        a: nums[0],
        b: nums[1],
        c: nums[2],
        d: nums[3],
        e: nums[4],
        f: nums[5],
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum XObjectKind {
    Image,
    Form { bbox: Rect, matrix: Matrix },
}

type GlyphRemovalMap = BTreeMap<usize, Vec<GlyphByteRef>>;

#[derive(Debug, Clone, Copy)]
struct GlyphByteRef {
    operand_index: usize,
    element_index: Option<usize>,
    byte_start: usize,
    byte_end: usize,
    width_units: f64,
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
                    width_units: glyph.width_units,
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
                    width_units: glyph.width_units,
                }),
            }
        }
    }
    output
}

fn rewrite_text_operations(
    operations: &mut [Operation],
    removals: &GlyphRemovalMap,
    mode: RedactionMode,
    warnings: &mut Vec<String>,
) -> usize {
    let compensate = matches!(mode, RedactionMode::Redact | RedactionMode::Erase);
    let mut removed = 0usize;

    for (operation_index, refs) in removals {
        let Some(operation) = operations.get_mut(*operation_index) else {
            continue;
        };

        // For ' and " operators, kern compensation is not possible (they carry
        // implicit newline / spacing side effects). Fall back to strip.
        let use_compensation = compensate && matches!(operation.operator.as_str(), "Tj" | "TJ");
        if compensate && !use_compensation {
            warnings.push(format!(
                "operator '{}' fell back to strip mode (kern compensation not supported)",
                operation.operator
            ));
        }

        // --- Direct string operators (Tj, ', ") ---
        let direct_refs: Vec<&GlyphByteRef> =
            refs.iter().filter(|e| e.element_index.is_none()).collect();

        if !direct_refs.is_empty() {
            // Group byte indices by operand
            let mut byte_sets = BTreeMap::<usize, BTreeSet<usize>>::new();
            let mut glyph_starts = BTreeMap::<usize, BTreeMap<usize, f64>>::new();
            let mut seen_ranges = BTreeSet::<(usize, usize)>::new();
            for entry in &direct_refs {
                let bytes = byte_sets.entry(entry.operand_index).or_default();
                for i in entry.byte_start..entry.byte_end {
                    bytes.insert(i);
                }
                if use_compensation && seen_ranges.insert((entry.byte_start, entry.byte_end)) {
                    glyph_starts
                        .entry(entry.operand_index)
                        .or_default()
                        .insert(entry.byte_start, entry.width_units);
                }
            }

            for (operand_index, bytes) in &byte_sets {
                if use_compensation {
                    // Build TJ array with kern compensation, convert Tj → TJ
                    if let Some(PdfValue::String(string)) = operation.operands.get(*operand_index) {
                        let starts = glyph_starts.get(operand_index).cloned().unwrap_or_default();
                        let array = build_compensated_array(string, bytes, &starts);
                        removed += bytes.len();
                        operation.operands[*operand_index] = PdfValue::Array(array);
                        operation.operator = "TJ".to_string();
                    }
                } else if let Some(PdfValue::String(string)) =
                    operation.operands.get_mut(*operand_index)
                {
                    removed += remove_bytes_from_string(string, bytes);
                }
            }
        }

        // --- TJ array operators ---
        let array_refs: Vec<&GlyphByteRef> =
            refs.iter().filter(|e| e.element_index.is_some()).collect();

        if !array_refs.is_empty() {
            // Group by (operand_index, element_index)
            let mut element_bytes = BTreeMap::<(usize, usize), BTreeSet<usize>>::new();
            let mut element_glyph_starts = BTreeMap::<(usize, usize), BTreeMap<usize, f64>>::new();
            let mut seen_ranges = BTreeSet::<(usize, usize, usize, usize)>::new();
            for entry in &array_refs {
                let el = entry.element_index.unwrap();
                let bytes = element_bytes.entry((entry.operand_index, el)).or_default();
                for i in entry.byte_start..entry.byte_end {
                    bytes.insert(i);
                }
                if use_compensation
                    && seen_ranges.insert((
                        entry.operand_index,
                        el,
                        entry.byte_start,
                        entry.byte_end,
                    ))
                {
                    element_glyph_starts
                        .entry((entry.operand_index, el))
                        .or_default()
                        .insert(entry.byte_start, entry.width_units);
                }
            }

            if use_compensation {
                // Rebuild the entire TJ array, expanding modified string elements
                // into sub-arrays with kern compensation.
                let affected_operands: BTreeSet<usize> =
                    element_bytes.keys().map(|(op, _)| *op).collect();
                for operand_index in affected_operands {
                    if let Some(PdfValue::Array(items)) =
                        operation.operands.get(operand_index).cloned()
                    {
                        let mut new_array = Vec::new();
                        for (el_index, item) in items.iter().enumerate() {
                            let key = (operand_index, el_index);
                            if let (Some(bytes), PdfValue::String(string)) =
                                (element_bytes.get(&key), item)
                            {
                                let starts =
                                    element_glyph_starts.get(&key).cloned().unwrap_or_default();
                                let sub = build_compensated_array(string, bytes, &starts);
                                removed += bytes.len();
                                new_array.extend(sub);
                            } else {
                                new_array.push(item.clone());
                            }
                        }
                        operation.operands[operand_index] = PdfValue::Array(new_array);
                    }
                }
            } else {
                for ((operand_index, element_index), bytes) in &element_bytes {
                    if let Some(PdfValue::Array(items)) = operation.operands.get_mut(*operand_index)
                    {
                        if let Some(PdfValue::String(string)) = items.get_mut(*element_index) {
                            removed += remove_bytes_from_string(string, bytes);
                        }
                    }
                }
            }
        }
    }
    removed
}

fn count_removed_glyphs(removals: &GlyphRemovalMap) -> usize {
    removals
        .values()
        .map(|entries| {
            entries
                .iter()
                .map(|r| (r.operand_index, r.element_index, r.byte_start, r.byte_end))
                .collect::<BTreeSet<_>>()
                .len()
        })
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

/// Builds a TJ-style array from a string with some bytes removed and kern
/// compensation inserted. `glyph_starts` maps each removed glyph's
/// `byte_start` to its `width_units` (deduped so multi-char glyphs count once).
fn build_compensated_array(
    string: &PdfString,
    removed_indices: &BTreeSet<usize>,
    glyph_starts: &BTreeMap<usize, f64>,
) -> Vec<PdfValue> {
    let mut result: Vec<PdfValue> = Vec::new();
    let mut kept_buf: Vec<u8> = Vec::new();
    let mut kern_accum: f64 = 0.0;
    let mut in_removed = false;

    for (i, &byte) in string.0.iter().enumerate() {
        let is_removed = removed_indices.contains(&i);
        if is_removed {
            if let Some(&width) = glyph_starts.get(&i) {
                kern_accum += width;
            }
        }
        match (in_removed, is_removed) {
            (false, false) => kept_buf.push(byte),
            (false, true) => {
                if !kept_buf.is_empty() {
                    result.push(PdfValue::String(PdfString(std::mem::take(&mut kept_buf))));
                }
                in_removed = true;
            }
            (true, false) => {
                if kern_accum.abs() > 0.01 {
                    result.push(PdfValue::Number(-kern_accum));
                    kern_accum = 0.0;
                }
                in_removed = false;
                kept_buf.push(byte);
            }
            (true, true) => {}
        }
    }

    // Flush trailing state
    if kern_accum.abs() > 0.01 {
        result.push(PdfValue::Number(-kern_accum));
    }
    if !kept_buf.is_empty() {
        result.push(PdfValue::String(PdfString(kept_buf)));
    }

    result
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
            "cm" => ctm = matrix_from_operands(&operation.operands)?.multiply(ctm),
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
            "v" => {
                // v x2 y2 x3 y3: the first control point is the current point;
                // the second control point and endpoint come from the operands.
                let current =
                    current_path_point(&path_segments).unwrap_or(Point { x: 0.0, y: 0.0 });
                let second = transform_point_pair(&operation.operands, 0, ctm, page_transform)?;
                let third = transform_point_pair(&operation.operands, 2, ctm, page_transform)?;
                path_segments.push(PathSegment::CurveTo(current, second, third));
            }
            "y" => {
                // y x1 y1 x3 y3: the second control point is the endpoint.
                let first = transform_point_pair(&operation.operands, 0, ctm, page_transform)?;
                let third = transform_point_pair(&operation.operands, 2, ctm, page_transform)?;
                path_segments.push(PathSegment::CurveTo(first, third, third));
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

/// Returns (count_removed, list_of_neutralized_xobject_names).
fn neutralize_image_operations(
    operations: &mut [Operation],
    targets: &[&NormalizedPageTarget],
    page_transform: Matrix,
    xobjects: &BTreeMap<String, XObjectKind>,
) -> PdfResult<(usize, Vec<String>)> {
    let mut removed = 0usize;
    let mut neutralized_names = Vec::new();
    let mut ctm = Matrix::identity();
    let mut ctm_stack = Vec::new();
    for operation in operations.iter_mut() {
        match operation.operator.as_str() {
            "q" => ctm_stack.push(ctm),
            "Q" => ctm = ctm_stack.pop().unwrap_or(Matrix::identity()),
            "cm" => ctm = matrix_from_operands(&operation.operands)?.multiply(ctm),
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
                            neutralized_names.push(name.to_string());
                            operation.operator = "n".to_string();
                            operation.operands.clear();
                            removed += 1;
                        }
                    }
                    Some(XObjectKind::Form { bbox, matrix }) => {
                        // The Form's effective transform in page space is
                        // matrix × current CTM × page_transform. We skip the
                        // Form entirely when its bounding rectangle, mapped
                        // into page space, does not intersect any target; only
                        // Forms that actually cover redacted content still
                        // produce a hard error.
                        let quad = bbox
                            .to_quad()
                            .transform(matrix.multiply(ctm).multiply(page_transform));
                        if targets.iter().any(|target| target.intersects_quad(&quad)) {
                            return Err(PdfError::Unsupported(
                                "Form XObjects intersecting a redaction target are not supported"
                                    .to_string(),
                            ));
                        }
                    }
                    None => {
                        return Err(PdfError::Unsupported(format!(
                            "Do operator references unknown XObject /{name}"
                        )));
                    }
                }
            }
            _ => {}
        }
    }
    Ok((removed, neutralized_names))
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
        let dict = match file.get_dictionary(*annotation_ref) {
            Ok(dict) => dict,
            Err(PdfError::MissingObject(_)) => {
                // Annotation object missing (shared and already removed, or absent)
                removed += 1;
                continue;
            }
            Err(other) => return Err(other),
        };
        let rect = dict
            .get("Rect")
            .map(parse_rect)
            .transpose()?
            .map(|rect| page_transform.transform_rect(rect));
        let subtype = dict
            .get("Subtype")
            .and_then(PdfValue::as_name)
            .unwrap_or("");
        let intersects = rect
            .map(|rect| targets.iter().any(|target| target.intersects_rect(&rect)))
            // Annotations without a Rect are conservatively treated as intersecting
            // unless they are known-harmless subtypes (Link navigation only).
            .unwrap_or(subtype != "Link");
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

fn write_page_contents_without_removal(
    file: &mut PdfFile,
    pages: &mut [PageInfo],
    page_index: usize,
    bytes: Vec<u8>,
) -> PdfResult<()> {
    let page = pages
        .get_mut(page_index)
        .ok_or(PdfError::InvalidPageIndex(page_index))?;

    let content_ref = file.allocate_object_ref();
    let mut dict = PdfDictionary::new();
    // FlateDecode-compress the rewritten content stream so the saved PDF does
    // not bloat with plaintext bytes. If compression fails for any reason,
    // fall back to writing the raw bytes.
    let (data, filter) = match flate_encode(&bytes) {
        Ok(compressed) => (compressed, true),
        Err(_) => (bytes, false),
    };
    if filter {
        dict.insert("Filter".to_string(), PdfValue::Name("FlateDecode".into()));
    }
    file.insert_object(
        content_ref,
        PdfObject::Stream(PdfStream { dict, data }),
    );
    if let PdfObject::Value(PdfValue::Dictionary(dictionary)) =
        file.get_object_mut(page.page_ref)?
    {
        dictionary.insert("Contents".to_string(), PdfValue::Reference(content_ref));
    }
    page.content_refs = vec![content_ref];
    Ok(())
}

fn overlay_stream_bytes(
    targets: &[&NormalizedPageTarget],
    color: Color,
    page_transform: Matrix,
    final_ctm: Matrix,
) -> PdfResult<Vec<u8>> {
    let inverse_page_transform = page_transform.inverse().ok_or_else(|| {
        PdfError::Unsupported(
            "page transform is singular and cannot be used for overlay painting".to_string(),
        )
    })?;
    let inverse_final_ctm = final_ctm.inverse().ok_or_else(|| {
        PdfError::Unsupported(
            "page content leaves a singular CTM, so redaction overlays cannot be painted safely"
                .to_string(),
        )
    })?;
    let red = f64::from(color.r) / 255.0;
    let green = f64::from(color.g) / 255.0;
    let blue = f64::from(color.b) / 255.0;
    let mut output = String::new();
    output.push_str("q\n");
    output.push_str(&format!(
        "{} {} {} {} {} {} cm\n",
        format_number(inverse_final_ctm.a),
        format_number(inverse_final_ctm.b),
        format_number(inverse_final_ctm.c),
        format_number(inverse_final_ctm.d),
        format_number(inverse_final_ctm.e),
        format_number(inverse_final_ctm.f)
    ));
    output.push_str(&format!("{red:.3} {green:.3} {blue:.3} rg\n"));
    for target in targets {
        for quad in &target.quads {
            let [a, b, c, d] = quad.transform(inverse_page_transform).points;
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
    Ok(output.into_bytes())
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

/// Rejects documents whose default Optional Content configuration marks any
/// OCG as off. Content inside an off-by-default OCG is not shown to the
/// reader but still lives in the content stream; a user who sees only the
/// visible page cannot select that text as a redaction target, so the engine
/// would silently leave the hidden content in the saved PDF. Erroring up
/// front is the same posture the engine takes for other silent-leak vectors
/// (encrypted PDFs, Form XObjects that intersect targets, etc.).
fn reject_hidden_optional_content(file: &PdfFile) -> PdfResult<()> {
    let Some(PdfValue::Reference(root_ref)) = file.trailer.get("Root") else {
        return Ok(());
    };
    let catalog = match file.get_dictionary(*root_ref) {
        Ok(dict) => dict,
        Err(_) => return Ok(()),
    };
    let Some(oc_properties_value) = catalog.get("OCProperties") else {
        return Ok(());
    };
    let oc_properties = match file.resolve_dict(oc_properties_value) {
        Ok(dict) => dict,
        Err(_) => return Ok(()),
    };
    let Some(default_value) = oc_properties.get("D") else {
        return Ok(());
    };
    let default_config = match file.resolve_dict(default_value) {
        Ok(dict) => dict,
        Err(_) => return Ok(()),
    };
    if let Some(off_value) = default_config.get("OFF") {
        let resolved = file.resolve(off_value).unwrap_or(off_value);
        if let Some(entries) = resolved.as_array() {
            if !entries.is_empty() {
                return Err(PdfError::Unsupported(
                    "documents with Optional Content Groups that are off by default are not \
                     supported for redaction: hidden layers may carry sensitive content that is \
                     not visible to the user and therefore cannot be safely targeted"
                        .to_string(),
                ));
            }
        }
    }
    // Also refuse documents that declare a non-default BaseState of "OFF",
    // which hides every OCG unless explicitly turned on.
    if let Some(base_state) = default_config
        .get("BaseState")
        .and_then(PdfValue::as_name)
    {
        if base_state == "OFF" || base_state == "Unchanged" {
            return Err(PdfError::Unsupported(format!(
                "documents with /OCProperties /D /BaseState /{base_state} are not supported for \
                 redaction because hidden layers cannot be safely targeted"
            )));
        }
    }
    Ok(())
}

fn current_path_point(path_segments: &[PathSegment]) -> Option<Point> {
    for segment in path_segments.iter().rev() {
        match segment {
            PathSegment::MoveTo(point) | PathSegment::LineTo(point) => return Some(*point),
            PathSegment::CurveTo(_, _, endpoint) => return Some(*endpoint),
            PathSegment::Rect(rect) => {
                return Some(Point {
                    x: rect.x,
                    y: rect.y,
                });
            }
            PathSegment::ClosePath => continue,
        }
    }
    None
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

fn final_page_ctm(operations: &[Operation]) -> PdfResult<Matrix> {
    let mut ctm = Matrix::identity();
    let mut stack = Vec::new();
    for operation in operations {
        match operation.operator.as_str() {
            "q" => stack.push(ctm),
            "Q" => ctm = stack.pop().unwrap_or(Matrix::identity()),
            "cm" => ctm = matrix_from_operands(&operation.operands)?.multiply(ctm),
            _ => {}
        }
    }
    Ok(ctm)
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

#[cfg(test)]
mod tests {
    use super::ensure_supported_operators;
    use pdf_content::Operation;

    #[test]
    fn accepts_common_graphics_state_operators() {
        let operations = vec![
            Operation {
                operator: "j".to_string(),
                operands: vec![],
            },
            Operation {
                operator: "J".to_string(),
                operands: vec![],
            },
            Operation {
                operator: "M".to_string(),
                operands: vec![],
            },
            Operation {
                operator: "d".to_string(),
                operands: vec![],
            },
            Operation {
                operator: "ri".to_string(),
                operands: vec![],
            },
            Operation {
                operator: "i".to_string(),
                operands: vec![],
            },
        ];

        ensure_supported_operators(&operations).expect("operators should be supported");
    }
}
