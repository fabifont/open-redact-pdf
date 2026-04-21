use open_redact_pdf::{PdfDocument, RedactionPlan, RedactionTarget};
use pdf_objects::parse_pdf;

fn fixture(name: &str) -> Vec<u8> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .canonicalize()
        .expect("fixture directory should exist");
    std::fs::read(root.join(name)).expect("fixture should exist")
}

#[test]
fn extracts_text_from_simple_fixture() {
    let document = PdfDocument::open(&fixture("simple-text.pdf")).expect("fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(extracted.text.contains("Secret Alpha"));
    assert!(extracted.text.contains("Beta Gamma"));
}

#[test]
fn rectangle_redaction_removes_target_text_but_preserves_other_text() {
    let mut document = PdfDocument::open(&fixture("simple-text.pdf")).expect("fixture should open");
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::Rect {
                page_index: 0,
                x: 70.0,
                y: 695.0,
                width: 95.0,
                height: 30.0,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted = reopened
        .extract_text(0)
        .expect("reopened text extraction should succeed");
    assert!(!extracted.text.contains("Secret"));
    assert!(extracted.text.contains("Beta Gamma"));

    // Verify that old content stream objects are removed from the output:
    // the raw bytes of the saved PDF must not contain the original unredacted text.
    let raw = String::from_utf8_lossy(&saved);
    assert!(
        !raw.contains("Secret Alpha"),
        "original content stream survived in saved PDF"
    );
}

#[test]
fn search_derived_quads_can_drive_redaction() {
    let mut document = PdfDocument::open(&fixture("simple-text.pdf")).expect("fixture should open");
    let matches = document
        .search_text(0, "Beta Gamma")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("quad redaction should succeed");
    assert!(report.text_glyphs_removed > 0);
    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted = reopened.extract_text(0).expect("extract should succeed");
    assert!(extracted.text.contains("Secret Alpha"));
    assert!(!extracted.text.contains("Beta Gamma"));
}

#[test]
fn type0_fonts_with_tounicode_are_searchable_and_redactable() {
    let mut document =
        PdfDocument::open(&fixture("type0-search.pdf")).expect("fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(extracted.text.contains("Secret CID"));

    let matches = document
        .search_text(0, "cid")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();

    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("extraction should still succeed");
    assert!(extracted_after.text.contains("Secret"));
    assert!(!extracted_after.text.contains("CID"));
}

#[test]
fn can_strip_metadata_and_attachments() {
    let mut document =
        PdfDocument::open(&fixture("metadata-attachments.pdf")).expect("fixture should open");
    document
        .apply_redactions(RedactionPlan {
            targets: Vec::new(),
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(true),
            strip_attachments: Some(true),
            sanitize_hidden_ocgs: None,
        })
        .expect("stripping should succeed");
    let saved = document.save().expect("save should succeed");
    let parsed = parse_pdf(&saved).expect("saved document should parse");
    assert!(!parsed.file.trailer.contains_key("Info"));
    let root_ref = match parsed.file.trailer.get("Root") {
        Some(pdf_objects::PdfValue::Reference(reference)) => *reference,
        _ => panic!("Root reference missing"),
    };
    let root = parsed
        .file
        .get_dictionary(root_ref)
        .expect("catalog should exist");
    // Names key should be removed from catalog after stripping attachments
    assert!(
        root.get("Names").is_none(),
        "Names key should be removed from catalog"
    );
    // Verify that metadata and attachment content are not in the raw output bytes.
    // The Info dictionary contained "Fixture Generator" and "Metadata Fixture".
    let raw = String::from_utf8_lossy(&saved);
    assert!(
        !raw.contains("Fixture Generator"),
        "Info dictionary content survived in saved PDF"
    );
}

#[test]
fn incremental_update_reads_latest_revision_and_redacts() {
    let mut document = PdfDocument::open(&fixture("incremental-update.pdf"))
        .expect("incremental fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(
        extracted.text.contains("Updated Secret"),
        "should see updated text, got: {}",
        extracted.text
    );
    assert!(
        !extracted.text.contains("Original Secret"),
        "should not see original text"
    );

    let matches = document
        .search_text(0, "Updated")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();

    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(!extracted_after.text.contains("Updated"));
    assert!(extracted_after.text.contains("Secret"));
}

#[test]
fn extgstate_font_text_is_extractable_and_redactable() {
    let mut document =
        PdfDocument::open(&fixture("extgstate-font.pdf")).expect("extgstate fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(
        extracted.text.contains("ExtGState Secret"),
        "should extract text set via gs operator, got: {}",
        extracted.text
    );
    assert!(extracted.text.contains("Normal Line"));

    let matches = document
        .search_text(0, "ExtGState")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();

    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(!extracted_after.text.contains("ExtGState"));
    assert!(extracted_after.text.contains("Normal Line"));
}

#[test]
fn inline_image_and_dictionary_operand_pages_are_parseable() {
    let mut document =
        PdfDocument::open(&fixture("inline-image.pdf")).expect("inline-image fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed despite inline image");
    assert!(
        extracted.text.contains("Inline Image Secret"),
        "should extract text before inline image, got: {}",
        extracted.text
    );
    assert!(
        extracted.text.contains("After Image"),
        "should extract text after inline image"
    );

    let matches = document
        .search_text(0, "Inline Image")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();

    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(!extracted_after.text.contains("Inline Image"));
    assert!(extracted_after.text.contains("After Image"));
}

#[test]
fn ocg_hidden_layer_refuses_redaction() {
    // The fixture's catalog declares one Optional Content Group marked as
    // OFF in the default configuration. Redaction must refuse to run on such
    // a document because the hidden layer may carry text that the user never
    // saw and therefore cannot target. Text extraction must still work so
    // callers can inspect what is there.
    let document =
        PdfDocument::open(&fixture("ocg-hidden-layer.pdf")).expect("ocg fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should still succeed");
    assert!(extracted.text.contains("Visible Line"));

    let mut document =
        PdfDocument::open(&fixture("ocg-hidden-layer.pdf")).expect("ocg fixture should open");
    let err = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::Rect {
                page_index: 0,
                x: 70.0,
                y: 695.0,
                width: 95.0,
                height: 30.0,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect_err("redaction must refuse documents with hidden OCGs");
    let message = err.to_string();
    assert!(
        message.contains("Optional Content Groups")
            || message.contains("OCProperties")
            || message.contains("hidden"),
        "error should mention hidden layers, got: {message}"
    );
}

#[test]
fn sanitize_hidden_ocgs_strips_hidden_content_and_allows_redaction() {
    // Fixture: a page with "Visible Line" (outside any OCG) plus
    // "Hidden Secret" inside a `BDC /OC /Hidden ... EMC` block pointing
    // at an OCG that is OFF by default. With `sanitize_hidden_ocgs`
    // set, the hidden block is stripped before redaction runs; the
    // visible line survives, the hidden line is gone, and re-opening
    // the saved PDF confirms the hidden bytes did not make it through.
    let document =
        PdfDocument::open(&fixture("ocg-hidden-content.pdf")).expect("fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should still succeed");
    assert!(extracted.text.contains("Visible Line"));
    assert!(extracted.text.contains("Hidden Secret"));

    let mut document =
        PdfDocument::open(&fixture("ocg-hidden-content.pdf")).expect("fixture should open");
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: Some(true),
        })
        .expect("sanitization should allow redaction to run");
    assert_eq!(report.text_glyphs_removed, 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved PDF should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("extraction should still succeed");
    assert!(extracted_after.text.contains("Visible Line"));
    assert!(
        !extracted_after.text.contains("Hidden Secret"),
        "sanitized output must not contain the previously-hidden OCG text"
    );

    let raw = String::from_utf8_lossy(&saved);
    assert!(
        !raw.contains("Hidden Secret"),
        "Hidden OCG bytes must not survive in the saved PDF"
    );
}

#[test]
fn sanitize_hidden_ocgs_handles_base_state_off() {
    // /BaseState /OFF hides every OCG unless it is explicitly listed
    // under /ON. Verify the sanitization pass recognises that form and
    // still strips the hidden block.
    let mut document =
        PdfDocument::open(&fixture("ocg-base-state-off.pdf")).expect("fixture should open");
    document
        .apply_redactions(RedactionPlan {
            targets: vec![],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: Some(true),
        })
        .expect("sanitization should accept /BaseState /OFF");
    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved PDF should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("extraction should still succeed");
    assert!(extracted_after.text.contains("Visible Line"));
    assert!(!extracted_after.text.contains("Hidden Secret"));
}

#[test]
fn overlay_text_is_stamped_over_redacted_regions() {
    use open_redact_pdf::{FillColor, RedactionMode};

    let mut document = PdfDocument::open(&fixture("simple-text.pdf")).expect("fixture should open");
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::Rect {
                page_index: 0,
                x: 70.0,
                y: 695.0,
                width: 95.0,
                height: 30.0,
            }],
            mode: Some(RedactionMode::Redact),
            fill_color: Some(FillColor { r: 0, g: 0, b: 0 }),
            overlay_text: Some("REDACTED".to_string()),
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction with overlay text should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(!extracted.text.contains("Secret Alpha"));
    // The overlay label is now part of the page's text layer, so text
    // extraction should find it above the redacted region.
    assert!(
        extracted.text.contains("REDACTED"),
        "overlay text should survive save + reopen as page text, got: {}",
        extracted.text
    );
}

#[test]
fn overlay_text_rejected_for_strip_mode() {
    use open_redact_pdf::RedactionMode;

    let mut document = PdfDocument::open(&fixture("simple-text.pdf")).expect("fixture should open");
    let err = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::Rect {
                page_index: 0,
                x: 70.0,
                y: 695.0,
                width: 95.0,
                height: 30.0,
            }],
            mode: Some(RedactionMode::Strip),
            fill_color: None,
            overlay_text: Some("REDACTED".to_string()),
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect_err("strip mode must refuse overlay_text");
    let message = err.to_string();
    assert!(
        message.contains("overlayText"),
        "error should mention overlayText, got: {message}"
    );
}

#[test]
fn encoding_differences_array_resolves_glyph_names() {
    // /Encoding is a dict with /BaseEncoding /WinAnsiEncoding and a
    // /Differences array that remaps byte 0x40 to /AE (Æ) and 0x7B to /fi
    // (the fi ligature). Text extraction must resolve both glyph names
    // through the AGL subset instead of falling back to U+FFFD.
    let document = PdfDocument::open(&fixture("encoding-differences.pdf"))
        .expect("encoding-differences fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(
        extracted.text.contains('Æ'),
        "/AE glyph name should resolve to Æ, got: {}",
        extracted.text
    );
    assert!(
        extracted.text.contains('\u{FB01}'),
        "/fi glyph name should resolve to U+FB01 (ﬁ), got: {}",
        extracted.text
    );
    assert!(
        !extracted.text.contains('\u{FFFD}'),
        "no replacement character should appear, got: {}",
        extracted.text
    );
    // Characters that are NOT in Differences still decode via the base WinAnsi
    // table — the space byte 0x20 remains a space, and the ASCII word stays
    // intact.
    assert!(extracted.text.contains("nice"));
}

#[test]
fn vector_v_and_y_curve_segments_are_included_in_path_bounds() {
    // The fixture draws a filled curved shape built from v and y Bezier
    // shorthands directly under "Curve Secret". A search-driven redaction
    // that hits the text must also neutralize the underlying path.
    let mut document =
        PdfDocument::open(&fixture("vector-vy-curves.pdf")).expect("vector-vy fixture should open");
    let matches = document
        .search_text(0, "Curve Secret")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert!(report.text_glyphs_removed > 0);
    assert!(
        report.path_paints_removed > 0,
        "v/y curve segments should be included in path bounds so the fill is neutralized"
    );

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let raw = String::from_utf8_lossy(&saved);
    assert!(!raw.contains("Curve Secret"));
    let _ = reopened; // ensure save round-trips cleanly
}

#[test]
fn form_xobject_text_is_extracted_and_searchable() {
    let document = PdfDocument::open(&fixture("form-xobject-text.pdf"))
        .expect("form-xobject fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(
        extracted.text.contains("Page Outer"),
        "outer page text should still be extracted, got: {}",
        extracted.text
    );
    assert!(
        extracted.text.contains("Form Inner Secret"),
        "Form XObject text should be extracted, got: {}",
        extracted.text
    );

    // Search should locate the Form XObject text and produce a valid quad.
    let matches = document
        .search_text(0, "Form Inner Secret")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    assert!(!matches[0].quads.is_empty());

    // The quad should land inside the page bounds (612 x 792). The Form's
    // Matrix translates content by +100 in y, and the page content stream
    // places the Form origin at (72, 400), so the glyphs sit near y=500.
    let bbox = matches[0].quads[0].bounding_rect();
    assert!(
        bbox.x >= 0.0 && bbox.max_x() <= 612.0 && bbox.y >= 0.0 && bbox.max_y() <= 792.0,
        "quad should be within page bounds, got bbox: {:?}",
        bbox
    );
}

#[test]
fn form_xobject_does_not_block_redaction_when_target_is_on_page_content() {
    // Redacting the outer page text must succeed even though the page also
    // invokes a Form XObject — the Form does not overlap the target so the
    // engine should leave the Do alone and proceed with the rewrite.
    let mut document = PdfDocument::open(&fixture("form-xobject-text.pdf"))
        .expect("form-xobject fixture should open");
    let matches = document
        .search_text(0, "Page Outer")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction of outer text should succeed even with a Form present");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(!extracted_after.text.contains("Page Outer"));
    assert!(
        extracted_after.text.contains("Form Inner Secret"),
        "Form XObject content should remain untouched since it did not intersect the target"
    );
}

#[test]
fn nested_form_xobject_redaction_recurses_into_inner_forms() {
    // form-xobject-nested.pdf is three layers deep: page → FmOuter → FmInner.
    // The targeted text "Nested Secret" lives in FmInner. The engine must
    // walk through both Forms, rewrite the inner Form's content, then
    // repoint the outer Form's own Resources.XObject at the redacted
    // inner copy.
    let mut document = PdfDocument::open(&fixture("form-xobject-nested.pdf"))
        .expect("nested form fixture should open");
    let extracted_before = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(extracted_before.text.contains("Nested Secret"));
    assert!(extracted_before.text.contains("Middle Layer"));
    assert!(extracted_before.text.contains("Page Outer"));

    let matches = document
        .search_text(0, "Nested Secret")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("nested Form redaction should succeed");
    assert!(report.text_glyphs_removed > 0);
    assert!(report.form_xobjects_rewritten >= 1);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(
        !extracted_after.text.contains("Nested Secret"),
        "inner-Form text should be gone after save, got: {}",
        extracted_after.text
    );
    assert!(
        extracted_after.text.contains("Page Outer"),
        "outermost text should survive"
    );
    assert!(
        extracted_after.text.contains("Middle Layer"),
        "middle Form text should survive (it was not targeted)"
    );
}

#[test]
fn form_xobject_redaction_rewrites_the_form_content_stream() {
    // The Form XObject in this fixture carries "Form Inner Secret".
    // Redacting that string now succeeds: the engine allocates a per-page
    // copy of the Form, rewrites its content stream to strip the targeted
    // glyphs, and updates the page's Resources.XObject to point at the
    // copy. The page text "Page Outer" (which lives in the page's own
    // content stream) must survive untouched.
    let mut document = PdfDocument::open(&fixture("form-xobject-text.pdf"))
        .expect("form-xobject fixture should open");
    let matches = document
        .search_text(0, "Form Inner Secret")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction inside a Form XObject should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(
        !extracted.text.contains("Form Inner Secret"),
        "Form-interior text should be gone after save, got: {}",
        extracted.text
    );
    assert!(
        extracted.text.contains("Page Outer"),
        "outer page text should remain untouched, got: {}",
        extracted.text
    );
}

#[test]
fn winansi_encoded_simple_font_decodes_non_ascii_bytes() {
    let mut document =
        PdfDocument::open(&fixture("winansi-font.pdf")).expect("winansi fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(
        extracted.text.contains("CaffÉ"),
        "É byte (0xC9) should decode under WinAnsi, got: {}",
        extracted.text
    );
    assert!(
        extracted.text.contains("50°"),
        "° byte should decode under WinAnsi, got: {}",
        extracted.text
    );
    assert!(
        extracted.text.contains("€"),
        "€ byte (0x80) should decode under WinAnsi, got: {}",
        extracted.text
    );
    assert!(
        extracted.text.contains("l’anno"),
        "’ byte (0x92) should decode to U+2019, got: {}",
        extracted.text
    );
    assert!(
        !extracted.text.contains('\u{FFFD}'),
        "no replacement characters should appear, got: {}",
        extracted.text
    );

    // Search + redact should still work end-to-end on the decoded text.
    let matches = document
        .search_text(0, "50°")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(!extracted_after.text.contains("50°"));
    assert!(extracted_after.text.contains("CaffÉ"));
}

#[test]
fn xref_stream_and_object_stream_fixture_is_parseable_and_redactable() {
    let mut document = PdfDocument::open(&fixture("xref-object-stream.pdf"))
        .expect("xref+ObjStm fixture should open");
    assert_eq!(document.page_count(), 1);

    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed on an ObjStm-backed page tree");
    assert!(
        extracted.text.contains("OBJSTM Secret"),
        "should extract text whose Page dictionary lives inside an ObjStm, got: {}",
        extracted.text
    );
    assert!(extracted.text.contains("Beta Gamma"));

    let matches = document
        .search_text(0, "OBJSTM Secret")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();

    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed on an ObjStm-backed page tree");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(!extracted_after.text.contains("OBJSTM Secret"));
    assert!(extracted_after.text.contains("Beta Gamma"));

    // The saved output is a flat classic-xref rewrite; the raw bytes must not
    // contain the original targeted text.
    let raw = String::from_utf8_lossy(&saved);
    assert!(
        !raw.contains("OBJSTM Secret"),
        "original content stream survived in saved PDF"
    );
}

#[test]
fn nested_cm_operators_produce_page_space_quads() {
    let mut document =
        PdfDocument::open(&fixture("nested-cm.pdf")).expect("nested-cm fixture should open");
    let extracted = document
        .extract_text(0)
        .expect("text extraction should succeed");
    assert!(
        extracted.text.contains("Nested CM Secret"),
        "should extract text from inner cm block, got: {}",
        extracted.text
    );
    assert!(extracted.text.contains("Outer Text"));

    // Search-derived quads must be within page bounds (612x792)
    let matches = document
        .search_text(0, "Nested CM")
        .expect("search should succeed");
    assert_eq!(matches.len(), 1);
    for quad in &matches[0].quads {
        let bbox = quad.bounding_rect();
        assert!(
            bbox.x >= -1.0 && bbox.max_x() <= 613.0 && bbox.y >= -1.0 && bbox.max_y() <= 793.0,
            "quad should be within page bounds, got bbox: {:?}",
            bbox
        );
    }

    // Quads should also drive a successful redaction
    let quads = matches[0]
        .quads
        .iter()
        .map(|quad| quad.points)
        .collect::<Vec<_>>();
    let report = document
        .apply_redactions(RedactionPlan {
            targets: vec![RedactionTarget::QuadGroup {
                page_index: 0,
                quads,
            }],
            mode: None,
            fill_color: None,
            overlay_text: None,
            remove_intersecting_annotations: Some(false),
            strip_metadata: Some(false),
            strip_attachments: Some(false),
            sanitize_hidden_ocgs: None,
        })
        .expect("redaction should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened
        .extract_text(0)
        .expect("reopened extraction should succeed");
    assert!(!extracted_after.text.contains("Nested CM"));
    assert!(extracted_after.text.contains("Outer Text"));
}
