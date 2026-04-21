---
title: Roadmap
---

# Roadmap

## Implemented MVP

- Classic xref parsing with incremental update chain support (follows `Prev` pointers)
- PDF 1.5+ cross-reference streams, object streams, and the hybrid `XRefStm` form
- `FlateDecode` with the TIFF predictor (`/Predictor 2`) and PNG predictors (10–15) via `DecodeParms`
- Page tree traversal
- Content parsing for common text, path, image, clipping, color, graphics-state, and marked-content operators (including inline images and dictionary operands)
- Simple-font text extraction and search geometry (including fonts set via ExtGState `gs` operator), with `ToUnicode` CMap decoding and `WinAnsiEncoding` for non-ASCII bytes
- `Type0` / `Identity-H` composite font extraction, search, and redaction when `ToUnicode` is available
- Geometry target normalization for rects, quads, and quad groups
- Three redaction modes: `strip` (remove bytes), `redact` (blank space + overlay), `erase` (blank space, no overlay)
- Tighter glyph bounding boxes (80% em-square height) to reduce adjacent-line false positives
- True redaction for a constrained subset of PDFs
- Deterministic full-save rewrite
- WASM bindings and a browser demo
- Demo UI with zoom controls, collapsible pages, search-driven redaction, and in-app error reporting

## Next priorities

- Broader CID and composite font support beyond `Identity-H` + `ToUnicode`
- Form XObject redaction (text extraction already traverses Form XObjects; redaction on pages that invoke a Form still errors explicitly)
- Better vector-path bounds
- Partial image rewriting
- Optional-content and hidden-layer sanitization
- Overlay text stamping
- Incremental-save preservation (reading is supported; output is always a flat rewrite)
- Parse encrypted pdfs

## Documentation policy

When one of these priorities lands, the following docs should be updated in the same change:

- `README.md`
- `docs/reference/supported-subset.md`
- the relevant API reference page under `docs/reference/`
- any affected workflow guide under `docs/guides/`
