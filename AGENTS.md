# AGENTS.md

## Project overview

Open Redact PDF is a browser-first PDF redaction engine implemented in Rust and exposed through WebAssembly.

Core product rules:

- Redaction must operate on PDF structure, not canvas overlays.
- Unredacted text should remain searchable and selectable whenever the supported subset allows it.
- Unsupported features must fail explicitly rather than silently degrade or corrupt output.
- Page-space geometry is the canonical redaction input model. UI tools are adapters, not the engine.

## Repository map

- `crates/open_redact_pdf`: public Rust facade API
- `crates/pdf_objects`: parser, object model, serializer
- `crates/pdf_content`: content stream parsing
- `crates/pdf_graphics`: matrices, bounds, page-space transforms
- `crates/pdf_text`: extraction, visual search order, match geometry
- `crates/pdf_targets`: canonical target model and normalization
- `crates/pdf_redact`: apply pipeline
- `crates/pdf_writer`: deterministic full-save rewrite
- `crates/pdf_wasm`: wasm-bindgen wrapper
- `packages/ts-sdk`: typed TypeScript wrapper
- `apps/demo-web`: React + Vite demo
- `tests/fixtures`: generated fixture corpus
- `docs`: publishable reference and architecture docs

## Setup commands

- Install JS dependencies: `pnpm install`
- Run Rust tests: `cargo test --workspace`
- Run Rust lint: `cargo clippy --workspace --all-targets -- -D warnings`
- Build the wasm bundle: `pnpm wasm:build`
- Build the TS SDK: `pnpm --filter @openredact/ts-sdk build`
- Start the demo: `pnpm --filter demo-web dev`
- Build the demo: `pnpm --filter demo-web build`
- Typecheck the demo: `pnpm --filter demo-web test`

## Working rules

- If you change Rust code that affects browser behavior, rebuild the wasm bundle with `pnpm wasm:build`.
- If you change public Rust or TS APIs, update both the code-level docs and the publishable docs in `docs/`.
- Keep the fixture corpus and integration tests aligned with supported behavior. Do not widen support without tests.
- Preserve the current MVP philosophy: smaller correct support is better than broad fragile support.
- Do not claim support for PDF features unless the parser, extraction path, apply path, and save path all agree.

## Testing instructions

- For engine changes, run `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`.
- For TS SDK or demo changes, run `pnpm --filter @openredact/ts-sdk build`, `pnpm --filter demo-web build`, and `pnpm --filter demo-web test`.
- When redaction behavior changes, prefer adding or updating integration coverage in `tests/integration/end_to_end.rs`.
- Fixture PDFs are generated from `tests/fixtures/generate-fixtures.mjs`; regenerate them when the fixture corpus changes.

## Documentation instructions

- The human-facing docs site lives in `docs/`.
- The root landing page is `docs/index.md`.
- Stable API references live under `docs/reference/`.
- Workflow and integration guides live under `docs/guides/`.
- `docs/publishing.md` describes the GitHub Pages setup and the documentation layout intended for later Context7 ingestion.

## Security considerations

- A visible black rectangle is not a redaction result unless the underlying targeted content is removed or neutralized.
- Never introduce a fallback that rasterizes full pages without documenting it as a deliberate, constrained behavior.
- When partial support is not safe, return an explicit `Unsupported` or `UnsupportedOption` error.

## Commit guidance

- Use short imperative commit messages.
- Keep refactors, behavior fixes, and fixture updates together only when they are directly coupled.
- After public API changes, update `README.md`, `AGENTS.md`, and the relevant docs pages in the same change.
