---
title: TypeScript and WASM API Reference
---

# TypeScript and WASM API Reference

The browser-facing API lives in `@open-redact-pdf/sdk`.

## Initialization

### `initWasm(): Promise<void>`

Loads the generated wasm module. Call this once before opening PDFs.

## Document handle

### `openPdf(input: Uint8Array): PdfHandle`

Parses an input PDF and returns an opaque handle used by the rest of the API.

### `getPageCount(handle: PdfHandle): number`

Returns the page count for the parsed PDF.

### `getPageSize(handle: PdfHandle, pageIndex: number): PageSize`

Returns the normalized page-space size.

## Text inspection

### `extractText(handle: PdfHandle, pageIndex: number): PageText`

Returns extracted text and geometry for the requested page.

### `searchText(handle: PdfHandle, pageIndex: number, query: string): TextMatch[]`

Searches text in visual glyph order and returns match geometry as quad arrays.

## Redaction

### `applyRedactions(handle: PdfHandle, plan: RedactionPlan): ApplyReport`

Applies redactions in place to the opened handle.

### `savePdf(handle: PdfHandle): Uint8Array`

Serializes the sanitized PDF as a new byte array.

## Canonical types

### `RectTarget`

```ts
type RectTarget = {
  kind: "rect"
  pageIndex: number
  x: number
  y: number
  width: number
  height: number
}
```

### `QuadTarget`

```ts
type QuadTarget = {
  kind: "quad"
  pageIndex: number
  points: [Point, Point, Point, Point]
}
```

### `QuadGroupTarget`

```ts
type QuadGroupTarget = {
  kind: "quadGroup"
  pageIndex: number
  quads: Array<[Point, Point, Point, Point]>
}
```

### `RedactionPlan`

```ts
type RedactionPlan = {
  targets: RedactionTarget[]
  fillColor?: { r: number; g: number; b: number }
  overlayText?: string | null
  removeIntersectingAnnotations?: boolean
  stripMetadata?: boolean
  stripAttachments?: boolean
}
```

### `TextMatch`

```ts
type TextMatch = {
  text: string
  pageIndex: number
  quads: Array<[Point, Point, Point, Point]>
}
```

## Example

```ts
import {
  initWasm,
  openPdf,
  searchText,
  applyRedactions,
  savePdf,
} from "@open-redact-pdf/sdk"

await initWasm()
const handle = openPdf(bytes)

const matches = searchText(handle, 0, "account")
const targets = matches.map((match) => ({
  kind: "quadGroup" as const,
  pageIndex: match.pageIndex,
  quads: match.quads,
}))

applyRedactions(handle, {
  targets,
  stripMetadata: true,
  stripAttachments: true,
})

const sanitized = savePdf(handle)
```

## Related docs

- [Browser integration](../guides/browser-integration/)
- [Canonical target model](target-model/)
