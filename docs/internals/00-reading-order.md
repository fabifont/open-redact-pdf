# Reading Order for New Contributors

This page tells you which internal docs to read, and in what order, depending on what you are trying to do. The internals documents form a dependency graph: some assume knowledge from others. Start at the right entry point and you will not need to backtrack.

---

## Entry points by role

### If you are completely new to this codebase

Begin with the PDF Primer before touching any code.

1. **02-pdf-primer.md** — What a PDF file actually is at the byte level. Cross-reference tables, object streams, content streams. You cannot reason about the parser or the redaction logic without this.
2. **14-glossary.md** — Keep this open as a reference tab. Every term used across the internals docs is defined here.

### If you want to understand the architecture

Read these two documents in order before looking at any crate.

1. **01-architecture-overview.md** — The 30,000-foot view. Crate dependency graph, pipeline stages, design principles, error model, and what is explicitly out of scope.
2. **15-spec-to-code.md** — Maps PDF specification concepts (operators, dictionaries, object types) to the Rust types and functions that implement them. Bridges the gap between the primer and the code.

### If you are modifying the parser

The parser lives in `crates/pdf_objects`. Read these before changing anything there.

1. **03-parsing-model.md** — How bytes become tokens, how tokens become objects, how the xref table and incremental update chains are resolved.
2. **04-object-model.md** — The Rust type hierarchy for PDF objects, how indirect references are resolved, and how the document tree is structured in memory.

### If you are working on text extraction or search

Text extraction depends on a correct understanding of coordinate systems. Do not skip the graphics state document.

1. **05-graphics-state.md** — The current transformation matrix, text matrix, font size scaling, and how page-space coordinates are produced from operator arguments.
2. **06-text-system.md** — Font loading, glyph decoding, character widths, and how individual glyphs become positioned quads in page space.
3. **07-search-geometry.md** — How extracted glyphs are sorted into visual reading order, how text is normalized for matching, and how substring matches are mapped back to glyph quads for the redaction pipeline.

### If you are working on redaction

Redaction depends on correct targets and a clear understanding of the apply pipeline.

1. **08-redaction-targets.md** — The `NormalizedPageTarget` model. How Rect, Quad, and QuadGroup inputs are validated and normalized into page-space quads.
2. **09-redaction-pipeline.md** — How the apply step intersects targets with glyphs, vectors, and images; how content streams are rewritten; how annotations and metadata are stripped.

### If you are working on the WASM or JavaScript layer

1. **11-wasm-boundary.md** — The wasm-bindgen surface, serialization conventions, error propagation across the boundary, and how the TypeScript SDK wraps the raw WASM exports.

### If you need to understand what is and is not supported

1. **13-limitations.md** — The explicit list of PDF features that are not supported, and why. Each limitation notes whether it is a deliberate scope decision or a known gap.
2. **12-security-model.md** — The threat model for redaction correctness. What "redacted" means in this engine, what attacks are in scope, and what the engine does not protect against.

### Essential reading for everyone

**16-top-ten-decisions.md** — Regardless of which area you are working in, read this document. It describes the ten most important implementation decisions made during the design of this engine. Every maintainer is expected to understand all ten before merging significant changes.

---

## Visual map of documentation dependencies

The arrows point from "required reading" to "depends on it". A document with multiple incoming arrows requires all of its prerequisites before it will make sense.

```
02-pdf-primer ──────────────────────────────────────────────┐
      │                                                      │
      ▼                                                      ▼
14-glossary                                        01-architecture-overview
                                                             │
                                                             ▼
                                                    15-spec-to-code
                                                             │
                                          ┌──────────────────┤
                                          │                  │
                                          ▼                  ▼
                                   03-parsing-model    05-graphics-state
                                          │                  │
                                          ▼                  ▼
                                   04-object-model     06-text-system
                                                             │
                                                             ▼
                                                      07-search-geometry
                                                             │
                                          ┌──────────────────┘
                                          │
                                          ▼
                                  08-redaction-targets
                                          │
                                          ▼
                                  09-redaction-pipeline
                                          │
                                          ▼
                                   10-writer
                                          │
                              ┌───────────┤
                              │           │
                              ▼           ▼
                       11-wasm-boundary  12-security-model
                                          │
                                          ▼
                                   13-limitations

16-top-ten-decisions  ←  read at any point; no prerequisites
```

---

## Document index

| File | Title | Primary audience |
|------|-------|-----------------|
| 00-reading-order.md | Reading Order for New Contributors | Everyone |
| 01-architecture-overview.md | Architecture Overview | Everyone |
| 02-pdf-primer.md | PDF Primer | New contributors |
| 03-parsing-model.md | Parsing Model | Parser contributors |
| 04-object-model.md | Object Model | Parser contributors |
| 05-graphics-state.md | Graphics State | Text / redaction contributors |
| 06-text-system.md | Text System | Text contributors |
| 07-search-geometry.md | Search Geometry | Text contributors |
| 08-redaction-targets.md | Redaction Targets | Redaction contributors |
| 09-redaction-pipeline.md | Redaction Pipeline | Redaction contributors |
| 10-writer.md | Writer | Writer / serialization contributors |
| 11-wasm-boundary.md | WASM Boundary | WASM / JS contributors |
| 12-security-model.md | Security Model | Everyone |
| 13-limitations.md | Limitations | Everyone |
| 14-glossary.md | Glossary | Everyone (reference) |
| 15-spec-to-code.md | Spec to Code Map | Everyone |
| 16-top-ten-decisions.md | Top Ten Decisions | Everyone |
