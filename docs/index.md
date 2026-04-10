---
title: Open Redact PDF Docs
---

# Open Redact PDF

Open Redact PDF is a browser-first PDF redaction engine implemented in Rust and exposed to browsers through WebAssembly. The project operates on PDF structure instead of flattening pages into images, removes targeted content for a constrained but real subset of PDFs, and preserves unredacted text where the supported subset allows it.

## Start here

- [Live demo](https://open-redact-pdf.fabifont.dev) — try the redaction engine in your browser
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

## Engine Internals

Deep technical documentation covering PDF spec concepts, implementation decisions, tradeoffs, and code-level explanations. Start with the reading order guide.

- [Reading order for new contributors](internals/00-reading-order/)
- [Architecture overview](internals/01-architecture-overview/)
- [PDF primer](internals/02-pdf-primer/)
- [Parsing model](internals/03-parsing-model/)
- [Object model and serialization](internals/04-object-model/)
- [Graphics state and coordinate systems](internals/05-graphics-state/)
- [Text system and extraction](internals/06-text-system/)
- [Search geometry and match modeling](internals/07-search-geometry/)
- [Redaction target model](internals/08-redaction-targets/)
- [Redaction application pipeline](internals/09-redaction-pipeline/)
- [Writer and deterministic output](internals/10-writer/)
- [WASM/JS boundary design](internals/11-wasm-boundary/)
- [Security and correctness model](internals/12-security-model/)
- [Known limitations](internals/13-limitations/)
- [Glossary](internals/14-glossary/)
- [PDF spec to code map](internals/15-spec-to-code/)
- [Top 10 implementation decisions](internals/16-top-ten-decisions/)

## Current MVP scope

- Unencrypted PDFs with classic xref tables
- Unfiltered or `FlateDecode` streams
- Deterministic full-document rewrites
- Common page content streams without Form XObjects on targeted pages
- `Type1`, `TrueType`, and `Type0` / `Identity-H` text when extraction can map glyphs safely
- Rectangle, quad, and quad-group redaction targets in canonical page space
- Three redaction modes: `strip`, `redact` (default), and `erase`
- Conservative image redaction at invocation level

Unsupported features fail explicitly instead of being silently ignored.
