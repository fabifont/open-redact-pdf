---
title: Roadmap
---

# Roadmap

## Implemented MVP

- Classic xref parsing with incremental update chain support (follows `Prev` pointers)
- Standard Security Handler decryption (V = 1/2 RC4; V = 4 R = 4 with the `/StdCF` crypt filter in `/CFM /V2` or `/CFM /AESV2` mode; V = 5 R = 5 or R = 6 with `/CFM /AESV3`, i.e. AES-256-CBC) under either the user password (including empty) or the owner password — the trailer's `/Encrypt` is consumed at parse time and downstream stages see a plaintext document. R = 6 runs the ISO 32000-2 iterative Algorithm 2.B hash; `/EncryptMetadata false` is honoured on V=4 (Algorithm 2 step-5 `0xFFFFFFFF` suffix and `/Type /Metadata` streams left in plaintext).
- PDF 1.5+ cross-reference streams, object streams, and the hybrid `XRefStm` form
- `FlateDecode` and `LZWDecode` with the TIFF predictor (`/Predictor 2`) and PNG predictors (10–15) via `DecodeParms`, plus `ASCII85Decode`, `ASCIIHexDecode`, and `RunLengthDecode` for text-oriented filter chains (`LZWDecode` honours `DecodeParms /EarlyChange`)
- Page tree traversal with inherited resources, media boxes, crop boxes, and rotation
- Content parsing for common text, path, image, clipping, color, graphics-state, and marked-content operators (including inline images and dictionary operands)
- Simple-font text extraction and search geometry (including fonts set via ExtGState `gs` operator), with `ToUnicode` CMap decoding, `WinAnsiEncoding` + `MacRomanEncoding` + `StandardEncoding` for non-ASCII bytes, and `/Encoding /Differences` arrays resolved through an Adobe Glyph List subset
- `Type0` / `Identity-H` composite font extraction, search, and redaction when `ToUnicode` is available
- Form XObject text extraction and search (recursive, with cycle protection and a depth cap)
- Form XObject redaction via per-page copy-on-write: text glyphs, vector paint, and Image XObject `Do` invocations inside the Form are all neutralized; nested Forms recurse up to depth 8
- Redaction refuses documents whose default Optional Content configuration hides any layer (no silent leaks from off-by-default OCGs). Callers can opt in to sanitization via `sanitizeHiddenOcgs: true`, which strips `BDC /OC /<name> ... EMC` content gated by hidden OCGs and clears the catalog's hidden state on save.
- Geometry target normalization for rects, quads, and quad groups
- Three redaction modes: `strip` (remove bytes), `redact` (blank space + overlay), `erase` (blank space, no overlay)
- `overlayText` labels stamped in `redact` mode, auto-sized to the target and coloured for contrast against the fill
- Tighter glyph bounding boxes (80% em-square height) to reduce adjacent-line false positives
- Vector path bounds include the `v` and `y` curve shorthands so paths built only from those are still covered
- Deterministic full-save rewrite with FlateDecode-compressed content streams
- WASM bindings and a browser demo
- Demo UI with zoom controls, collapsible pages, search-driven redaction, Form-rewrite count in the report, and in-app error reporting
- `cargo-release` workspace configuration (`release.toml`) bumps every crate's version, rewrites the inter-crate `path + version` pins and both `package.json` files, tags and pushes — all in a single command; `scripts/check-release-version.mjs` retains its defence-in-depth verification of the same invariants in CI

## Next priorities

- Broader composite font encodings beyond `Identity-H` + `ToUnicode` — required for CJK documents that use Adobe's predefined CMaps (`UniJIS-UTF16-H`, `UniGB-UCS2-H`, `UniCNS-UTF16-H`, `UniKS-UCS2-H`)
- Partial image rewriting so redaction targets that overlap only part of an Image XObject mask the affected pixels instead of neutralizing the whole `Do`
- Incremental-save preservation (reading is supported; output is always a flat rewrite that flattens xref streams and object streams into a classic xref table)
- Public-key security handler — Standard Security Handler V = 1/2/4/5 (RC4 + AES-128 + AES-256) under the user or owner password is in; the remaining gap is the public-key `/Filter /Adobe.PubSec` form, which wraps the file key in a PKCS#7 recipient envelope rather than deriving it from a password.
- Smarter visual line grouping for dense layouts where several short text runs sit only a unit or two apart in `y` and the current heuristic merges them into one line

## Documentation policy

When one of these priorities lands, the following docs should be updated in the same change:

- `README.md`
- `docs/reference/supported-subset.md`
- the relevant API reference page under `docs/reference/`
- any affected workflow guide under `docs/guides/`
