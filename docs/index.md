---
title: Open Redact PDF
---

# Open Redact PDF

Open Redact PDF is a browser-first PDF redaction engine implemented in Rust and exposed to browsers through WebAssembly. The project operates on PDF structure instead of flattening pages into images, removes targeted content for a constrained but real subset of PDFs, and preserves unredacted text where the supported subset allows it.

!!! tip "Live demo"
    [Try the redaction engine in your browser →](https://open-redact-pdf.fabifont.dev){ .md-button .md-button--primary }

## Start here

- [Getting started](getting-started.md)
- [Development workflow](development.md)
- [Publishing and deployment](publishing.md)

## Reference

- [Rust API](reference/rust-api.md)
- [TypeScript and WASM API](reference/ts-sdk.md)
- [Canonical target model](reference/target-model.md)
- [Supported PDF subset and failure model](reference/supported-subset.md)
- [Workspace crate map](reference/workspace-crates.md)

## Design and security

- [Architecture](architecture.md)
- [Security model](security-model.md)
- [Why this is not a canvas overlay tool](why-not-overlays.md)
- [Roadmap](roadmap.md)

## Guides

- [Redaction workflow](guides/redaction-workflow.md)
- [Browser integration](guides/browser-integration.md)
- [Encrypted PDFs](guides/encrypted-pdfs.md)
- [Testing and fixtures](guides/testing-and-fixtures.md)
- [Releasing](guides/releasing.md)

## Engine Internals

Deep technical documentation covering PDF spec concepts, implementation decisions, tradeoffs, and code-level explanations. Start with the reading order guide.

- [Reading order for new contributors](internals/00-reading-order.md)
- [Architecture overview](internals/01-architecture-overview.md)
- [PDF primer](internals/02-pdf-primer.md)
- [Parsing model](internals/03-parsing-model.md)
- [Object model and serialization](internals/04-object-model.md)
- [Graphics state and coordinate systems](internals/05-graphics-state.md)
- [Text system and extraction](internals/06-text-system.md)
- [Search geometry and match modeling](internals/07-search-geometry.md)
- [Redaction target model](internals/08-redaction-targets.md)
- [Redaction application pipeline](internals/09-redaction-pipeline.md)
- [Writer and deterministic output](internals/10-writer.md)
- [WASM/JS boundary design](internals/11-wasm-boundary.md)
- [Security and correctness model](internals/12-security-model.md)
- [Known limitations](internals/13-limitations.md)
- [Glossary](internals/14-glossary.md)
- [PDF spec to code map](internals/15-spec-to-code.md)
- [Top 10 implementation decisions](internals/16-top-ten-decisions.md)

## Current MVP scope

- Unencrypted PDFs, plus Standard Security Handler decryption at V = 1/2 (RC4), V = 4 (AES-128), and V = 5 (AES-256 / R = 5 or R = 6) under either the user or owner password — classic xref tables, PDF 1.5+ cross-reference streams, object streams, and the hybrid `XRefStm` form are all handled
- Unfiltered or `FlateDecode` streams, including PNG and TIFF `DecodeParms` predictors
- Deterministic full-document rewrites with FlateDecode-compressed content streams
- Form XObjects traversed for text extraction, search, and copy-on-write redaction (text, vector paint, and Image `Do` invocations inside the Form), with nested Forms handled recursively
- `Type1`, `TrueType`, and `Type0` / `Identity-H` text with `ToUnicode`, `WinAnsiEncoding`, `MacRomanEncoding`, and `/Encoding /Differences` decoding
- Rectangle, quad, and quad-group redaction targets in canonical page space
- Three redaction modes: `strip`, `redact` (default), and `erase`, with optional `overlayText` labels in `redact` mode
- Conservative image redaction at invocation level
- Hidden-by-default Optional Content Groups are refused by default; callers can opt in via `sanitizeHiddenOcgs: true` to strip `BDC /OC /<name> ... EMC` content gated by hidden layers before redaction

!!! warning "Fail-explicit design"
    Unsupported features return an explicit error instead of being silently ignored or producing incorrect output.
