# Open Redact PDF

A browser-first PDF redaction engine built in Rust and compiled to WebAssembly. Operates on PDF structure rather than flattening pages to images, removes targeted content from content streams, and preserves searchable text outside redacted regions.

**[Live demo](https://open-redact-pdf.fabifont.dev)** · **[Documentation](https://fabifont.github.io/open-redact-pdf)**

---

## Key properties

- Redaction removes content from PDF structure — a visible rectangle alone is not a redaction
- Unredacted text stays searchable and selectable where the supported subset allows
- Unsupported features return an explicit error rather than silently corrupting output
- Deterministic full-document rewrite on every save

The [security model](https://fabifont.github.io/open-redact-pdf/security-model/) and [supported PDF subset](https://fabifont.github.io/open-redact-pdf/reference/supported-subset/) are documented in full.

## Quick start

```bash
# Install JS dependencies
just install

# Full build (wasm → ts-sdk → demo)
just build

# Run all tests
just test

# Start the demo dev server
just dev
```

See the [getting started guide](https://fabifont.github.io/open-redact-pdf/getting-started/) for prerequisites and a full walkthrough.

## Packages

| Package | Description |
|---|---|
| [`open-redact-pdf`](https://crates.io/crates/open-redact-pdf) | Rust facade crate with the stable public API |
| [`@fabifont/open-redact-pdf`](https://www.npmjs.com/package/@fabifont/open-redact-pdf) | Typed TypeScript wrapper for the WASM bundle |

## Releases

Push a version tag (e.g. `v0.1.0`) to trigger the release workflow, which publishes both the Rust crates to crates.io and `@fabifont/open-redact-pdf` to npm. Versions must match across `Cargo.toml`, `packages/ts-sdk/package.json`, and the tag.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.
