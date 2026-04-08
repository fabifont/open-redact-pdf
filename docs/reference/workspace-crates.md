---
title: Workspace Crate Map
---

# Workspace Crate Map

## Public facade

### `open_redact_pdf`

Stable Rust API surface for document open, inspect, search, redact, and save.

## Internal engine crates

### `pdf_objects`

Responsible for:

- parser
- object model
- classic xref loading
- stream storage
- PDF serialization

### `pdf_content`

Responsible for:

- content stream tokenization
- operand parsing
- low-level operation IR
- page content concatenation

### `pdf_graphics`

Responsible for:

- matrices
- points, rects, quads
- page normalization transforms
- geometry helpers

### `pdf_text`

Responsible for:

- font loading for the supported subset
- glyph positioning
- text extraction
- visual-order text search
- search match geometry

### `pdf_targets`

Responsible for:

- canonical redaction targets
- plan normalization
- validation
- page-space bounds

### `pdf_redact`

Responsible for:

- redaction planning
- glyph removal
- vector neutralization
- image invocation removal
- annotation removal
- overlay fill painting

### `pdf_writer`

Responsible for:

- deterministic full-document save
- new xref emission

### `pdf_wasm`

Responsible for:

- wasm-bindgen exports
- serde bridging between JS and Rust types

## JS packages

### `packages/ts-sdk`

Thin typed wrapper around wasm exports. It normalizes raw wasm output into the stable TS types exposed to browser applications.

### `apps/demo-web`

Example browser UI that:

- loads local PDFs
- renders pages with PDF.js
- authors rectangle targets
- compiles search matches into quad-group targets
- applies redactions through the wasm engine
