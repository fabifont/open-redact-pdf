---
title: Browser Integration
---

# Browser Integration

## Recommended layering

- Use PDF.js or another viewer only for preview rendering and user interaction
- Keep redaction logic in `@fabifont/open-redact-pdf`
- Convert UI events into canonical page-space geometry before calling `applyRedactions`

## Typical flow

1. Load PDF bytes from a file input
2. Call `initWasm()`
3. Open the document with `openPdf(bytes)`
4. Read page sizes and render the preview with a viewer
5. Convert pointer or search results into `RectTarget`, `QuadTarget`, or `QuadGroupTarget`
6. Call `applyRedactions`
7. Call `savePdf`
8. Offer the returned bytes as a download

## Coordinate conversion

The demo converts preview coordinates into page-space using:

- the rendered viewport scale
- the normalized page height
- a bottom-left page origin

Do not pass screen-space pixels directly into the engine.

## Rebuilding browser artifacts

When Rust or wasm-facing code changes:

```bash
pnpm wasm:build
pnpm --filter @fabifont/open-redact-pdf build
pnpm --filter open-redact-pdf-demo-web dev
```
