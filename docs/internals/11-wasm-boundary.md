# WASM and JavaScript Boundary Design

## 1. Architecture

```
Rust crates (open_redact_pdf, pdf_*)
    ↓ wasm-pack --target bundler
pdf_wasm crate (wasm_api.rs)
    ↓ wasm-bindgen generates
packages/ts-sdk/vendor/pdf-wasm/ (JS glue + .wasm binary)
    ↓ dynamic import()
packages/ts-sdk/src/index.ts
    ↓ workspace dependency
apps/demo-web/
```

## 2. PdfHandle and interior mutability

`PdfHandle` wraps `RefCell<PdfDocument>`. Why `RefCell`?

- `wasm_bindgen` exports take `&PdfHandle` (shared reference)
- But `apply_redactions` needs `&mut` access to modify the document
- `RefCell` provides runtime-checked interior mutability
- This is safe because JS is single-threaded — no concurrent borrows are possible

## 3. Serde boundary

- **Input (JS → Rust)**: `serde_wasm_bindgen::from_value::<RedactionPlan>(plan)` deserializes a JS object into a typed Rust struct
- **Output (Rust → JS)**: `serde_wasm_bindgen::to_value(...)` serializes Rust structs to JS objects
- **Direct types**: `Vec<u8>` becomes `Uint8Array`, `usize` becomes JS number, `String` stays string

## 4. Error handling

All errors become JS strings via `PdfError::to_string()`. There are no structured error objects on the JS side. The TS SDK does not wrap these — they propagate as exceptions thrown from the WASM call.

## 5. TS SDK normalization layer

The WASM layer returns snake_case keys. The TS SDK normalizes these to camelCase:

| WASM key | TS SDK key |
|---|---|
| `page_index` | `pageIndex` |
| `char_start` | `charStart` |
| `text_glyphs_removed` | `textGlyphsRemoved` |
| `{ points: [...] }` quad shape | `[Point, Point, Point, Point]` |

## 6. Memory management

- `freePdf(handle)` calls the generated `.free()` method to release WASM heap memory
- `wasm-bindgen` generates a `FinalizationRegistry` fallback for GC-driven cleanup
- Callers should explicitly call `freePdf` — GC timing is unpredictable and the WASM heap is not managed by the JS GC

## 7. PdfHandle as opaque branded type

In TypeScript: `type PdfHandle = { readonly __brand: "PdfHandle" }`. Callers cannot construct one directly. This prevents accidental misuse such as passing an arbitrary object to SDK functions that expect a loaded PDF handle.

## 8. Per-page extraction cache

`extract_text` and `search_text` consult a `Mutex<HashMap<usize, Arc<ExtractedPageText>>>` on `PdfDocument`. On a miss, the call performs the full parse content stream → load fonts → interpret operators → decode glyphs pipeline once and stores the `Arc` result. Subsequent calls for the same page clone the `Arc` and skip the walk entirely. `apply_redactions` takes `&mut self`, clears the cache, then runs the redaction so post-redaction extractions always reflect the rewritten content stream.

## 9. Why it was coded this way

- **Bundler target** (not `web`/`nodejs`): works with Vite, Webpack, and Rollup without extra configuration
- **`Mutex<HashMap>` cache**: picks up `Send + Sync` cheaply for native callers that might wrap a document in a background task, and has no practical cost in the single-threaded browser where lock contention does not exist
- **Branded type**: type safety at the TS layer without any runtime cost
- **Cache invalidated on `apply_redactions`**: correctness comes first — stale extractions would mis-locate text that has just been removed

## 10. What would break

| Change | Consequence |
|---|---|
| Forgetting to clear the cache in `apply_redactions` | `extract_text` after redaction would return pre-redaction glyphs and mis-route subsequent target geometry |
| Not normalizing snake_case | TS callers receive wrong property names; all field accesses return `undefined` |
| Not calling `freePdf` | Memory leak; WASM heap grows unbounded across PDF loads |
