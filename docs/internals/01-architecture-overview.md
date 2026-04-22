# Architecture Overview

This document is the 30,000-foot view of the Open Redact PDF engine. It explains what each crate is responsible for, how data flows through the system from raw bytes to a redacted output file, the design principles that constrain every implementation decision, and the error model.

Read **02-pdf-primer.md** first if you have not already. Read **15-spec-to-code.md** afterward to map the concepts here to specific Rust types and functions.

---

## Crate dependency graph

Each crate has a single responsibility. Dependencies only flow downward: no crate may import from a crate above it in this graph.

```
pdf_graphics          (geometry primitives, no PDF-specific deps)
      ↑
pdf_objects           (parser, object model, serializer; + flate2 for stream decompression)
      ↑
pdf_content ──────────────────────────┐
      ↑                               ↑
pdf_targets                      pdf_text
      ↑                               ↑
      └──────────── pdf_redact ───────┘
                        ↑
                   pdf_writer
                        ↑
               open_redact_pdf        (public Rust facade)
                        ↑
                    pdf_wasm          (wasm-bindgen wrapper)
                        ↑
                    TS SDK            (packages/ts-sdk)
                        ↑
                   Demo App           (apps/demo-web)
```

### Crate responsibilities

**pdf_graphics** — Matrices (the current transformation matrix and text matrix), bounding box types, quad types, and page-space coordinate arithmetic. No PDF object knowledge. No I/O.

**pdf_objects** — Tokenizer, parser, xref resolution (including incremental update chains via `Prev` pointers), object model (`PdfObject`, `PdfDict`, `PdfArray`, `PdfStream`), stream decompression (FlateDecode via flate2), and the serializer that writes objects back to bytes. The output of the parser is a `ParsedDocument`.

**pdf_content** — Content stream operator parser. Reads a decompressed stream byte sequence and produces a sequence of `ContentOperator` values with their operands. Handles inline images, dictionary operands, and the full operand stack. Does not interpret operators; that is the job of higher crates.

**pdf_text** — Interprets content stream operators in the context of a graphics state and font map to extract glyph positions. Implements reading order sort, text normalization, and substring matching mapped back to glyph quads. The two primary entry points are `analyze_page_text()` and `search_page_text()`.

**pdf_targets** — Defines the `NormalizedPageTarget` type and the `normalize_plan()` function that validates and normalizes the caller-supplied redaction targets (Rect, Quad, QuadGroup) into verified page-space quads.

**pdf_redact** — The apply pipeline. Intersects normalized targets with extracted glyphs, vector paths, and image XObjects. Rewrites content streams to remove or neutralize matched content. Strips annotations, removes metadata fields. Entry point: `apply_redactions()`.

**pdf_writer** — Produces a single-revision, deterministic output file from a modified document tree. Calls `pdf_objects::serialize_pdf()`. No incremental updates are written; every save is a full rewrite.

**open_redact_pdf** — The public Rust API surface. Wraps the lower crates behind `PdfDocument`, `RedactionPlan`, and `RedactionResult`. This is the only crate external Rust callers should import.

**pdf_wasm** — wasm-bindgen wrapper around `open_redact_pdf`. Handles serialization of Rust types across the WASM boundary and maps `PdfError` to JavaScript exceptions. See **11-wasm-boundary.md**.

---

## Pipeline overview

A complete redaction pass moves through six stages. Each stage has a well-defined input type and output type. Stages are not cached; every call re-executes the full chain from that stage forward.

### Stage 1: Parse

**Entry point:** `pdf_objects::parse_pdf(bytes: &[u8])`

**What happens:** The tokenizer scans the byte slice to locate the startxref offset. The xref parser resolves the cross-reference table, following `Prev` pointers to reconstruct incremental update chains. Object entries are read lazily on demand. The catalog and page tree are walked to build a flat page list.

**Output:** `ParsedDocument { file: PdfFile, catalog: PdfDictRef, pages: Vec<PageRef> }`

### Stage 2: Extract

**Entry point:** `pdf_text::analyze_page_text(file: &PdfFile, page_index: usize, page: &PageInfo)`

