---
title: Development Workflow
---

# Development Workflow

## Workspace model

The repository is a Rust workspace plus a `pnpm` workspace:

- Rust crates implement parsing, graphics, text extraction, target normalization, redaction, writing, and wasm bindings.
- The TS SDK is a thin typed layer over the generated wasm package.
- The demo uses PDF.js only for preview rendering and pointer interaction. The redaction engine remains in Rust/WASM.

## Recommended workflow

1. Run the Rust checks: `cargo test --workspace`
2. Rebuild wasm when browser-facing behavior changes: `pnpm wasm:build`
3. Rebuild the TS SDK if its source or generated wasm changed: `pnpm --filter @open-redact-pdf/sdk build`
4. Typecheck and build the demo: `pnpm --filter open-redact-pdf-demo-web build`

## When to update docs

Update docs in the same change when you:

- add or remove supported PDF features
- change public Rust or TS API shapes
- change the redaction safety model
- add fixtures that materially expand the tested subset
- change browser integration or build workflow

## Fixtures and tests

- Fixtures live in `tests/fixtures`
- Integration coverage lives in `tests/integration/end_to_end.rs`
- Generated fixtures are maintained by `tests/fixtures/generate-fixtures.mjs`

## Non-negotiable engineering rules

- Do not silently broaden support
- Do not leave visual overlays without removing the underlying targeted content
- Do not introduce page rasterization as a default fallback
- Prefer explicit `Unsupported` errors over ambiguous output

## Related docs

- [Testing and fixtures](guides/testing-and-fixtures.md)
- [Security model](security-model.md)
- [Roadmap](roadmap.md)
