# PDF Spec Concepts Mapped to Code

This page is a quick-reference index mapping PDF specification concepts to their implementation locations in this repository. Use it when reading the spec alongside the source, or when tracing a bug back to the relevant code.

Spec references use ISO 32000-1:2008 section numbers.

---

## Concept Map

| PDF Concept | Spec Reference | Repository Location | Key Types / Functions |
|---|---|---|---|
| File structure | ISO 32000 7.5 | `crates/pdf_objects/src/parser.rs` | `parse_pdf`, `parse_header`, `find_startxref` |
| Cross-reference table | ISO 32000 7.5.4 | `crates/pdf_objects/src/parser.rs` | `parse_xref_table`, `parse_xref_section`, `XrefEntry` |
| Incremental updates | ISO 32000 7.5.6 | `crates/pdf_objects/src/parser.rs` | `parse_xref_table` (Prev chain loop) |
| Indirect objects | ISO 32000 7.3.10 | `crates/pdf_objects/src/types.rs` | `ObjectRef`, `PdfObject`, `PdfFile::get_object` |
| Object types | ISO 32000 7.3 | `crates/pdf_objects/src/types.rs` | `PdfValue` enum |
| Stream objects | ISO 32000 7.3.8 | `crates/pdf_objects/src/types.rs`, `stream.rs` | `PdfStream`, `decode_stream` |
| FlateDecode | ISO 32000 7.4.4 | `crates/pdf_objects/src/stream.rs` | `decode_stream` (inflate) |
| Document catalog | ISO 32000 7.7.2 | `crates/pdf_objects/src/document.rs` | `DocumentCatalog`, `build_document` |
| Page tree | ISO 32000 7.7.3 | `crates/pdf_objects/src/document.rs` | `collect_pages`, `PageInfo` |
| Page objects | ISO 32000 7.7.3.3 | `crates/pdf_objects/src/document.rs` | `PageInfo { page_ref, resources, page_box, content_refs }` |
| MediaBox / CropBox | ISO 32000 7.7.3.3 | `crates/pdf_graphics/src/geometry.rs` | `PageBox { media_box, crop_box, rotate }` |
| Coordinate systems | ISO 32000 8.3 | `crates/pdf_graphics/src/geometry.rs` | `Matrix`, `transform_point`, `PageBox::normalized_transform` |
| CTM | ISO 32000 8.3.4 | `crates/pdf_text/src/text.rs`, `crates/pdf_redact/src/redact.rs` | `ctm` variable, `cm` operator handler |
| cm operator | ISO 32000 8.4.4 | `crates/pdf_text/src/text.rs` | `cm` handler: `ctm = matrix.multiply(ctm)` |
| Graphics state stack | ISO 32000 8.4.2 | `crates/pdf_text/src/text.rs` | `q`/`Q` handlers, `ctm_stack` |
| Content streams | ISO 32000 7.8.2 | `crates/pdf_content/src/content.rs` | `parse_content_stream`, `Operation` |
| Text state | ISO 32000 9.3 | `crates/pdf_text/src/text.rs` | `RuntimeTextState` |
| Text-showing operators | ISO 32000 9.4.3 | `crates/pdf_text/src/text.rs` | `show_text` function |
| BT/ET | ISO 32000 9.4.1 | `crates/pdf_text/src/text.rs` | `BT` handler (resets matrices only) |
| Text matrix (Tm) | ISO 32000 9.4.2 | `crates/pdf_text/src/text.rs` | `text_state.text_matrix` |
| Text rendering mode | ISO 32000 9.3.6 | `crates/pdf_text/src/text.rs` | `text_state.text_render_mode`, `visible` flag |
| Glyph positioning | ISO 32000 9.4.4 | `crates/pdf_text/src/text.rs` | `show_text` advance computation |
| TJ operator | ISO 32000 9.4.3 | `crates/pdf_text/src/text.rs`, `crates/pdf_redact/src/redact.rs` | `TJ` handler, `build_compensated_array` |
| Simple fonts | ISO 32000 9.6 | `crates/pdf_text/src/text.rs` | `SimpleFont`, `load_single_font` |
| Type0 fonts | ISO 32000 9.7 | `crates/pdf_text/src/text.rs` | `CompositeFont`, `load_composite_font` |
| ToUnicode | ISO 32000 9.10.3 | `crates/pdf_text/src/text.rs` | `parse_to_unicode_cmap` |
| CID widths | ISO 32000 9.7.4.3 | `crates/pdf_text/src/text.rs` | `parse_cid_widths` |
| ExtGState | ISO 32000 8.4.5 | `crates/pdf_text/src/text.rs` | `ExtGStateFontMap`, `gs` operator handler |
| Inline images | ISO 32000 8.9.7 | `crates/pdf_content/src/content.rs` | `skip_inline_image` |
| XObjects | ISO 32000 8.8 | `crates/pdf_redact/src/redact.rs` | `load_xobjects`, `XObjectKind` |
| Annotations | ISO 32000 12.5 | `crates/pdf_redact/src/redact.rs` | `remove_annotations` |
| Path construction | ISO 32000 8.5.2 | `crates/pdf_content/src/content.rs` | `PathSegment` enum |
| Path painting | ISO 32000 8.5.3 | `crates/pdf_content/src/content.rs` | `PaintOperator` enum |
| Marked content | ISO 32000 14.6 | `crates/pdf_content/src/content.rs` | `BMC`/`BDC`/`EMC` passthrough |

---

## Concepts This Engine Does NOT Implement

The following PDF features are explicitly out of scope. Encountering any of these in a document will result in an explicit error or silent passthrough, depending on the subsystem. No partial or best-effort support is provided.

- **Encryption** (ISO 32000 7.6) — standard security handler, public-key security handler, and crypt filters are all unsupported. Any `Encrypt` key in the trailer causes rejection.
- **Digital signatures** (ISO 32000 12.8) — signature fields and signature dictionaries are not validated or preserved in any meaningful way.
- **Optional content / layers** (ISO 32000 8.11) — optional content groups (OCGs) are inspected at the catalog level to reject documents with hidden-by-default layers. Callers can opt in via `sanitize_hidden_ocgs` to strip `BDC /OC /<name> ... EMC` content gated by hidden OCGs before redaction. Optional content membership dictionaries (OCMDs) are not yet tracked — pages that rely on them for visibility fall back to the rejection path.
- **JavaScript actions** (ISO 32000 12.6.4.16) — no JavaScript engine is present. JavaScript actions are passed through unexamined.
- **Forms / AcroForms** (ISO 32000 12.7) — interactive form fields, widget annotations, and form data are not extracted, filled, or redacted at the field level.
- **Multimedia** (ISO 32000 13) — sound, video, 3D, and rich media annotations are not interpreted.
- **Transparency and blending** (ISO 32000 11) — transparency groups, soft masks, and blend modes are parsed as content operators but their effect on rendering is not modeled.
- **Type3 fonts** (ISO 32000 9.6.5) — glyph procedures defined as content streams are not executed.
- **CFF / OpenType fonts** — CFF (Compact Font Format) embedded font programs are not parsed for advance widths or outlines.
- **Pattern and shading fills** (ISO 32000 8.7) — tiling patterns and shading patterns are passed through uninterpreted; their geometry is not included in path bounds calculations.
