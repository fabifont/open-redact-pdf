# Known Limitations and Future Work

This page documents the current known limitations of the engine, organized by subsystem, along with recommended future improvements and notes on intentional design constraints.

---

## Current Limitations

### Parser

- **No encrypted PDFs:** Any PDF containing an `Encrypt` key in the trailer dictionary is rejected outright. There is no support for password-based or certificate-based decryption.
- **Only FlateDecode stream filter:** The engine decodes streams compressed with FlateDecode (zlib/deflate). The following filters are all rejected with an explicit error: LZWDecode, ASCII85Decode, ASCIIHexDecode, DCTDecode (JPEG), JBIG2Decode, and JPXDecode.
- **No TIFF predictor:** FlateDecode with PNG predictors (10–15) is supported. The TIFF predictor (`/Predictor 2`) is rejected with an explicit error because it requires separate bit-depth-aware reconstruction and has not shown up in real-world PDFs that motivated the rest of the parser.
- **Stream Length indirect references:** The parser only handles a literal integer `Length` value in stream dictionaries. If `Length` is an indirect reference (e.g., `5 0 R`), the parser falls back to scanning for the `endstream` keyword rather than resolving the reference. This fallback works for most well-formed PDFs but may mis-frame malformed or unusual streams.

### Font System

- **Simple fonts (Type1/TrueType) — WinAnsi + ToUnicode:** Simple-font byte decoding first consults a `ToUnicode` CMap if the font dictionary exposes one; otherwise it uses `WinAnsiEncoding` when declared (the most common case in real-world PDFs, including many Italian bank statements). Other named encodings (`MacRomanEncoding`, `MacExpertEncoding`, `StandardEncoding`) fall back to ASCII + `U+FFFD`, and `/Encoding` dictionaries with a `Differences` array are not applied — their overrides are silently ignored unless the font also provides `ToUnicode`.
- **Composite fonts — Identity-H only:** Type0 (composite) fonts are supported only when the encoding is `Identity-H`. Other CID encodings (e.g., `90ms-RKSJ-H`, `UniGB-UCS2-H`, `UniJIS-UTF16-H`, etc.) are rejected with an explicit error.
- **No embedded font program parsing:** The engine does not parse embedded Type1, TrueType, or CFF font programs. Glyph outlines, advance widths from the font binary, and kerning tables are all unavailable. Advance widths come from the PDF font dictionary's `Widths` or `W` array only.
- **Ascent/descent heuristic:** Because font programs are not parsed, vertical metrics (ascent and descent) are approximated as 80% and 12% of the font size respectively. These heuristics work for typical Latin text but may produce inaccurate bounding boxes for scripts with unusual vertical extents.

### Text Extraction

- **No caching:** `analyze_page_text` re-executes the full content stream interpretation on every call. For pages with large or complex content streams, repeated calls are expensive.
- **No annotation text:** Text in annotation appearances (widget annotations, free text annotations, etc.) is not extracted.

### Search

- **Substring matching only:** The search engine performs literal substring matching against extracted text. There is no regex support, no stemming, and no word-boundary awareness.
- **No right-to-left or vertical text:** Bidirectional text (Arabic, Hebrew) and vertical text (CJK vertical writing modes) are not supported. The visual ordering heuristics assume left-to-right horizontal text.
- **Empirical line detection thresholds:** The visual line grouping algorithm uses fixed threshold multipliers to decide when two glyphs belong to the same line. These thresholds work for typical documents but may fail for documents with unusual line spacing, superscripts, or mixed font sizes on the same line.
- **Gap-based word detection:** Words are split at horizontal gaps exceeding a threshold derived from the space character width. This may merge adjacent words when the gap is too small, or split one word into two when fonts use unusual advance widths.

### Redaction

- **Form XObjects on targeted pages cause a hard error:** If a page that contains a targeted region also uses Form XObjects, the redact pipeline returns an explicit `Unsupported` error. Text extraction and search both recurse into Form XObjects today, but redaction would require copy-on-write rewriting of the Form's own content stream and is not implemented yet.
- **`v` and `y` Bezier curves not tracked in path bounds:** When computing bounding boxes for vector paths (to neutralize graphic content under a redaction target), the `v` and `y` curve operators (which omit one control point) are not included in the accumulated bounds. A curved path using only `v`/`y` segments may not be fully covered.
- **`'` and `"` text operators use strip mode:** The `'` (move-to-next-line-and-show) and `"` (set-spacing-move-show) operators fall back to stripping the text glyph run without kern compensation. Redacted text adjacent to `'`/`"` runs may have slightly incorrect spacing in the output.
- **`overlay_text` not implemented:** The redaction target model includes an `overlay_text` field for placing replacement label text over a redacted region. This field is accepted by the API but has no effect.
- **No per-glyph redaction control:** The minimum redaction unit is a quad group (a set of bounding quads for a matched text run). Individual glyphs within a matched run cannot be selectively preserved.
- **AABB quad intersection:** Target-to-glyph intersection testing uses axis-aligned bounding box (AABB) intersection. For rotated redaction targets, this may over-select or under-select glyphs near the rotation boundary.

### Output

- **Uncompressed content streams:** Rewritten content streams are emitted without compression. Output files may be significantly larger than the input for documents with large content streams.
- **No object renumbering or garbage collection:** After redaction, unreferenced indirect objects (e.g., font descriptors for fonts no longer used) remain in the output. No dead object elimination is performed.
- **No linearization:** Output PDFs are not linearized ("fast web view"). Sequential reading and progressive loading are not supported.

---

## Recommended Future Improvements

The following improvements are listed in priority order, weighted by coverage impact and security relevance:

1. **Xref stream support** — required to process the majority of PDFs produced by modern tools (Acrobat, Preview, LibreOffice, Chrome print-to-PDF).
2. **ToUnicode/Encoding support for simple fonts** — required for correct text extraction from non-ASCII Type1 and TrueType fonts (accented Latin, Greek, Cyrillic, etc.).
3. **Non-Identity-H composite font encodings** — required for CJK documents using standard CMaps.
4. **Form XObject support** — at minimum, read-through text extraction so that text inside reusable XObjects is searchable and redactable.
5. **Page-level extraction caching** — avoid re-parsing content streams on every `analyze_page_text` call; cache results keyed by page reference and content stream digest.
6. **Additional stream filters** — ASCII85Decode and LZWDecode are the most commonly encountered unsupported filters after FlateDecode.
7. **Output stream re-compression** — compress rewritten content streams with FlateDecode to reduce output size.
8. **Encrypted PDF support** — at least password-based decryption (RC4 and AES standard security handler) so that password-protected documents can be opened, redacted, and re-saved without encryption.

---

## Design Notes on Intentional Limitations

**Explicit failure is a feature.** The engine is designed to fail loudly when it encounters something it cannot handle correctly. Silent degradation — producing output that appears redacted but leaks content — is the primary security risk the design guards against. Every `Unsupported` error is preferable to a false sense of security.

**No streaming or incremental output.** The engine operates on a fully-loaded in-memory document representation. It does not support streaming input or incremental output. In a WASM environment, the entire PDF is already resident in the browser's memory before any processing begins, so this constraint has no practical cost. A streaming architecture would complicate the correctness model without delivering a meaningful benefit in the target deployment context.

**No multi-threading.** The WebAssembly target is single-threaded. While the native Rust codebase does not contain any fundamental obstacle to parallelism (page processing is independent), multi-threading is not a current priority and the architecture is not designed to exploit it.
