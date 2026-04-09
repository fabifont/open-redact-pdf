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
    let mut document = PdfDocument::open(&fixture("type0-search.pdf")).expect("fixture should open");
    let extracted = document.extract_text(0).expect("text extraction should succeed");
    assert!(extracted.text.contains("Secret CID"));

    let matches = document.search_text(0, "cid").expect("search should succeed");
    assert_eq!(matches.len(), 1);
    let quads = matches[0].quads.iter().map(|quad| quad.points).collect::<Vec<_>>();

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
        })
        .expect("redaction should succeed");
    assert!(report.text_glyphs_removed > 0);

    let saved = document.save().expect("save should succeed");
    let reopened = PdfDocument::open(&saved).expect("saved pdf should reopen");
    let extracted_after = reopened.extract_text(0).expect("extraction should still succeed");
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
    let mut document =
        PdfDocument::open(&fixture("incremental-update.pdf")).expect("incremental fixture should open");
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