**What happens:** The content stream for the requested page is decompressed and parsed into a `ParsedPageContent` by `pdf_content`. A graphics state machine interprets the operators, maintaining the current transformation matrix, text matrix, font, font size, and text state parameters. For each glyph-placing operator (`Tj`, `TJ`, `'`, `"`, etc.), the current state is used to compute the glyph's bounding quad in page space. Font encoding and ToUnicode CMaps are consulted to decode each glyph to a Unicode character.

**Output:** `ExtractedPageText { text: String, items: Vec<TextItem>, glyphs: Vec<Glyph> }`

### Stage 3: Search

**Entry point:** `pdf_text::search_page_text(extracted, query)`

**What happens:** Glyphs are sorted into visual display order (left-to-right, top-to-bottom in page space). The concatenated character sequence is normalized (whitespace collapsing, Unicode normalization). The query string is matched against the normalized sequence. Each match is mapped back through the normalization index to the original `GlyphQuad` slice, yielding a set of quads in page space.

**Output:** `Vec<SearchMatch>` where each match carries the matched text and its page-space quads.

### Stage 4: Target normalization

**Entry point:** `pdf_targets::normalize_plan(plan: RedactionPlan, page_sizes: &[pdf_graphics::Size])`

**What happens:** The caller-supplied `RedactionPlan` contains one or more targets per page. Each target is one of: a page-space `Rect`, a `Quad` (four corner points), or a `QuadGroup` (a search match result). Each target is validated (non-degenerate, within page bounds) and converted to one or more `NormalizedPageTarget` values holding verified quads.

**Output:** `PdfResult<NormalizedRedactionPlan>` (wraps a `Vec<NormalizedPageTarget>` plus mode, color, and flag fields)

### Stage 5: Redact

**Entry point:** `pdf_redact::apply_redactions(file: &mut PdfFile, pages: &mut [PageInfo], plan: &NormalizedRedactionPlan)`

**What happens:** For each normalized target quad, the pipeline intersects the quad against:
- **Glyphs** — any `GlyphQuad` whose bounds overlap the target is removed from the content stream.
- **Vector paths** — path construction and painting operators whose bounding boxes overlap the target are removed.
- **Image XObjects** — images whose bounds overlap the target are removed or their stream replaced with a zero-byte placeholder.

The content stream is rewritten with the matched operators removed or replaced. Annotation objects covering the target area are deleted from the page's `/Annots` array. Document-level metadata fields (Author, Subject, Keywords, Producer, Creator) are cleared from the Info dictionary and the XMP metadata stream.

**Output:** The modified `ParsedDocument` (mutated in place).

### Stage 6: Write

**Entry point:** `pdf_writer::save_document(doc)` → `pdf_objects::serialize_pdf(doc)`

**What happens:** The full document is serialized from scratch as a single-revision PDF. Object offsets are computed sequentially. A new xref table is written. The trailer dictionary is updated. No compression is applied to newly written content streams (existing compressed streams are preserved as-is if they were not modified). Output is deterministic: given the same document tree, the serializer produces byte-for-byte identical output.

**Output:** `Vec<u8>` — the complete redacted PDF file.

---

## Data flow diagram

```
Caller supplies bytes
        │
        ▼
 pdf_objects::parse_pdf()
        │
        ▼
  ParsedDocument
        │
        ├──────────────────────────────────────────┐
        │                                          │
        ▼                                          ▼
pdf_text::analyze_page_text()          (stored for write-back)
        │
        ▼
  ExtractedPageText
        │
        ▼
pdf_text::search_page_text()   ◄── query string from caller
        │
        ▼
  Vec<SearchMatch>
        │
        ▼
pdf_targets::normalize_plan()  ◄── RedactionPlan from caller
        │
        ▼
  NormalizedRedactionPlan
        │
        ▼
pdf_redact::apply_redactions() ◄── mutates ParsedDocument
        │
        ▼
  ParsedDocument (modified)
        │
        ▼
pdf_writer::save_document()
        │
        ▼
  Vec<u8>  →  caller receives redacted PDF bytes
```

