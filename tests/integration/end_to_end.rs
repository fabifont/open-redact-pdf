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
fn can_strip_metadata_and_attachments() {
    let mut document =
        PdfDocument::open(&fixture("metadata-attachments.pdf")).expect("fixture should open");
    document
        .apply_redactions(RedactionPlan {
            targets: Vec::new(),
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
    if let Some(pdf_objects::PdfValue::Reference(names_ref)) = root.get("Names") {
        let names = parsed
            .file
            .get_dictionary(*names_ref)
            .expect("names dictionary should exist");
        assert!(names.get("EmbeddedFiles").is_none());
    }
}
