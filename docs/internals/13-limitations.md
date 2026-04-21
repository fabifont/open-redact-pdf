# Known Limitations and Future Work

This page documents the current known limitations of the engine, organized by subsystem, along with recommended future improvements and notes on intentional design constraints.

---

## Current Limitations

### Parser

- **No encrypted PDFs:** Any PDF containing an `Encrypt` key in the trailer dictionary is rejected outright. There is no support for password-based or certificate-based decryption.
- **Only FlateDecode stream filter:** The engine decodes streams compressed with FlateDecode (zlib/deflate). The following filters are all rejected with an explicit error: LZWDecode, ASCII85Decode, ASCIIHexDecode, DCTDecode (JPEG), JBIG2Decode, and JPXDecode.
- _(resolved)_ TIFF predictor (`/Predictor 2`) is now supported for 8-bit components; other bit depths still error explicitly.
- **Stream Length indirect references:** The parser only handles a literal integer `Length` value in stream dictionaries. If `Length` is an indirect reference (e.g., `5 0 R`), the parser falls back to scanning for the `endstream` keyword rather than resolving the reference. This fallback works for most well-formed PDFs but may mis-frame malformed or unusual streams.

### Font System

- **Simple fonts (Type1/TrueType) — WinAnsi + ToUnicode + Differences:** Simple-font byte decoding first consults a `ToUnicode` CMap if the font dictionary exposes one, then applies any `/Encoding` `/Differences` overrides through a compact Adobe Glyph List subset, then falls back to the named base encoding — `WinAnsiEncoding` handles the full Windows-1252 repertoire. Other named encodings (`MacRomanEncoding`, `MacExpertEncoding`, `StandardEncoding`) still fall back to ASCII + `U+FFFD`, and glyph names that are not in the AGL subset produce `U+FFFD` even when they appear in `/Differences`.
- **Composite fonts — Identity-H only:** Type0 (composite) fonts are supported only when the encoding is `Identity-H`. Other CID encodings (e.g., `90ms-RKSJ-H`, `UniGB-UCS2-H`, `UniJIS-UTF16-H`, etc.) are rejected with an explicit error.
- **No embedded font program parsing:** The engine does not parse embedded Type1, TrueType, or CFF font programs. Glyph outlines, advance widths from the font binary, and kerning tables are all unavailable. Advance widths come from the PDF font dictionary's `Widths` or `W` array only.
- **Ascent/descent heuristic:** Because font programs are not parsed, vertical metrics (ascent and descent) are approximated as 80% and 12% of the font size respectively. These heuristics work for typical Latin text but may produce inaccurate bounding boxes for scripts with unusual vertical extents.

### Text Extraction

- **No caching:** `analyze_page_text` re-executes the full content stream interpretation on every call. For pages with large or complex content streams, repeated calls are expensive.
- **No annotation text:** Text in annotation appearances (widget annotations, free text annotations, etc.) is not extracted.
- _(resolved)_ Text inside Form XObjects is now extracted by recursing into their content streams; their `Matrix`, their local `Resources.Font`, and their local `Resources.ExtGState` are honoured, and the recursion is cycle-protected and depth-capped at 16.

### Search

- **Substring matching only:** The search engine performs literal substring matching against extracted text. There is no regex support, no stemming, and no word-boundary awareness.
- **No right-to-left or vertical text:** Bidirectional text (Arabic, Hebrew) and vertical text (CJK vertical writing modes) are not supported. The visual ordering heuristics assume left-to-right horizontal text.
- **Empirical line detection thresholds:** The visual line grouping algorithm uses fixed threshold multipliers to decide when two glyphs belong to the same line. These thresholds work for typical documents but may fail for documents with unusual line spacing, superscripts, or mixed font sizes on the same line.
- **Gap-based word detection:** Words are split at horizontal gaps exceeding a threshold derived from the space character width. This may merge adjacent words when the gap is too small, or split one word into two when fonts use unusual advance widths.

### Redaction

