---
title: Open Redact PDF Docs
---

# Open Redact PDF

Open Redact PDF is a browser-first PDF redaction engine implemented in Rust and exposed to browsers through WebAssembly. The project operates on PDF structure instead of flattening pages into images, removes targeted content for a constrained but real subset of PDFs, and preserves unredacted text where the supported subset allows it.

## Start here

- [Getting started](getting-started/)
- [Development workflow](development/)
- [Publishing and documentation ingestion](publishing/)

## Reference

- [Rust API](reference/rust-api/)
- [TypeScript and WASM API](reference/ts-sdk/)
- [Canonical target model](reference/target-model/)
- [Supported PDF subset and failure model](reference/supported-subset/)
- [Workspace crate map](reference/workspace-crates/)

## Design and security

- [Architecture](architecture/)
- [Security model](security-model/)
- [Why this is not a canvas overlay tool](why-not-overlays/)
- [Roadmap](roadmap/)

## Guides

- [Redaction workflow](guides/redaction-workflow/)
- [Browser integration](guides/browser-integration/)
- [Testing and fixtures](guides/testing-and-fixtures/)

## Current MVP scope

- Unencrypted PDFs with classic xref tables
- Unfiltered or `FlateDecode` streams
- Deterministic full-document rewrites
- Common page content streams without Form XObjects on targeted pages
- `Type1`, `TrueType`, and `Type0` / `Identity-H` text when extraction can map glyphs safely
- Rectangle, quad, and quad-group redaction targets in canonical page space
- Conservative image redaction at invocation level

Unsupported features fail explicitly instead of being silently ignored.
