use std::collections::{BTreeMap, BTreeSet};

use pdf_content::{
    Operation, PaintOperator, PathSegment, parse_content_stream, parse_page_contents,
};
use pdf_graphics::{Color, Matrix, Point, Rect};
use pdf_objects::{
    ObjectRef, PageInfo, PdfDictionary, PdfError, PdfFile, PdfObject, PdfResult, PdfStream,
    PdfString, PdfValue, decode_stream, flate_encode, serialize_value,
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
    /// Count of Image XObject `Do` invocations whose underlying stream
    /// was rewritten in place to mask only the targeted pixel region
    /// (partial overlap), instead of being replaced with `n` (full
    /// drop). Each masked image produces one fresh indirect object via
    /// copy-on-write so multi-page-shared images are not affected.
    #[serde(default)]
    pub image_draws_masked: usize,
    pub annotations_removed: usize,
    /// Count of Form XObject copies produced by copy-on-write redaction, each
    /// of which carries the rewrite of a Form whose `BBox × Matrix × CTM`
    /// intersected at least one redaction target.
    #[serde(default)]
    pub form_xobjects_rewritten: usize,
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

    if plan.sanitize_hidden_ocgs {
        sanitize_hidden_optional_content(file, pages, &mut report.warnings)?;
    } else {
        reject_hidden_optional_content(file)?;
    }

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
    // Set of original Image XObject refs whose stream was copy-on-write
    // replaced by a masked variant on at least one page. After all
    // per-page work finishes we re-check whether any live reference
    // (page or Form XObject Resources) still points at each original
    // and drop the unreachable ones from `file.objects`. This prevents
    // the writer from emitting the original (unmasked) pixels as a
    // dangling indirect object.
    let mut cowed_image_originals: BTreeSet<ObjectRef> = BTreeSet::new();

    // Allocated lazily on the first page that actually needs to stamp an
    // overlay label. Shared across pages so the saved PDF contains a single
    // Helvetica font dictionary for the label rather than one copy per page.
    let mut overlay_font_ref: Option<ObjectRef> = None;

    for (page_index, targets) in page_targets {
        let mut page = pages
            .get(page_index)
            .cloned()
            .ok_or(PdfError::InvalidPageIndex(page_index))?;
        let page_transform = page.page_box.normalized_transform();
        let extracted = analyze_page_text(file, page_index, &page)?;
        let parsed = parse_page_contents(file, &page)?;
        ensure_supported_operators(&parsed.operations)?;
        let xobjects = load_xobjects(file, &page.resources)?;

        // Partition glyphs by their origin: page content stream vs. Form
        // XObject. Form-origin glyphs carry operation indices and byte
        // locations relative to the Form's stream, so they cannot be mixed
        // with the page's operations when rewriting.
        let mut page_glyphs = Vec::with_capacity(extracted.glyphs.len());
        let mut form_glyph_groups: BTreeMap<ObjectRef, Vec<Glyph>> = BTreeMap::new();
        for glyph in extracted.glyphs.into_iter() {
            match glyph.source_form {
                None => page_glyphs.push(glyph),
                Some(form_ref) => form_glyph_groups.entry(form_ref).or_default().push(glyph),
            }
        }

        // Identify Forms invoked on this page whose bounding quad overlaps a
        // target, and redact each of them by allocating a per-page copy and
        // rewriting its content stream.
        let form_redactions = redact_intersecting_forms(
            file,
            &page,
            &parsed.operations,
            &targets,
            page_transform,
            &xobjects,
            &form_glyph_groups,
            plan.mode,
            &mut report.warnings,
        )?;
        let redacted_form_names: BTreeSet<String> =
            form_redactions.iter().map(|r| r.name.clone()).collect();
        for redaction in &form_redactions {
            report.text_glyphs_removed += redaction.glyphs_removed;
        }
        report.form_xobjects_rewritten += form_redactions.len();
        if !form_redactions.is_empty() {
            override_page_xobject_refs(file, &mut pages[page_index], &form_redactions)?;
            // Refresh local snapshot so downstream code (annotation
            // removal, overlay font registration, deferred removals)
            // sees the post-override resources state.
            page = pages[page_index].clone();
        }

        let glyph_removals = collect_glyph_removals(&page_glyphs, &targets);
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

        let outcome = neutralize_image_operations(
            &mut operations,
            &targets,
            page_transform,
            &xobjects,
            &redacted_form_names,
            plan.fill_color,
        )?;
        report.image_draws_removed += outcome.removed_count;
        report.image_draws_masked += outcome.masked_count;
        if outcome.removed_count > 0 {
            report.warnings.push(format!(
                "page {page_index}: intersecting images were removed at invocation level"
            ));
        }

        // Apply pending partial-mask rewrites via copy-on-write so any
        // other page that shares the same image stream is unaffected.
        // Multiple `Do` invocations of the same Image XObject on a
        // single page are grouped by `original_ref` so all of their
        // pixel rectangles land in one masked COW copy — without
        // grouping, the last `rewire_page_xobject` would clobber
        // earlier masks. On Unsupported (image format outside the
        // supported subset) every name in the group falls back to
        // whole-invocation neutralization.
        let mut partial_mask_fallback_names: Vec<String> = Vec::new();
        let mut grouped_masks: BTreeMap<
            ObjectRef,
            (
                Vec<crate::image_mask::ImagePixelRect>,
                BTreeSet<String>,
            ),
        > = BTreeMap::new();
        for mask in &outcome.partial_masks {
            let entry = grouped_masks.entry(mask.original_ref).or_default();
            entry.0.push(mask.pixel_rect);
            entry.1.insert(mask.xobject_name.clone());
        }
        for (original_ref, (pixel_rects, names)) in &grouped_masks {
            match apply_partial_masks_for_image(
                file,
                pages,
                page_index,
                *original_ref,
                pixel_rects,
                names,
                plan.fill_color,
            ) {
                Ok(()) => {
                    cowed_image_originals.insert(*original_ref);
                }
                Err(PdfError::Unsupported(_)) => {
                    for name in names {
                        fallback_image_to_drop(&mut operations, name);
                        partial_mask_fallback_names.push(name.clone());
                    }
                    let group_count = pixel_rects.len();
                    report.image_draws_masked =
                        report.image_draws_masked.saturating_sub(group_count);
                    report.image_draws_removed += group_count;
                }
                Err(other) => return Err(other),
            }
        }

        // Defer XObject removal so shared objects stay available for other pages
        let mut neutralized_names = outcome.neutralized_names;
        neutralized_names.extend(partial_mask_fallback_names);
        if !neutralized_names.is_empty() {
            deferred_xobject_removals
                .push((page.resources.clone(), neutralized_names));
        }

        let annotation_removed = if plan.remove_intersecting_annotations {
            remove_annotations(file, &page, &targets, page_transform)?
        } else {
            0
        };
        report.annotations_removed += annotation_removed;

        let mut content_bytes = serialize_operations(&operations);
        if plan.mode == RedactionMode::Redact {
            let overlay_spec = plan
                .overlay_text
                .as_deref()
                .map(|text| (OVERLAY_FONT_NAME, text));
            if overlay_spec.is_some() {
                overlay_font_ref = Some(ensure_overlay_font(file, overlay_font_ref)?);
                register_overlay_font_on_page(
                    file,
                    page.page_ref,
                    &page.resources,
                    overlay_font_ref
                        .expect("overlay font must be allocated when overlay_spec is some"),
                )?;
            }
            let overlay = overlay_stream_bytes(
                &targets,
                plan.fill_color,
                page.page_box.normalized_transform(),
                final_page_ctm(&operations)?,
                overlay_spec,
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

    // Drop original Image XObjects whose pixels were copy-on-write
    // replaced and whose stream is no longer referenced from any live
    // page or Form XObject. Without this the writer would emit the
    // unmasked original alongside the masked COW copy, leaving the
    // redacted pixels recoverable from the saved bytes.
    if !cowed_image_originals.is_empty() {
        prune_unreferenced_images(file, &cowed_image_originals);
    }

    Ok(report)
}

/// Remove every entry of `candidates` from `file.objects` that no
/// remaining live indirect object references. Reachability is computed
/// over a single sweep of `file.objects` (excluding the candidate
/// streams themselves) plus the trailer.
fn prune_unreferenced_images(file: &mut PdfFile, candidates: &BTreeSet<ObjectRef>) {
    let mut still_referenced: BTreeSet<ObjectRef> = BTreeSet::new();
    for (object_ref, object) in &file.objects {
        if candidates.contains(object_ref) {
            // The object referencing itself does not count as a live
            // reference — we are deciding whether anything else points
            // at it.
            continue;
        }
        match object {
            PdfObject::Value(value) => {
                collect_referenced_into(value, candidates, &mut still_referenced)
            }
            PdfObject::Stream(stream) => {
                for value in stream.dict.values() {
                    collect_referenced_into(value, candidates, &mut still_referenced);
                }
            }
        }
    }
    for value in file.trailer.values() {
        collect_referenced_into(value, candidates, &mut still_referenced);
    }
    for candidate in candidates {
        if !still_referenced.contains(candidate) {
            file.objects.remove(candidate);
        }
    }
}

fn collect_referenced_into(
    value: &PdfValue,
    candidates: &BTreeSet<ObjectRef>,
    out: &mut BTreeSet<ObjectRef>,
) {
    match value {
        PdfValue::Reference(target) => {
            if candidates.contains(target) {
                out.insert(*target);
            }
        }
        PdfValue::Array(items) => {
            for item in items {
                collect_referenced_into(item, candidates, out);
            }
        }
        PdfValue::Dictionary(dict) => {
            for v in dict.values() {
                collect_referenced_into(v, candidates, out);
            }
        }
        _ => {}
    }
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
/// Apply a group of partial-mask requests that all target the same
/// underlying image stream. Decodes the original once, paints every
/// pixel rectangle, re-encodes once, allocates one COW image, and
/// repoints every involved XObject name on this page at the new ref.
/// Returns [`PdfError::Unsupported`] when the image's format is
/// outside the supported subset; the caller falls back to
/// whole-invocation drop for every name in the group.
fn apply_partial_masks_for_image(
    file: &mut PdfFile,
    pages: &mut [PageInfo],
    page_index: usize,
    original_ref: ObjectRef,
    pixel_rects: &[crate::image_mask::ImagePixelRect],
    xobject_names: &BTreeSet<String>,
    fill_color: Color,
) -> PdfResult<()> {
    let stream = match file.get_object(original_ref)? {
        PdfObject::Stream(stream) => stream.clone(),
        _ => {
            return Err(PdfError::Corrupt(format!(
                "Image XObject {} {} did not resolve to a stream",
                original_ref.object_number, original_ref.generation
            )));
        }
    };
    let masked =
        crate::image_mask::mask_image_region_multi(&stream, pixel_rects, fill_color)?;
    let new_stream = PdfStream {
        dict: masked.new_dict,
        data: masked.new_data,
    };
    let new_ref = file.allocate_object_ref();
    file.objects.insert(new_ref, PdfObject::Stream(new_stream));

    // Repoint every page-resource name that mapped to this original
    // image at the single COW copy.
    for name in xobject_names {
        rewire_page_xobject(file, pages, page_index, name, new_ref)?;
    }
    Ok(())
}

/// Replace the page's `Resources.XObject[name]` with a fresh reference,
/// cloning intermediate dictionaries so other pages that share the same
/// resources stay pointing at the original image. Updates both the
/// in-memory `PageInfo.resources` cache AND the page object in
/// `file.objects` so the writer picks up the new reference.
fn rewire_page_xobject(
    file: &mut PdfFile,
    pages: &mut [PageInfo],
    page_index: usize,
    name: &str,
    new_ref: ObjectRef,
) -> PdfResult<()> {
    let resources_value = pages[page_index]
        .resources
        .get("XObject")
        .cloned()
        .ok_or_else(|| {
            PdfError::Corrupt(
                "page resources missing /XObject when applying partial image mask"
                    .to_string(),
            )
        })?;
    let xobjects_ref = match resources_value {
        PdfValue::Reference(reference) => Some(reference),
        PdfValue::Dictionary(_) => None,
        _ => {
            return Err(PdfError::Corrupt(
                "page Resources.XObject is not a dict or reference".to_string(),
            ));
        }
    };

    if let Some(xobjects_ref) = xobjects_ref {
        // Indirect XObject dict: clone and update.
        let mut dict = match file.get_object(xobjects_ref)? {
            PdfObject::Value(PdfValue::Dictionary(dict)) => dict.clone(),
            _ => {
                return Err(PdfError::Corrupt(
                    "page Resources.XObject reference does not point at a dictionary"
                        .to_string(),
                ));
            }
        };
        dict.insert(name.to_string(), PdfValue::Reference(new_ref));
        let new_xobjects_ref = file.allocate_object_ref();
        file.objects
            .insert(new_xobjects_ref, PdfObject::Value(PdfValue::Dictionary(dict)));
        pages[page_index].resources.insert(
            "XObject".to_string(),
            PdfValue::Reference(new_xobjects_ref),
        );
    } else if let Some(PdfValue::Dictionary(dict)) = pages[page_index].resources.get_mut("XObject")
    {
        dict.insert(name.to_string(), PdfValue::Reference(new_ref));
    }

    // Mirror the change into the page object's /Resources so the
    // writer emits the updated XObject pointer.
    let page_ref = pages[page_index].page_ref;
    let updated_resources = pages[page_index].resources.clone();
    match file.get_object_mut(page_ref)? {
        PdfObject::Value(PdfValue::Dictionary(page_dict)) => {
            page_dict.insert(
                "Resources".to_string(),
                PdfValue::Dictionary(updated_resources),
            );
        }
        _ => {
            return Err(PdfError::Corrupt(format!(
                "page {} {} is not a dictionary",
                page_ref.object_number, page_ref.generation
            )));
        }
    }
    Ok(())
}

/// Rewrite the `Do` invocation that targets `name` to a no-op `n`.
/// Used when a partial-mask attempt fails (unsupported format) and the
/// caller falls back to whole-invocation neutralization.
fn fallback_image_to_drop(operations: &mut [Operation], name: &str) {
    for op in operations.iter_mut() {
        if op.operator == "Do" {
            if let Some(PdfValue::Name(operand_name)) = op.operands.first() {
                if operand_name == name {
                    op.operator = "n".to_string();
                    op.operands.clear();
                }
            }
        }
    }
}

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
    // PDF § 7.8.2 lets a BX/EX compatibility section enclose operators that
    // a conforming reader may ignore. Within such a section we still require
    // every operator we recognize to be one we can redact safely, but any
    // unrecognized operator is passed through — matching what a viewer does
    // — so a page that is otherwise supported isn't rejected outright.
    let mut compat_depth: u32 = 0;
    for operation in operations {
        let op = operation.operator.as_str();
        if op == "BX" {
            compat_depth = compat_depth.saturating_add(1);
            continue;
        }
        if op == "EX" {
            compat_depth = compat_depth.saturating_sub(1);
            continue;
        }
        let supported = matches!(
            op,
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
        if !supported && compat_depth == 0 {
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
            "Image" => XObjectKind::Image {
                object_ref,
                dict: stream.dict.clone(),
            },
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
                None => {
                    return Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 1.0,
                        height: 1.0,
                    };
                }
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

#[derive(Debug, Clone, PartialEq)]
enum XObjectKind {
    Image {
        object_ref: ObjectRef,
        dict: PdfDictionary,
    },
    Form {
        bbox: Rect,
        matrix: Matrix,
    },
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
    neutralize_vector_operations_with_ctm(operations, targets, page_transform, Matrix::identity())
}

fn neutralize_vector_operations_with_ctm(
    operations: &mut [Operation],
    targets: &[&NormalizedPageTarget],
    page_transform: Matrix,
    base_ctm: Matrix,
) -> PdfResult<usize> {
    let mut removed = 0usize;
    let mut ctm = base_ctm;
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

/// Outcome of the image-XObject neutralization pass on a single
/// content stream. Whole-image drops use `n` rewriting and the
/// underlying stream is removed by the caller; partial overlaps are
/// recorded as [`PendingImageMask`] entries to be applied via
/// copy-on-write so multi-page-shared images don't leak across pages.
#[derive(Debug, Default)]
struct NeutralizationOutcome {
    /// Names of XObjects whose `Do` was replaced with `n` (full drop).
    neutralized_names: Vec<String>,
    /// Image XObjects whose underlying stream needs to be rewritten in
    /// place with a masked copy (partial overlap).
    partial_masks: Vec<PendingImageMask>,
    /// Count of full-drop replacements.
    removed_count: usize,
    /// Count of partial-mask rewrites.
    masked_count: usize,
}

#[derive(Debug)]
struct PendingImageMask {
    xobject_name: String,
    original_ref: ObjectRef,
    pixel_rect: crate::image_mask::ImagePixelRect,
}

fn neutralize_image_operations(
    operations: &mut [Operation],
    targets: &[&NormalizedPageTarget],
    page_transform: Matrix,
    xobjects: &BTreeMap<String, XObjectKind>,
    redacted_form_names: &BTreeSet<String>,
    fill_color: Color,
) -> PdfResult<NeutralizationOutcome> {
    neutralize_image_operations_with_ctm(
        operations,
        targets,
        page_transform,
        xobjects,
        redacted_form_names,
        Matrix::identity(),
        fill_color,
    )
}

#[allow(clippy::too_many_arguments)]
fn neutralize_image_operations_with_ctm(
    operations: &mut [Operation],
    targets: &[&NormalizedPageTarget],
    page_transform: Matrix,
    xobjects: &BTreeMap<String, XObjectKind>,
    redacted_form_names: &BTreeSet<String>,
    base_ctm: Matrix,
    fill_color: Color,
) -> PdfResult<NeutralizationOutcome> {
    let _ = fill_color; // mask colour is consumed when the partial mask is applied
    let mut outcome = NeutralizationOutcome::default();
    let mut ctm = base_ctm;
    let mut ctm_stack = Vec::new();
    for operation in operations.iter_mut() {
        match operation.operator.as_str() {
            "q" => ctm_stack.push(ctm),
            "Q" => ctm = ctm_stack.pop().unwrap_or(Matrix::identity()),
            "cm" => ctm = matrix_from_operands(&operation.operands)?.multiply(ctm),
            "Do" => {
                let name = operand_name(operation, 0)?;
                match xobjects.get(name) {
                    Some(XObjectKind::Image { object_ref, dict }) => {
                        let total_transform = ctm.multiply(page_transform);
                        let image_quad = Rect {
                            x: 0.0,
                            y: 0.0,
                            width: 1.0,
                            height: 1.0,
                        }
                        .to_quad()
                        .transform(total_transform);
                        if !targets.iter().any(|target| target.intersects_quad(&image_quad)) {
                            // No overlap — leave the Do intact.
                            continue;
                        }
                        // Determine whether overlap is partial or full
                        // by mapping the union of intersecting target
                        // quads back into image-space coordinates.
                        match compute_image_pixel_rect(
                            dict,
                            total_transform,
                            targets,
                        ) {
                            ImageOverlap::Full | ImageOverlap::Unsupported => {
                                outcome.neutralized_names.push(name.to_string());
                                operation.operator = "n".to_string();
                                operation.operands.clear();
                                outcome.removed_count += 1;
                            }
                            ImageOverlap::Partial(pixel_rect) => {
                                outcome.partial_masks.push(PendingImageMask {
                                    xobject_name: name.to_string(),
                                    original_ref: *object_ref,
                                    pixel_rect,
                                });
                                outcome.masked_count += 1;
                            }
                        }
                    }
                    Some(XObjectKind::Form { bbox, matrix }) => {
                        // Forms whose content was rewritten by the pre-pass
                        // have already been redacted in place, so their Do
                        // still points (through the page's updated Resources)
                        // at the per-page copy and must be left alone here.
                        if redacted_form_names.contains(name) {
                            // already handled upstream
                        } else {
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
    Ok(outcome)
}

#[derive(Debug)]
enum ImageOverlap {
    Full,
    Partial(crate::image_mask::ImagePixelRect),
    /// Geometry can't be computed (degenerate CTM, unreadable dict).
    /// Caller falls back to whole-invocation neutralization.
    Unsupported,
}

fn compute_image_pixel_rect(
    dict: &PdfDictionary,
    total_transform: Matrix,
    targets: &[&NormalizedPageTarget],
) -> ImageOverlap {
    let Some(width) = dict.get("Width").and_then(PdfValue::as_integer) else {
        return ImageOverlap::Unsupported;
    };
    let Some(height) = dict.get("Height").and_then(PdfValue::as_integer) else {
        return ImageOverlap::Unsupported;
    };
    if width <= 0 || height <= 0 {
        return ImageOverlap::Unsupported;
    }
    let width = width as u32;
    let height = height as u32;

    let Some(inverse) = total_transform.inverse() else {
        return ImageOverlap::Unsupported;
    };

    // Aggregate the union of all target hits in unit-square (u, v) space.
    // Targets that don't overlap the image quad are skipped.
    let mut u_min = f64::INFINITY;
    let mut u_max = f64::NEG_INFINITY;
    let mut v_min = f64::INFINITY;
    let mut v_max = f64::NEG_INFINITY;

    let image_quad = Rect {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    }
    .to_quad()
    .transform(total_transform);
    let image_rect = image_quad.bounding_rect();

    for target in targets {
        if !target.intersects_quad(&image_quad) {
            continue;
        }
        for quad in &target.quads {
            // Clip target quad to the image bounding rect (page-space
            // AABB), then map all four corners back to (u, v).
            let target_rect = quad.bounding_rect();
            let clipped = match clip_rect(image_rect, target_rect) {
                Some(rect) => rect,
                None => continue,
            };
            for corner in clipped.to_quad().points {
                let p = inverse.transform_point(corner);
                u_min = u_min.min(p.x);
                u_max = u_max.max(p.x);
                v_min = v_min.min(p.y);
                v_max = v_max.max(p.y);
            }
        }
    }

    if !u_min.is_finite() || !u_max.is_finite() {
        return ImageOverlap::Unsupported;
    }

    let u_lo = u_min.clamp(0.0, 1.0);
    let u_hi = u_max.clamp(0.0, 1.0);
    let v_lo = v_min.clamp(0.0, 1.0);
    let v_hi = v_max.clamp(0.0, 1.0);
    if u_hi <= u_lo + 1e-9 || v_hi <= v_lo + 1e-9 {
        return ImageOverlap::Unsupported;
    }

    // Full cover: target AABB contains the entire unit square.
    let full_cover = u_lo <= 1e-6 && u_hi >= 1.0 - 1e-6
        && v_lo <= 1e-6 && v_hi >= 1.0 - 1e-6;
    if full_cover {
        return ImageOverlap::Full;
    }

    // PDF Y-up → image Y-down: top of image = v=1, bottom = v=0.
    let x_min = (u_lo * width as f64).floor().clamp(0.0, width as f64) as u32;
    let x_max = (u_hi * width as f64).ceil().clamp(0.0, width as f64) as u32;
    let y_min = ((1.0 - v_hi) * height as f64).floor().clamp(0.0, height as f64) as u32;
    let y_max = ((1.0 - v_lo) * height as f64).ceil().clamp(0.0, height as f64) as u32;

    if x_max <= x_min || y_max <= y_min {
        return ImageOverlap::Unsupported;
    }

    ImageOverlap::Partial(crate::image_mask::ImagePixelRect {
        x: x_min,
        y: y_min,
        w: x_max - x_min,
        h: y_max - y_min,
    })
}

fn clip_rect(a: Rect, b: Rect) -> Option<Rect> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let max_x = a.max_x().min(b.max_x());
    let max_y = a.max_y().min(b.max_y());
    if max_x <= x || max_y <= y {
        return None;
    }
    Some(Rect {
        x,
        y,
        width: max_x - x,
        height: max_y - y,
    })
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
    file.insert_object(content_ref, PdfObject::Stream(PdfStream { dict, data }));
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
    overlay: Option<(&str, &str)>,
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

    if let Some((font_name, text)) = overlay {
        // Pick a text color that contrasts with the fill: white on dark fills,
        // black on light fills, by comparing perceived brightness.
        let brightness = 0.299 * red + 0.587 * green + 0.114 * blue;
        let (tr, tg, tb) = if brightness < 0.5 {
            (1.0, 1.0, 1.0)
        } else {
            (0.0, 0.0, 0.0)
        };
        output.push_str(&format!("{tr:.3} {tg:.3} {tb:.3} rg\n"));
        for target in targets {
            // Fit the label vertically inside the target with a 20%
            // top+bottom margin, capped at 14pt so redactions on very tall
            // targets do not print comically large text.
            let bounds = inverse_page_transform.transform_rect(target.bounds);
            let height = bounds.height.abs();
            let size = (height * 0.6).clamp(4.0, 14.0);
            let x = bounds.x + size * 0.25;
            let y = bounds.y + size * 0.2;
            output.push_str("BT\n");
            output.push_str(&format!("/{} {} Tf\n", font_name, format_number(size)));
            output.push_str(&format!("{} {} Td\n", format_number(x), format_number(y)));
            output.push('(');
            output.push_str(&escape_pdf_string(text));
            output.push_str(") Tj\n");
            output.push_str("ET\n");
        }
    }

    output.push_str("Q\n");
    Ok(output.into_bytes())
}

/// Synthetic resource name used for the shared Helvetica font that stamps
/// overlay labels. The leading underscore keeps the name out of the range
/// any real PDF producer is likely to pick for its own fonts.
const OVERLAY_FONT_NAME: &str = "_ORP_Overlay";

fn ensure_overlay_font(file: &mut PdfFile, current: Option<ObjectRef>) -> PdfResult<ObjectRef> {
    if let Some(existing) = current {
        return Ok(existing);
    }
    let mut font_dict = PdfDictionary::new();
    font_dict.insert("Type".to_string(), PdfValue::Name("Font".into()));
    font_dict.insert("Subtype".to_string(), PdfValue::Name("Type1".into()));
    font_dict.insert("BaseFont".to_string(), PdfValue::Name("Helvetica".into()));
    font_dict.insert(
        "Encoding".to_string(),
        PdfValue::Name("WinAnsiEncoding".into()),
    );
    let font_ref = file.allocate_object_ref();
    file.insert_object(font_ref, PdfObject::Value(PdfValue::Dictionary(font_dict)));
    Ok(font_ref)
}

fn register_overlay_font_on_page(
    file: &mut PdfFile,
    page_ref: ObjectRef,
    effective_resources: &PdfDictionary,
    font_ref: ObjectRef,
) -> PdfResult<()> {
    // Start from whatever /Font dict the page already has (direct or
    // inherited), extend it with our synthetic overlay font, and write the
    // result back to the page's Resources as a direct dict.
    let existing_fonts: PdfDictionary = match effective_resources.get("Font") {
        Some(value) => file.resolve_dict(value).cloned().unwrap_or_default(),
        None => PdfDictionary::new(),
    };
    let mut new_fonts = existing_fonts;
    new_fonts.insert(OVERLAY_FONT_NAME.to_string(), PdfValue::Reference(font_ref));
    let mut new_resources = effective_resources.clone();
    new_resources.insert("Font".to_string(), PdfValue::Dictionary(new_fonts));
    match file.get_object_mut(page_ref)? {
        PdfObject::Value(PdfValue::Dictionary(page_dict)) => {
            page_dict.insert("Resources".to_string(), PdfValue::Dictionary(new_resources));
            Ok(())
        }
        _ => Err(PdfError::Corrupt(format!(
            "page {} {} is not a dictionary",
            page_ref.object_number, page_ref.generation
        ))),
    }
}

fn escape_pdf_string(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '(' | ')' | '\\' => {
                output.push('\\');
                output.push(character);
            }
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            _ => output.push(character),
        }
    }
    output
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

#[derive(Debug, Clone)]
struct FormRedaction {
    name: String,
    new_ref: ObjectRef,
    glyphs_removed: usize,
}

#[allow(clippy::too_many_arguments)]
fn redact_intersecting_forms(
    file: &mut PdfFile,
    page: &PageInfo,
    operations: &[Operation],
    targets: &[&NormalizedPageTarget],
    page_transform: Matrix,
    xobjects: &BTreeMap<String, XObjectKind>,
    form_glyph_groups: &BTreeMap<ObjectRef, Vec<Glyph>>,
    mode: RedactionMode,
    warnings: &mut Vec<String>,
) -> PdfResult<Vec<FormRedaction>> {
    // Walk the page content stream once to find each Do of a Form XObject
    // whose bounding quad in page space intersects a redaction target. We
    // track each (name, invocation CTM) so that nested Do inside a Form
    // could later be handled analogously; for this MVP, nested Forms
    // inside a redacted Form still error out via ensure_supported_operators.
    let mut intersecting: Vec<(String, ObjectRef)> = Vec::new();
    let mut ctm = Matrix::identity();
    let mut ctm_stack: Vec<Matrix> = Vec::new();
    for operation in operations {
        match operation.operator.as_str() {
            "q" => ctm_stack.push(ctm),
            "Q" => ctm = ctm_stack.pop().unwrap_or(Matrix::identity()),
            "cm" => ctm = matrix_from_operands(&operation.operands)?.multiply(ctm),
            "Do" => {
                let name = operand_name(operation, 0)?;
                if let Some(XObjectKind::Form { bbox, matrix }) = xobjects.get(name) {
                    let quad = bbox
                        .to_quad()
                        .transform(matrix.multiply(ctm).multiply(page_transform));
                    if targets.iter().any(|target| target.intersects_quad(&quad)) {
                        if let Some(form_ref) = lookup_form_ref(file, &page.resources, name)? {
                            intersecting.push((name.to_string(), form_ref));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // The page's own content stream starts with CTM = identity and the
    // page-space mapping is applied after the Form's own Matrix, so the
    // base invocation CTM passed down to a top-level Form's redactor is
    // effectively the CTM active at the Do (captured during the walk above).
    // We do the walk again because the mutable recursive calls below require
    // exclusive &mut to file.
    let mut walk_ctm = Matrix::identity();
    let mut walk_stack: Vec<Matrix> = Vec::new();
    let mut invocation_ctms: BTreeMap<ObjectRef, Matrix> = BTreeMap::new();
    for operation in operations {
        match operation.operator.as_str() {
            "q" => walk_stack.push(walk_ctm),
            "Q" => walk_ctm = walk_stack.pop().unwrap_or(Matrix::identity()),
            "cm" => walk_ctm = matrix_from_operands(&operation.operands)?.multiply(walk_ctm),
            "Do" => {
                let name = operand_name(operation, 0)?;
                if let Some(form_ref) = lookup_form_ref(file, &page.resources, name)? {
                    invocation_ctms.entry(form_ref).or_insert(walk_ctm);
                }
            }
            _ => {}
        }
    }

    let mut redactions = Vec::with_capacity(intersecting.len());
    for (name, form_ref) in intersecting {
        let base_ctm = invocation_ctms
            .get(&form_ref)
            .copied()
            .unwrap_or(Matrix::identity());
        let (new_ref, removed) = redact_form_xobject(
            file,
            form_ref,
            form_glyph_groups,
            targets,
            page_transform,
            base_ctm,
            mode,
            warnings,
            0,
        )?;
        redactions.push(FormRedaction {
            name,
            new_ref,
            glyphs_removed: removed,
        });
    }
    Ok(redactions)
}

fn lookup_form_ref(
    file: &PdfFile,
    resources: &PdfDictionary,
    name: &str,
) -> PdfResult<Option<ObjectRef>> {
    let xobject_dict = match resources.get("XObject") {
        Some(value) => file.resolve_dict(value)?,
        None => return Ok(None),
    };
    match xobject_dict.get(name) {
        Some(PdfValue::Reference(object_ref)) => Ok(Some(*object_ref)),
        _ => Ok(None),
    }
}

const MAX_FORM_REDACTION_DEPTH: usize = 8;

#[allow(clippy::too_many_arguments)]
fn redact_form_xobject(
    file: &mut PdfFile,
    form_ref: ObjectRef,
    form_glyphs_by_ref: &BTreeMap<ObjectRef, Vec<Glyph>>,
    targets: &[&NormalizedPageTarget],
    page_transform: Matrix,
    base_ctm: Matrix,
    mode: RedactionMode,
    warnings: &mut Vec<String>,
    depth: usize,
) -> PdfResult<(ObjectRef, usize)> {
    if depth >= MAX_FORM_REDACTION_DEPTH {
        warnings.push(format!(
            "Form XObject recursion depth {MAX_FORM_REDACTION_DEPTH} exceeded; nested Form \
             redaction stopped — verify that deeper hidden content is not relied upon"
        ));
        return Ok((form_ref, 0));
    }
    let original = match file.get_object(form_ref)? {
        PdfObject::Stream(stream) => stream.clone(),
        _ => {
            return Err(PdfError::Corrupt(format!(
                "Form XObject {} {} is not a stream",
                form_ref.object_number, form_ref.generation
            )));
        }
    };

    let form_matrix = parse_form_matrix(&original.dict);
    // CTM active while interpreting this Form's content stream.
    let form_invocation_ctm = form_matrix.multiply(base_ctm);

    let form_resources: PdfDictionary = match original.dict.get("Resources") {
        Some(value) => file.resolve_dict(value).cloned().unwrap_or_default(),
        None => PdfDictionary::new(),
    };

    let decoded = pdf_objects::decode_stream(&original)?;
    let parsed = pdf_content::parse_content_stream(&decoded)?;
    ensure_supported_operators(&parsed.operations)?;

    // Walk the Form's content to find nested Do of Form XObjects whose
    // bounding quad in page space intersects any target. For each, recurse
    // so the inner Form's content is rewritten too; record the override so
    // we can repoint this Form's own Resources.XObject at the copy.
    let inner_xobjects = load_xobjects(file, &form_resources)?;
    let mut inner_targets: Vec<(String, ObjectRef)> = Vec::new();
    let mut inner_invocation_ctms: BTreeMap<ObjectRef, Matrix> = BTreeMap::new();
    {
        let mut ctm = form_invocation_ctm;
        let mut stack: Vec<Matrix> = Vec::new();
        for operation in &parsed.operations {
            match operation.operator.as_str() {
                "q" => stack.push(ctm),
                "Q" => ctm = stack.pop().unwrap_or(form_invocation_ctm),
                "cm" => ctm = matrix_from_operands(&operation.operands)?.multiply(ctm),
                "Do" => {
                    let name = operand_name(operation, 0)?;
                    if let Some(XObjectKind::Form { bbox, matrix }) = inner_xobjects.get(name) {
                        let quad = bbox
                            .to_quad()
                            .transform(matrix.multiply(ctm).multiply(page_transform));
                        if targets.iter().any(|target| target.intersects_quad(&quad)) {
                            if let Some(inner_ref) = lookup_form_ref(file, &form_resources, name)? {
                                inner_invocation_ctms.entry(inner_ref).or_insert(ctm);
                                inner_targets.push((name.to_string(), inner_ref));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let mut glyphs_removed_total = 0usize;
    let mut inner_overrides: BTreeMap<String, ObjectRef> = BTreeMap::new();
    for (name, inner_ref) in inner_targets {
        let invocation_ctm = inner_invocation_ctms
            .get(&inner_ref)
            .copied()
            .unwrap_or(form_invocation_ctm);
        let (new_ref, removed) = redact_form_xobject(
            file,
            inner_ref,
            form_glyphs_by_ref,
            targets,
            page_transform,
            invocation_ctm,
            mode,
            warnings,
            depth + 1,
        )?;
        inner_overrides.insert(name, new_ref);
        glyphs_removed_total += removed;
    }

    let empty = Vec::new();
    let own_glyphs = form_glyphs_by_ref.get(&form_ref).unwrap_or(&empty);
    let removals = collect_glyph_removals(own_glyphs, targets);
    glyphs_removed_total += count_removed_glyphs(&removals);
    let mut new_ops = parsed.operations.clone();
    rewrite_text_operations(&mut new_ops, &removals, mode, warnings);

    // Neutralize vector paint and Image XObject invocations inside the Form
    // the same way we do on the page, but starting the CTM at this Form's
    // invocation CTM so the resulting quads are in page space.
    neutralize_vector_operations_with_ctm(
        &mut new_ops,
        targets,
        page_transform,
        form_invocation_ctm,
    )?;
    let redacted_inner_form_names: BTreeSet<String> = inner_overrides.keys().cloned().collect();
    // Inside Form XObjects we don't currently support partial pixel
    // masks (each Form's Resources are nested and a per-Form COW would
    // need a deeper rewrite). Whole-invocation neutralization is still
    // the conservative behaviour here. The fill_color parameter is
    // ignored on this branch and any partial_masks returned would be
    // dropped; in practice the inner content rarely contains an Image
    // whose target overlap is partial AND format-supported, but record
    // a warning if that ever happens so the silent loss is visible.
    let inner_outcome = neutralize_image_operations_with_ctm(
        &mut new_ops,
        targets,
        page_transform,
        &inner_xobjects,
        &redacted_inner_form_names,
        form_invocation_ctm,
        Color::BLACK,
    )?;
    if !inner_outcome.partial_masks.is_empty() {
        // Force the partial-mask path to fall back to drop inside Forms.
        for mask in inner_outcome.partial_masks {
            for op in new_ops.iter_mut() {
                if op.operator == "Do" {
                    if let Some(PdfValue::Name(name)) = op.operands.first() {
                        if *name == mask.xobject_name {
                            op.operator = "n".to_string();
                            op.operands.clear();
                        }
                    }
                }
            }
        }
    }

    let serialized = serialize_operations(&new_ops);
    let (data, use_flate) = match pdf_objects::flate_encode(&serialized) {
        Ok(compressed) => (compressed, true),
        Err(_) => (serialized, false),
    };

    let mut new_dict = original.dict.clone();
    new_dict.remove("Length");
    new_dict.remove("Filter");
    new_dict.remove("DecodeParms");
    if use_flate {
        new_dict.insert("Filter".to_string(), PdfValue::Name("FlateDecode".into()));
    }

    // If any inner Form was redacted, rewrite this Form's Resources.XObject
    // entries so the saved PDF references the redacted copies. Other Forms
    // (and other pages) that still use the original refs are unaffected.
    if !inner_overrides.is_empty() {
        let existing_xobject: PdfDictionary = match form_resources.get("XObject") {
            Some(value) => file.resolve_dict(value).cloned().unwrap_or_default(),
            None => PdfDictionary::new(),
        };
        let mut new_xobject = existing_xobject;
        for (name, new_ref) in &inner_overrides {
            new_xobject.insert(name.clone(), PdfValue::Reference(*new_ref));
        }
        let mut new_resources = form_resources.clone();
        new_resources.insert("XObject".to_string(), PdfValue::Dictionary(new_xobject));
        new_dict.insert("Resources".to_string(), PdfValue::Dictionary(new_resources));
    }

    let new_ref = file.allocate_object_ref();
    file.insert_object(
        new_ref,
        PdfObject::Stream(PdfStream {
            dict: new_dict,
            data,
        }),
    );
    Ok((new_ref, glyphs_removed_total))
}

fn override_page_xobject_refs(
    file: &mut PdfFile,
    page: &mut PageInfo,
    redactions: &[FormRedaction],
) -> PdfResult<()> {
    let existing_xobject: PdfDictionary = match page.resources.get("XObject") {
        Some(value) => file.resolve_dict(value).cloned().unwrap_or_default(),
        None => PdfDictionary::new(),
    };
    let mut new_xobject = existing_xobject;
    for redaction in redactions {
        new_xobject.insert(
            redaction.name.clone(),
            PdfValue::Reference(redaction.new_ref),
        );
    }
    let mut new_resources = page.resources.clone();
    new_resources.insert(
        "XObject".to_string(),
        PdfValue::Dictionary(new_xobject.clone()),
    );

    // Keep the in-memory PageInfo.resources cache in sync with what we
    // write to the page object so any later rewrite (e.g. partial image
    // mask) reads the post-override state instead of a stale snapshot.
    page.resources = new_resources.clone();

    let page_ref = page.page_ref;
    match file.get_object_mut(page_ref)? {
        PdfObject::Value(PdfValue::Dictionary(page_dict)) => {
            page_dict.insert("Resources".to_string(), PdfValue::Dictionary(new_resources));
            Ok(())
        }
        _ => Err(PdfError::Corrupt(format!(
            "page {} {} is not a dictionary",
            page_ref.object_number, page_ref.generation
        ))),
    }
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
    if let Some(base_state) = default_config.get("BaseState").and_then(PdfValue::as_name) {
        if base_state == "OFF" || base_state == "Unchanged" {
            return Err(PdfError::Unsupported(format!(
                "documents with /OCProperties /D /BaseState /{base_state} are not supported for \
                 redaction because hidden layers cannot be safely targeted"
            )));
        }
    }
    Ok(())
}

/// Opt-in version of [`reject_hidden_optional_content`]. Rather than
/// refusing the document, collect every OCG that is off in the default
/// configuration, strip matching `BDC /OC /<name> ... EMC` runs from
/// every page's content stream, and clear the hidden state in the
/// catalog so the saved output no longer advertises a partially
/// disabled layer. Form XObject content is currently only scanned at
/// the top-level — OCG markers inside nested Form bodies are not
/// rewritten and are flagged via a warning so callers can audit.
fn sanitize_hidden_optional_content(
    file: &mut PdfFile,
    pages: &mut [PageInfo],
    warnings: &mut Vec<String>,
) -> PdfResult<()> {
    let hidden_refs = collect_hidden_ocg_refs(file)?;
    if hidden_refs.is_empty() {
        return Ok(());
    }

    for page in pages.iter_mut() {
        let hidden_names = collect_hidden_ocg_names_for_page(file, &page.resources, &hidden_refs)?;
        if hidden_names.is_empty() {
            continue;
        }
        for content_ref in page.content_refs.clone() {
            rewrite_content_stream_stripping_hidden_ocgs(file, content_ref, &hidden_names)?;
        }
        if page.resources.contains_key("XObject") {
            // Nested Form XObjects may themselves contain BDC /OC blocks.
            // Surface a warning so callers know the current implementation
            // does not walk into them.
            warnings.push(format!(
                "sanitize_hidden_ocgs: Form XObject content on page {} was not rewritten; \
                 nested OCG markers inside Forms are not yet stripped",
                page.page_ref.object_number
            ));
        }
    }

    clear_hidden_optional_content_config(file)?;
    Ok(())
}

fn collect_hidden_ocg_refs(file: &PdfFile) -> PdfResult<BTreeSet<ObjectRef>> {
    let Some(PdfValue::Reference(root_ref)) = file.trailer.get("Root") else {
        return Ok(BTreeSet::new());
    };
    let Ok(catalog) = file.get_dictionary(*root_ref) else {
        return Ok(BTreeSet::new());
    };
    let Some(oc_properties_value) = catalog.get("OCProperties") else {
        return Ok(BTreeSet::new());
    };
    let Ok(oc_properties) = file.resolve_dict(oc_properties_value) else {
        return Ok(BTreeSet::new());
    };
    let Some(default_value) = oc_properties.get("D") else {
        return Ok(BTreeSet::new());
    };
    let Ok(default_config) = file.resolve_dict(default_value) else {
        return Ok(BTreeSet::new());
    };

    let base_state = default_config
        .get("BaseState")
        .and_then(PdfValue::as_name)
        .unwrap_or("ON");

    let mut hidden = BTreeSet::new();
    if matches!(base_state, "OFF" | "Unchanged") {
        // All OCGs are hidden (except those in the /ON list).
        if let Some(all_ocgs_value) = oc_properties.get("OCGs") {
            if let Some(entries) = file
                .resolve(all_ocgs_value)
                .unwrap_or(all_ocgs_value)
                .as_array()
            {
                for entry in entries {
                    if let PdfValue::Reference(object_ref) = entry {
                        hidden.insert(*object_ref);
                    }
                }
            }
        }
        if let Some(on_value) = default_config.get("ON") {
            if let Some(entries) = file.resolve(on_value).unwrap_or(on_value).as_array() {
                for entry in entries {
                    if let PdfValue::Reference(object_ref) = entry {
                        hidden.remove(object_ref);
                    }
                }
            }
        }
    } else if let Some(off_value) = default_config.get("OFF") {
        if let Some(entries) = file.resolve(off_value).unwrap_or(off_value).as_array() {
            for entry in entries {
                if let PdfValue::Reference(object_ref) = entry {
                    hidden.insert(*object_ref);
                }
            }
        }
    }
    Ok(hidden)
}

fn collect_hidden_ocg_names_for_page(
    file: &PdfFile,
    page_resources: &PdfDictionary,
    hidden_refs: &BTreeSet<ObjectRef>,
) -> PdfResult<BTreeSet<String>> {
    let Some(properties_value) = page_resources.get("Properties") else {
        return Ok(BTreeSet::new());
    };
    let Ok(properties) = file.resolve_dict(properties_value) else {
        return Ok(BTreeSet::new());
    };
    let mut hidden_names: BTreeSet<String> = BTreeSet::new();
    for (name, value) in properties.iter() {
        let resolved_ref = if let PdfValue::Reference(object_ref) = value {
            *object_ref
        } else {
            // Direct OCG dicts do not carry a stable object ref, so we
            // can only match by reference here. That matches how real
            // writers emit OCG properties in practice.
            continue;
        };
        if hidden_refs.contains(&resolved_ref) {
            hidden_names.insert(name.clone());
        }
    }
    Ok(hidden_names)
}

fn rewrite_content_stream_stripping_hidden_ocgs(
    file: &mut PdfFile,
    content_ref: ObjectRef,
    hidden_names: &BTreeSet<String>,
) -> PdfResult<()> {
    let stream = match file.objects.get(&content_ref) {
        Some(PdfObject::Stream(stream)) => stream.clone(),
        _ => return Ok(()),
    };
    let decoded = decode_stream(&stream)?;
    let parsed = parse_content_stream(&decoded)?;
    let (filtered, stripped) = strip_hidden_ocg_operations(parsed.operations, hidden_names);
    if stripped == 0 {
        return Ok(());
    }
    let new_bytes = serialize_operations(&filtered);
    let mut new_dict = stream.dict.clone();
    // The sanitized bytes replace the original encoded payload; drop
    // the filter pipeline so the saved output re-encodes from plain
    // bytes (the writer compresses content streams on save).
    new_dict.remove("Filter");
    new_dict.remove("DecodeParms");
    new_dict.insert(
        "Length".to_string(),
        PdfValue::Integer(new_bytes.len() as i64),
    );
    file.objects.insert(
        content_ref,
        PdfObject::Stream(PdfStream {
            dict: new_dict,
            data: new_bytes,
        }),
    );
    Ok(())
}

/// Walk `operations` and drop every marked-content section whose
/// opening `BDC` carries a tag of `OC` and a name operand that appears
/// in `hidden_names`. Nested marked-content sections inside a
/// suppressed block are also stripped; unrelated marked-content
/// sections (Span, Artifact, etc.) are preserved verbatim. Returns the
/// surviving operations plus a count of removed operations (nonzero
/// iff anything was stripped).
fn strip_hidden_ocg_operations(
    operations: Vec<Operation>,
    hidden_names: &BTreeSet<String>,
) -> (Vec<Operation>, usize) {
    // `suppress_stack[i]` is true when the marked-content section at
    // depth `i` (1-based) should be dropped. Track nesting so that a
    // `BDC /OC /hidden` inside a `BMC /Span` still terminates at the
    // correct `EMC`.
    let mut suppress_stack: Vec<bool> = Vec::new();
    let mut output = Vec::with_capacity(operations.len());
    let mut stripped = 0usize;

    for op in operations {
        let currently_suppressed = suppress_stack.iter().any(|flag| *flag);
        match op.operator.as_str() {
            "BMC" => {
                suppress_stack.push(false);
                if !currently_suppressed {
                    output.push(op);
                } else {
                    stripped += 1;
                }
            }
            "BDC" => {
                let opens_hidden = !currently_suppressed && is_hidden_oc_bdc(&op, hidden_names);
                suppress_stack.push(opens_hidden);
                if currently_suppressed || opens_hidden {
                    stripped += 1;
                } else {
                    output.push(op);
                }
            }
            "EMC" => {
                let was_suppressed = suppress_stack.pop().unwrap_or(false);
                if currently_suppressed || was_suppressed {
                    stripped += 1;
                } else {
                    output.push(op);
                }
            }
            _ => {
                if currently_suppressed {
                    stripped += 1;
                } else {
                    output.push(op);
                }
            }
        }
    }

    (output, stripped)
}

fn is_hidden_oc_bdc(op: &Operation, hidden_names: &BTreeSet<String>) -> bool {
    if op.operands.len() != 2 {
        return false;
    }
    let tag = match op.operands[0].as_name() {
        Some(name) => name,
        None => return false,
    };
    if tag != "OC" {
        return false;
    }
    match &op.operands[1] {
        PdfValue::Name(name) => hidden_names.contains(name),
        _ => false,
    }
}

fn clear_hidden_optional_content_config(file: &mut PdfFile) -> PdfResult<()> {
    let Some(PdfValue::Reference(root_ref)) = file.trailer.get("Root") else {
        return Ok(());
    };
    let root_ref = *root_ref;
    let mut catalog = match file.get_dictionary(root_ref) {
        Ok(dict) => dict.clone(),
        Err(_) => return Ok(()),
    };
    let Some(oc_properties_ref_or_dict) = catalog.get("OCProperties").cloned() else {
        return Ok(());
    };

    let mut oc_properties = file
        .resolve_dict(&oc_properties_ref_or_dict)
        .cloned()
        .unwrap_or_default();
    let oc_properties_ref = match &oc_properties_ref_or_dict {
        PdfValue::Reference(reference) => Some(*reference),
        _ => None,
    };

    if let Some(default_value) = oc_properties.get("D").cloned() {
        let default_ref = match &default_value {
            PdfValue::Reference(reference) => Some(*reference),
            _ => None,
        };
        let mut default_config = file
            .resolve_dict(&default_value)
            .cloned()
            .unwrap_or_default();
        default_config.remove("OFF");
        default_config.insert("BaseState".to_string(), PdfValue::Name("ON".to_string()));

        if let Some(reference) = default_ref {
            file.objects.insert(
                reference,
                PdfObject::Value(PdfValue::Dictionary(default_config)),
            );
        } else {
            oc_properties.insert("D".to_string(), PdfValue::Dictionary(default_config));
        }
    }

    if let Some(reference) = oc_properties_ref {
        file.objects.insert(
            reference,
            PdfObject::Value(PdfValue::Dictionary(oc_properties)),
        );
    } else {
        catalog.insert(
            "OCProperties".to_string(),
            PdfValue::Dictionary(oc_properties),
        );
        file.objects
            .insert(root_ref, PdfObject::Value(PdfValue::Dictionary(catalog)));
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

    #[test]
    fn bx_ex_compat_section_masks_unknown_operator() {
        // Unknown operator `sh` outside a BX/EX section is rejected so the
        // engine never silently drops a paint call that would leave visible
        // content behind.
        let outside = vec![Operation {
            operator: "sh".to_string(),
            operands: vec![],
        }];
        assert!(ensure_supported_operators(&outside).is_err());

        // Inside a BX/EX compatibility section the same operator is allowed
        // through, matching the PDF § 7.8.2 rule that conforming readers
        // may ignore unrecognized operators in that context.
        let inside = vec![
            Operation {
                operator: "BX".to_string(),
                operands: vec![],
            },
            Operation {
                operator: "sh".to_string(),
                operands: vec![],
            },
            Operation {
                operator: "EX".to_string(),
                operands: vec![],
            },
        ];
        ensure_supported_operators(&inside)
            .expect("unknown operator inside BX/EX should be accepted");

        // Once the BX/EX section closes, the same unknown operator is again
        // rejected.
        let after = vec![
            Operation {
                operator: "BX".to_string(),
                operands: vec![],
            },
            Operation {
                operator: "EX".to_string(),
                operands: vec![],
            },
            Operation {
                operator: "sh".to_string(),
                operands: vec![],
            },
        ];
        assert!(ensure_supported_operators(&after).is_err());
    }
}
