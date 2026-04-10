---
title: Getting Started
---

# Getting Started

## Prerequisites

- Rust stable
- `wasm32-unknown-unknown`: `rustup target add wasm32-unknown-unknown`
- `wasm-pack`
- Node.js 22+
- `pnpm` 10+

## Install dependencies

```bash
pnpm install
```

## Verify the workspace

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
pnpm wasm:build
pnpm --filter @open-redact-pdf/sdk build
pnpm --filter open-redact-pdf-demo-web build
pnpm --filter open-redact-pdf-demo-web test
```

## Run the demo

```bash
pnpm --filter open-redact-pdf-demo-web dev
```

The demo rebuilds the wasm bundle when the Rust or TS SDK inputs are newer than the generated browser artifacts.

## Minimal Rust example

```rust
use open_redact_pdf::{PdfDocument, RedactionPlan, RedactionTarget};

let bytes = std::fs::read("input.pdf")?;
let mut document = PdfDocument::open(&bytes)?;

let report = document.apply_redactions(RedactionPlan {
    targets: vec![RedactionTarget::Rect {
        page_index: 0,
        x: 72.0,
        y: 500.0,
        width: 120.0,
        height: 18.0,
    }],
    fill_color: None,
    overlay_text: None,
    remove_intersecting_annotations: Some(true),
    strip_metadata: Some(true),
    strip_attachments: Some(true),
})?;

let sanitized = document.save()?;
std::fs::write("sanitized.pdf", sanitized)?;
println!("{report:?}");
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Minimal TypeScript example

```ts
import {
  initWasm,
  openPdf,
  searchText,
  applyRedactions,
  savePdf,
} from "@open-redact-pdf/sdk";

await initWasm();
const handle = openPdf(inputBytes);

const matches = searchText(handle, 0, "account");
applyRedactions(handle, {
  targets: matches.map((match) => ({
    kind: "quadGroup",
    pageIndex: match.pageIndex,
    quads: match.quads,
  })),
  stripMetadata: true,
  stripAttachments: true,
});

const output = savePdf(handle);
```

## Next reading

- [Rust API](reference/rust-api.md)
- [TypeScript and WASM API](reference/ts-sdk.md)
- [Browser integration](guides/browser-integration.md)
