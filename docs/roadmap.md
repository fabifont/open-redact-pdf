---
title: Roadmap
---

# Roadmap

## Implemented MVP

- Classic xref parsing with incremental update chain support (follows `Prev` pointers)
- Standard Security Handler decryption (V = 1/2, R = 2/3, RC4 up to 128-bit) under either the user password (including empty) or the owner password — the trailer's `/Encrypt` is consumed at parse time and downstream stages see a plaintext document
- PDF 1.5+ cross-reference streams, object streams, and the hybrid `XRefStm` form
- `FlateDecode` with the TIFF predictor (`/Predictor 2`) and PNG predictors (10–15) via `DecodeParms`
- Page tree traversal with inherited resources, media boxes, crop boxes, and rotation
- Content parsing for common text, path, image, clipping, color, graphics-state, and marked-content operators (including inline images and dictionary operands)
- Simple-font text extraction and search geometry (including fonts set via ExtGState `gs` operator), with `ToUnicode` CMap decoding, `WinAnsiEncoding` for non-ASCII bytes, and `/Encoding /Differences` arrays resolved through an Adobe Glyph List subset
- `Type0` / `Identity-H` composite font extraction, search, and redaction when `ToUnicode` is available
- Form XObject text extraction and search (recursive, with cycle protection and a depth cap)
- Form XObject redaction via per-page copy-on-write: text glyphs, vector paint, and Image XObject `Do` invocations inside the Form are all neutralized; nested Forms recurse up to depth 8
- Redaction refuses documents whose default Optional Content configuration hides any layer (no silent leaks from off-by-default OCGs)
- Geometry target normalization for rects, quads, and quad groups
- Three redaction modes: `strip` (remove bytes), `redact` (blank space + overlay), `erase` (blank space, no overlay)
- `overlayText` labels stamped in `redact` mode, auto-sized to the target and coloured for contrast against the fill
- Tighter glyph bounding boxes (80% em-square height) to reduce adjacent-line false positives
- Vector path bounds include the `v` and `y` curve shorthands so paths built only from those are still covered
- Deterministic full-save rewrite with FlateDecode-compressed content streams
- WASM bindings and a browser demo
- Demo UI with zoom controls, collapsible pages, search-driven redaction, Form-rewrite count in the report, and in-app error reporting

## Next priorities

- Broader composite font encodings beyond `Identity-H` + `ToUnicode` — required for CJK documents that use Adobe's predefined CMaps (`UniJIS-UTF16-H`, `UniGB-UCS2-H`, `UniCNS-UTF16-H`, `UniKS-UCS2-H`)
- Partial image rewriting so redaction targets that overlap only part of an Image XObject mask the affected pixels instead of neutralizing the whole `Do`
- Optional-content sanitization: today documents with hidden-by-default OCGs are refused up front; a future change would let callers opt in to stripping hidden layers so they can still redact the visible content
- Incremental-save preservation (reading is supported; output is always a flat rewrite that flattens xref streams and object streams into a classic xref table)
- Broaden encrypted-PDF support — Standard Security Handler V = 4 (AES-128), V = 5 / R = 6 (AES-256), and the public-key handler. RC4 under the user or owner password is in; these extensions would complete the story.
- Smarter visual line grouping for dense layouts where several short text runs sit only a unit or two apart in `y` and the current heuristic merges them into one line
- Adopt `cargo-release` for the Rust publish step so the workspace version bump, inter-crate pin rewrite, tag creation, and ordered publish happen through a single tool; extend `scripts/check-release-version.mjs` to verify every `crates/*/Cargo.toml` inter-crate pin matches the workspace version, so "bumped workspace but forgot the pins" stops being a silent foot-gun

## Documentation policy

When one of these priorities lands, the following docs should be updated in the same change:

- `README.md`
- `docs/reference/supported-subset.md`
- the relevant API reference page under `docs/reference/`
- any affected workflow guide under `docs/guides/`