---

## Design principles

These principles are not preferences; they are constraints that existing code enforces and new code must preserve.

### Fail explicitly

Unsupported PDF features must return a typed error. The engine never silently ignores an operator, object, or option it does not understand when that ignorance could affect redaction correctness. `PdfError::Unsupported` and `PdfError::UnsupportedOption` exist precisely for this.

### Structural redaction

A black rectangle drawn over text is not a redaction. The engine must remove or neutralize the underlying content bytes. The security model document (**12-security-model.md**) explains the threat model in detail.

### Deterministic output

The serializer uses `BTreeMap` everywhere object ordering matters, processes pages and objects in stable index order, writes a single xref table (no incremental updates), and applies no compression to newly generated streams. Given identical input and targets, the output bytes are identical. This makes testing and diffing tractable.

### MVP scope

Small, correct support is better than broad, fragile support. Every supported feature must be covered end-to-end: parser, extraction path, apply path, and write path. Features where any one of these is missing or uncertain are treated as unsupported until all four are solid.

### Page-space geometry as the canonical model

All redaction targets are expressed in page space (the coordinate system defined by the page's MediaBox, with the origin at the bottom-left corner and units in points). UI tools, search results, and caller-supplied rects are all adapters that must translate into page space. The engine does not accept screen pixels or percentages.

---

## Error model

All errors are variants of `PdfError`. The variants are designed so that callers can distinguish recoverable caller errors from engine bugs from unsupported-but-valid PDF features.

| Variant | Meaning |
|---------|---------|
| `PdfError::Parse` | The input bytes are not valid PDF syntax. The file may be truncated, encrypted without a supported scheme, or otherwise malformed at the token level. |
| `PdfError::Corrupt` | The bytes are syntactically valid PDF but structurally invalid: a required dictionary key is missing, an object reference points to a non-existent object, the page tree is malformed. |
| `PdfError::Unsupported` | The file uses a valid PDF feature that this engine does not implement. The file is not corrupt; the engine simply does not support this feature yet. |
| `PdfError::InvalidPageIndex` | The caller requested a page index that does not exist in the document. |
| `PdfError::MissingObject` | An indirect reference within the document tree could not be resolved. This usually indicates a corrupt file, but is distinct from `Corrupt` because it surfaces at object-resolution time rather than parse time. |
| `PdfError::UnsupportedOption` | The caller passed an option or configuration value that the engine does not implement. The input is valid; the option is simply not supported. |

Errors propagate as `Result<T, PdfError>` throughout the Rust API. At the WASM boundary, `PdfError` is mapped to a JavaScript `Error` with a structured message; see **11-wasm-boundary.md** for the mapping details.

---

## What is not in scope

The following capabilities are explicitly outside the current design. They are not planned for the near term and should not be assumed by callers or contributors.

**Page-level extraction cache.** `PdfDocument` keeps a `Mutex<HashMap<usize, Arc<ExtractedPageText>>>` so repeated `extract_text` / `search_text` calls on the same page reuse the parsed glyph list; `apply_redactions` clears the map because the underlying content streams are about to change. Lower-level helpers like `analyze_page_text` are still cache-free and re-walk the stream on every call — callers that go around the facade get no caching.

**Minimal multi-threading surface.** The engine is still designed for single-threaded use. The only synchronization primitive is the `Mutex` used by the text-extraction cache above; there is no rayon usage. WASM targets are inherently single-threaded in the browser main thread and in most worker configurations.

**No streaming or incremental output.** The writer produces the entire output file as a single `Vec<u8>` before returning. There is no streaming writer and no append-only incremental update path (we intentionally write full rewrites to avoid leaving prior content in the file).

**No PDF creation.** The engine only operates on existing PDF files. It cannot produce a PDF from scratch. The workflow is always: parse an existing file, modify it, write it back.

**No rendering.** The engine does not rasterize pages. There is no canvas, no image output, and no visual preview capability. The demo application uses a separate renderer (pdf.js) for display; the engine only handles the structural redaction.
