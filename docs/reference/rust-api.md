---
title: Rust API Reference
---

# Rust API Reference

The stable Rust entry point is the `open_redact_pdf` crate.

## Core types

### `PdfDocument`

Primary document handle for parsing, inspection, redaction, and save.

```rust
pub struct PdfDocument;
```

Methods:

- `PdfDocument::open(bytes: &[u8]) -> PdfResult<PdfDocument>` — opens an unencrypted PDF, or an encrypted PDF whose user password is empty
- `PdfDocument::open_with_password(bytes: &[u8], password: &[u8]) -> PdfResult<PdfDocument>` — opens an encrypted PDF using the supplied password. The password is tried first as the user password, then as the owner password. Returns `PdfError::InvalidPassword` if neither authenticates.
- `PdfDocument::page_count(&self) -> usize`
- `PdfDocument::page_size(&self, page_index: usize) -> PdfResult<PageSize>`
- `PdfDocument::extract_text(&self, page_index: usize) -> PdfResult<PageText>`
- `PdfDocument::search_text(&self, page_index: usize, query: &str) -> PdfResult<Vec<TextMatch>>`
- `PdfDocument::apply_redactions(&mut self, plan: RedactionPlan) -> PdfResult<ApplyReport>`
- `PdfDocument::save(&self) -> PdfResult<Vec<u8>>`

### `PageSize`

```rust
pub struct PageSize {
    pub width: f64,
    pub height: f64,
}
```

Coordinates are expressed in normalized page-space PDF units.

### `PageText`

```rust
pub struct PageText {
    pub page_index: usize,
    pub text: String,
    pub items: Vec<TextItem>,
}
```

`text` is a human-readable page string assembled from extracted text runs. `items` carry geometry for future authoring workflows such as selection-driven redaction.

### `TextItem`

Re-exported from `pdf_text`.

Important fields:

- `text`
- `bbox`
- `quad`
- `char_start`
- `char_end`

### `TextMatch`

Re-exported from `pdf_text`.

Important fields:

- `text`
- `page_index`
- `quads`

Search results are returned as merged visual match regions rather than raw content-stream character order.

### `RedactionMode`

Re-exported from `pdf_targets`.

```rust
pub enum RedactionMode {
    Strip,   // remove bytes; text shifts, no overlay
    Redact,  // blank space + colored overlay (default)
    Erase,   // blank space, no overlay
}
```

### `RedactionPlan`

Re-exported from `pdf_targets`.

Important fields:

- `targets`
- `mode` — controls visual output (see `RedactionMode`)
- `fill_color`
- `overlay_text`
- `remove_intersecting_annotations`
- `strip_metadata`
- `strip_attachments`

### `RedactionTarget`

Re-exported from `pdf_targets`.

Variants:

- `Rect`
- `Quad`
- `QuadGroup`

### `ApplyReport`

Re-exported from `pdf_redact`.

Important counters:

- `pages_touched`
- `text_glyphs_removed`
- `path_paints_removed`
- `image_draws_removed`
- `annotations_removed`
- `form_xobjects_rewritten` — Form XObject per-page copies produced by copy-on-write redaction
- `warnings`

### `PdfError`

Re-exported from `pdf_objects`.

Typical categories in the MVP:

- invalid page index
- parse or corruption errors
- unsupported PDF features
- unsupported options
- invalid password — the supplied password does not authenticate as the user or owner password on an encrypted document

## Example

```rust
use open_redact_pdf::{PdfDocument, RedactionMode, RedactionPlan, RedactionTarget};

let mut document = PdfDocument::open(&bytes)?;
let matches = document.search_text(0, "account")?;

let targets = matches
    .into_iter()
    .map(|match_| RedactionTarget::QuadGroup {
        page_index: match_.page_index,
        quads: match_.quads.into_iter().map(|quad| quad.points).collect(),
    })
    .collect();

document.apply_redactions(RedactionPlan {
    targets,
    mode: Some(RedactionMode::Redact),
    fill_color: None,
    overlay_text: None,
    remove_intersecting_annotations: Some(true),
    strip_metadata: Some(true),
    strip_attachments: Some(true),
})?;

let output = document.save()?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Related docs

- [Canonical target model](../reference/target-model.md)
- [Supported subset](../reference/supported-subset.md)
