---
title: TypeScript and WASM API Reference
---

# TypeScript and WASM API Reference

The browser-facing API lives in `@fabifont/open-redact-pdf`.

## Initialization

### `initWasm(): Promise<void>`

Loads the generated wasm module. Call this once before opening PDFs.

## Document handle

### `openPdf(input: Uint8Array): PdfHandle`

Parses an unencrypted PDF, or an encrypted PDF whose user password is empty, and returns an opaque handle used by the rest of the API.

### `openPdfWithPassword(input: Uint8Array, password: string): PdfHandle`

Opens an encrypted PDF using the supplied password. The password is tried first as the user password, then as the owner password; if neither authenticates, the call throws. The password is interpreted as UTF-8 bytes. For unencrypted documents the password is ignored.

### `freePdf(handle: PdfHandle): void`

Releases the memory held by a document handle. Call this before replacing a handle (e.g., when reopening a different file) to avoid leaking wasm heap memory.

### `getPageCount(handle: PdfHandle): number`

Returns the page count for the parsed PDF.

### `getPageSize(handle: PdfHandle, pageIndex: number): PageSize`

Returns the normalized page-space size.

## Text inspection

### `extractText(handle: PdfHandle, pageIndex: number): PageText`

Returns extracted text and geometry for the requested page. Results are cached per-page on the document handle, so repeated `extractText` / `searchText` calls on the same page skip the content-stream walk.

### `searchText(handle: PdfHandle, pageIndex: number, query: string): TextMatch[]`

Searches text in visual glyph order and returns match geometry as quad arrays. Reuses the same per-page cache as `extractText`.

## Redaction

### `applyRedactions(handle: PdfHandle, plan: RedactionPlan): ApplyReport`

Applies redactions in place to the opened handle. The per-page text cache is cleared before the call returns so later `extractText` / `searchText` calls reflect the rewritten content streams.

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

### `RedactionMode`

```ts
type RedactionMode = "strip" | "redact" | "erase"
```

Controls the visual and structural output of text redaction:

- `"strip"` — physically removes the targeted bytes; surrounding text shifts to fill the gap. No overlay is painted.
- `"redact"` — replaces targeted text with blank space and paints a colored overlay over the region. **(default)**
- `"erase"` — replaces targeted text with blank space but paints no overlay, leaving a visible gap.

### `RedactionPlan`

```ts
type RedactionPlan = {
  targets: RedactionTarget[]
  mode?: RedactionMode
  fillColor?: { r: number; g: number; b: number }
  overlayText?: string | null
  removeIntersectingAnnotations?: boolean
  stripMetadata?: boolean
  stripAttachments?: boolean
  sanitizeHiddenOcgs?: boolean
}
```

Setting `sanitizeHiddenOcgs: true` lets redaction run on documents whose catalog declares Optional Content Groups that are off in the default configuration. The engine strips `BDC /OC /<name> ... EMC` content referenced by any hidden OCG from every page before redaction, then clears the catalog's hidden-layer state on save.

### `TextMatch`

```ts
type TextMatch = {
  text: string
  pageIndex: number
  quads: Array<[Point, Point, Point, Point]>
}
```

### `ApplyReport`

```ts
type ApplyReport = {
  pagesTouched: number
  textGlyphsRemoved: number
  pathPaintsRemoved: number
  imageDrawsRemoved: number
  annotationsRemoved: number
  formXObjectsRewritten: number
  warnings: string[]
}
```

`formXObjectsRewritten` counts the per-page Form XObject copies produced by copy-on-write redaction — one per Form whose `BBox × Matrix × CTM` intersected a target.

## Example

```ts
import {
  initWasm,
  openPdf,
  freePdf,
  searchText,
  applyRedactions,
  savePdf,
} from "@fabifont/open-redact-pdf"

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
  mode: "redact",
  stripMetadata: true,
  stripAttachments: true,
})

const sanitized = savePdf(handle)
```

## Related docs

- [Browser integration](../guides/browser-integration.md)
- [Canonical target model](target-model.md)