- **Form XObjects are redacted via copy-on-write with nested-Form recursion:** each Form whose `BBox × Matrix × CTM` intersects a target is cloned per page (new `ObjectRef`), and the copy's content stream is rewritten to strip the targeted glyphs; the page's `Resources.XObject` entry is then rewritten to point at the copy, so other pages that share the original Form are unaffected. If the redacted Form invokes another Form whose bounding quad also intersects a target, the rewrite recurses — the inner Form is copied too, its content is rewritten, and the outer Form's `Resources.XObject` is repointed at the inner copy. Recursion is capped at depth 8 with a warning; vector paint operators and Image XObjects inside a redacted Form are still passed through unchanged.
- _(resolved)_ `v` and `y` Bezier curve operators are now converted into full three-point curves in the path-bounds tracker by inferring the missing control point (current path point for `v`, endpoint for `y`), so curved paths that use only those shorthands are fully covered.
- **`'` and `"` text operators use strip mode:** The `'` (move-to-next-line-and-show) and `"` (set-spacing-move-show) operators fall back to stripping the text glyph run without kern compensation. Redacted text adjacent to `'`/`"` runs may have slightly incorrect spacing in the output.
- _(resolved)_ `overlay_text` is now stamped inside each redacted region in `redact` mode using Helvetica sized to fit the target, with contrast-aware black-or-white text color against the fill. The label is written into the page's own content stream, so it is searchable and extractable like any other page text; glyph names outside the Adobe Glyph List subset still fall back to `U+FFFD` on extraction.
- **No per-glyph redaction control:** The minimum redaction unit is a quad group (a set of bounding quads for a matched text run). Individual glyphs within a matched run cannot be selectively preserved.
- **AABB quad intersection:** Target-to-glyph intersection testing uses axis-aligned bounding box (AABB) intersection. For rotated redaction targets, this may over-select or under-select glyphs near the rotation boundary.

### Output

- _(resolved)_ Rewritten content streams are now FlateDecode-compressed on save; output no longer ships plaintext content bytes from the rewrite.
- **No object renumbering or garbage collection:** After redaction, unreferenced indirect objects (e.g., font descriptors for fonts no longer used) remain in the output. No dead object elimination is performed.
- **No linearization:** Output PDFs are not linearized ("fast web view"). Sequential reading and progressive loading are not supported.

---

## Recommended Future Improvements

The following improvements are listed in priority order, weighted by coverage impact and security relevance:

1. **Paint- and image-aware Form redaction** — text inside redacted Forms (including nested Forms) is handled via copy-on-write recursion. What remains: running the vector-path and image-neutralization passes on the Form's own content so paint operators and inner Image XObjects that sit under a target are neutralized alongside the text.
2. **Non-Identity-H composite font encodings** — required for CJK documents using standard CMaps (`UniJIS-UTF16-H`, `UniGB-UCS2-H`, `UniCNS-UTF16-H`, `UniKS-UCS2-H`, and friends).
3. **Page-level extraction caching** — avoid re-parsing content streams on every `analyze_page_text` call; cache results keyed by page reference and content stream digest.
4. **Additional stream filters** — ASCII85Decode and LZWDecode are the most commonly encountered unsupported filters after FlateDecode.
5. **Object stream re-emission on save** — the writer currently flattens the input xref stream and any object streams into a classic xref table with inline indirect objects, which can double the saved size of modern PDFs. Re-emitting as an xref stream + object streams would match the input shape.
6. **Encrypted PDF support** — at least password-based decryption (RC4 and AES standard security handler) so that password-protected documents can be opened, redacted, and re-saved without encryption.
7. **Smarter line grouping** — the current visual line-detection heuristic merges text runs whose centres are within `line_height × 0.55` of one another, which fails on dense layouts (e.g., bank statements) where several short lines sit only a unit or two apart.

---

## Design Notes on Intentional Limitations

**Explicit failure is a feature.** The engine is designed to fail loudly when it encounters something it cannot handle correctly. Silent degradation — producing output that appears redacted but leaks content — is the primary security risk the design guards against. Every `Unsupported` error is preferable to a false sense of security.

**No streaming or incremental output.** The engine operates on a fully-loaded in-memory document representation. It does not support streaming input or incremental output. In a WASM environment, the entire PDF is already resident in the browser's memory before any processing begins, so this constraint has no practical cost. A streaming architecture would complicate the correctness model without delivering a meaningful benefit in the target deployment context.

**No multi-threading.** The WebAssembly target is single-threaded. While the native Rust codebase does not contain any fundamental obstacle to parallelism (page processing is independent), multi-threading is not a current priority and the architecture is not designed to exploit it.
