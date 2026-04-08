# Roadmap

## Implemented MVP

- Classic xref parsing
- Page tree traversal
- Content parsing for common text, path, and image operators
- Simple-font text extraction and search geometry
- `Type0` / `Identity-H` composite font extraction, search, and redaction when `ToUnicode` is available
- Geometry target normalization for rects, quads, and quad groups
- True redaction for a constrained subset of PDFs
- Deterministic full-save rewrite
- WASM bindings and a browser demo

## Next priorities

- Broader CID and composite font support beyond `Identity-H` + `ToUnicode`
- Form XObject traversal and redaction
- Better vector-path bounds
- Partial image rewriting
- Optional-content and hidden-layer sanitization
- Overlay text stamping
- Incremental-save preservation
