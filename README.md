# Open Redact PDF

Open Redact PDF is a browser-first PDF redaction engine built in Rust and exposed to the browser through WebAssembly. It works on PDF structure rather than flattening whole pages into images, preserves searchable text outside redacted regions where supported, and removes targeted content inside redactions for a constrained but real subset of unencrypted PDFs.

## Documentation

The repository now includes:

- a root [AGENTS.md](AGENTS.md) file for coding-agent workflow guidance
- a publishable documentation site under [`docs/`](docs/index.md)
- code-level API docs in the Rust facade crate and TS SDK source

Key entry points:

- [Documentation home](docs/index.md)
- [Rust API reference](docs/reference/rust-api.md)
- [TypeScript and WASM API reference](docs/reference/ts-sdk.md)
- [Publishing and Context7 guidance](docs/publishing.md)

## Status

This repository currently targets a deliberately narrow MVP:

- Unencrypted PDFs with classic cross-reference tables
- Unfiltered or `FlateDecode` streams
- Full-document rewrite on save
- Simple page content streams without Form XObjects
- Simple `Type1` and `TrueType` text with horizontal writing
- `Type0` / `Identity-H` text with `ToUnicode` maps and two-byte CIDs
- Rectangle, quad, and quad-group redaction targets in page space
- Conservative image redaction by removing intersecting image draws

Unsupported features fail explicitly instead of being silently ignored.

## Monorepo Layout

- `crates/open_redact_pdf`: Rust facade crate with the stable core API
- `crates/pdf_objects`: low-level PDF object model, parser, and serializer
- `crates/pdf_content`: content stream parsing and page content loading
- `crates/pdf_graphics`: page-space geometry and transforms
- `crates/pdf_text`: text extraction and search geometry
- `crates/pdf_targets`: canonical redaction target model and normalization
- `crates/pdf_redact`: redaction planning and application
- `crates/pdf_writer`: full-save PDF rewrite
- `crates/pdf_wasm`: `wasm-bindgen` browser wrapper
- `packages/ts-sdk`: typed TypeScript wrapper for the wasm package
- `apps/demo-web`: small React demo for manual and search-driven redaction
- `tests/fixtures`: fixture corpus
- `tests/integration`: end-to-end integration tests

## Getting Started

### Prerequisites

- Rust stable
- `wasm32-unknown-unknown` target: `rustup target add wasm32-unknown-unknown`
- `wasm-pack`: `cargo install wasm-pack --locked`
- Node.js 22+
- `pnpm` 10+

### Build

```bash
cargo test --workspace
pnpm install
pnpm wasm:build
pnpm --filter @openredact/ts-sdk build
pnpm --filter demo-web dev
```

## Security Model

Redaction in this project means the output PDF must not retain the removed text in content streams that continue to be referenced by the output file. A visible black rectangle alone does not count as redaction. The current implementation removes intersecting text glyphs, removes intersecting vector paint operations, removes intersecting image draws conservatively, and paints replacement fill marks after content removal.

See [docs/security-model.md](docs/security-model.md), [docs/architecture.md](docs/architecture.md), [docs/roadmap.md](docs/roadmap.md), and [docs/why-not-overlays.md](docs/why-not-overlays.md).
