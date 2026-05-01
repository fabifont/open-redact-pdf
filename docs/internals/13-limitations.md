# Known Limitations and Future Work

This page documents the current known limitations of the engine, organized by subsystem, along with recommended future improvements and notes on intentional design constraints.

---

## Current Limitations

### Parser

- **Encrypted PDFs — RC4 (V = 1/2), AES-128 (V = 4), AES-256 (V = 5), and PubSec (V = 4/5):** PDFs secured by the Standard Security Handler at V = 1 or V = 2 and R = 2 or R = 3 (RC4 up to 128-bit); V = 4 with R = 4 and the `/StdCF` crypt filter in `/V2` (RC4-128) or `/AESV2` (AES-128-CBC) mode; or V = 5 with R = 5 or R = 6 and `/CFM /AESV3` (AES-256-CBC) are decrypted in place during parsing. Either the user or owner password authenticates; the empty user password is the default. `/EncryptMetadata false` is honoured on V=4 (Algorithm 2 step-5 `0xFFFFFFFF` suffix, and `/Type /Metadata` streams skip decryption). V=5 R=6 runs the full ISO 32000-2 iterative Algorithm 2.B hash. Public-key (`/Filter /Adobe.PubSec`) PDFs are decrypted via a recipient X.509 certificate plus its RSA private key, both DER-encoded; SubFilters `adbe.pkcs7.s4` (V=4 / AES-128) and `adbe.pkcs7.s5` (V=5 / AES-256) are supported. Recipients are matched by `IssuerAndSerialNumber` or `SubjectKeyIdentifier`; the inner content key is unwrapped via RSA-PKCS1v15 or RSA-OAEP and the seed+permissions are extracted from a CMS `EnvelopedData` inner AES-CBC layer. The remaining gaps are SubFilter `s3` (V=1 RC4-40, deprecated) and key-agreement (`KeyAgreeRecipientInfo`, ECDH) recipients — both rejected with `Unsupported`.
- **Stream filters:** The engine decodes `FlateDecode`, `ASCII85Decode`, `ASCIIHexDecode`, `LZWDecode`, and `RunLengthDecode`, including chained filter pipelines (e.g. `[/ASCII85Decode /FlateDecode]`). `LZWDecode` honours both `DecodeParms /EarlyChange` values (0 and 1, default 1), and the PNG/TIFF predictor pipeline runs on top of the decompressed bytes exactly as for `FlateDecode`. `DCTDecode` (JPEG), `JBIG2Decode`, `JPXDecode`, and `CCITTFaxDecode` are still rejected with an explicit error.
- _(resolved)_ TIFF predictor (`/Predictor 2`) is now supported for 8-bit components; other bit depths still error explicitly.
- **Stream Length indirect references:** The parser resolves indirect `/Length` references against the xref table at parse time, so `<< /Length 5 0 R >>` streams whose resolved length points at a plain non-negative integer are framed exactly per the PDF spec. Lengths that point at compressed (ObjStm-backed) integers, or lengths whose resolved object is not a simple integer, still fall back to scanning forward for the `endstream` keyword.

### Font System

- **Simple fonts (Type1/TrueType) — WinAnsi / MacRoman / Standard + ToUnicode + Differences:** Simple-font byte decoding first consults a `ToUnicode` CMap if the font dictionary exposes one, then applies any `/Encoding` `/Differences` overrides through a compact Adobe Glyph List subset, then falls back to the named base encoding. `WinAnsiEncoding` (Windows-1252), `MacRomanEncoding` (Mac Roman), and `StandardEncoding` (Adobe Standard PostScript) are all decoded fully, including the historical quirks `StandardEncoding` bakes in — `0x27` decodes to `quoteright` (U+2019) and `0x60` to `quoteleft` (U+2018). `MacExpertEncoding` still falls back to ASCII + `U+FFFD`, and glyph names that are not in the AGL subset produce `U+FFFD` even when they appear in `/Differences`.
- **Composite fonts — `Identity-H` plus four Unicode-keyed CJK CMaps:** Type0 fonts are supported when the encoding is `Identity-H` or one of Adobe's predefined Unicode-keyed CMaps `UniGB-UCS2-H`, `UniKS-UCS2-H`, `UniJIS-UTF16-H`, or `UniCNS-UTF16-H`. The latter four bypass the CID step and decode the byte stream directly to Unicode (UCS-2 BE or UTF-16 BE, including surrogate-pair SMP scalars). Glyph widths under those four CMaps always fall back to the descendant font's `/DW` because per-CID widths are not computed; to keep this honest, fonts whose descendant `/W` array would override any CID's width away from `/DW` are rejected with `Unsupported` rather than silently mis-positioning glyphs. For `UTF-16-H`, `ToUnicode` overrides are not consulted for surrogate-pair glyphs (the in-tree `ToUnicode` parser only accepts 1- or 2-byte keys); ToUnicode entries with 4-byte source codes are silently skipped during parsing, and the SMP scalar is composed directly from the raw UTF-16 bytes. Other CID encodings (e.g., `90ms-RKSJ-H`, `GBK-EUC-H`) and all `-V` vertical variants are still rejected with an explicit error.
- **No embedded font program parsing:** The engine does not parse embedded Type1, TrueType, or CFF font programs. Glyph outlines, advance widths from the font binary, and kerning tables are all unavailable. Advance widths come from the PDF font dictionary's `Widths` or `W` array only.
- **Ascent/descent heuristic:** Because font programs are not parsed, vertical metrics (ascent and descent) are approximated as 80% and 12% of the font size respectively. These heuristics work for typical Latin text but may produce inaccurate bounding boxes for scripts with unusual vertical extents.

### Text Extraction

- _(resolved)_ Per-page extraction results are now cached on `PdfDocument` via an `Arc`-wrapped map keyed by page index; `extract_text` and `search_text` reuse the cached walk across calls, and `apply_redactions` clears the cache so post-redaction reads reflect the rewritten content stream.
- **No annotation text:** Text in annotation appearances (widget annotations, free text annotations, etc.) is not extracted.
- _(resolved)_ Text inside Form XObjects is now extracted by recursing into their content streams; their `Matrix`, their local `Resources.Font`, and their local `Resources.ExtGState` are honoured, and the recursion is cycle-protected and depth-capped at 16.

### Search

- **Substring matching only:** The search engine performs literal substring matching against extracted text. There is no regex support, no stemming, and no word-boundary awareness.
- **No right-to-left or vertical text:** Bidirectional text (Arabic, Hebrew) and vertical text (CJK vertical writing modes) are not supported. The visual ordering heuristics assume left-to-right horizontal text.
- **Empirical line detection thresholds:** The visual line grouping algorithm uses an anchor-based proportional y-tolerance (`height_ref × 0.10`, where `height_ref` is the maximum glyph bbox height seen on the line and the anchor is the y-centre of the first glyph placed). This works for typical documents and dense layouts down to sub-1pt row spacing, but it can split a superscript or footnote glyph onto its own visual line when the offset exceeds 10% of the surrounding line's dominant glyph height.
- **Gap-based word detection:** Words are split at horizontal gaps exceeding a threshold derived from the space character width. This may merge adjacent words when the gap is too small, or split one word into two when fonts use unusual advance widths.

### Redaction

- **Form XObjects are redacted via copy-on-write with full recursion:** each Form whose `BBox × Matrix × CTM` intersects a target is cloned per page (new `ObjectRef`). The copy's content stream is rewritten to strip the targeted glyphs, neutralize any vector paint operators that fall under a target, and replace `Do` invocations of intersecting Image XObjects with `n`. If the redacted Form invokes another Form whose bounding quad also intersects a target, the rewrite recurses — the inner Form is copied, its own content is rewritten (text, paint, and Image XObjects), and the outer Form's `Resources.XObject` is repointed at the inner copy. Recursion is capped at depth 8 with a warning.
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

1. _(resolved)_ **Object stream re-emission on save** — the writer now mirrors input shape. Classic-input PDFs continue to save as classic xref tables; xref-stream-shaped inputs save as a `Type /XRef` stream with eligible objects (gen=0, non-stream values) packed into freshly-built `Type /ObjStm` containers (FlateDecode, 100 members per chunk). The parser drops the Encrypt dictionary after decryption and the original ObjStm containers after materialisation so the writer never emits them as dangling unreferenced bytes.
2. **Broaden encrypted PDF support** — RC4 (V = 1/2), AES-128 (V = 4 R = 4), and AES-256 (V = 5 R = 5 / R = 6) under the user or owner password are in. The remaining gap is the public-key security handler (`/Filter /Adobe.PubSec`), which wraps the file key in a PKCS#7 recipient envelope rather than deriving it from a password. Adding it would allow opening PDFs sent to a specific certificate.
3. _(resolved)_ **Smarter line grouping** — the visual line-detection heuristic now uses an anchor-based proportional tolerance (`height_ref × 0.10`, where `height_ref` is the maximum glyph bbox height seen on the line and the anchor is the y-centre of the first glyph placed). The previous `min(line_height × 0.3, 1.0)` formula's `1pt` absolute cap merged sub-1pt-apart rows for small fonts; the new formula splits 6pt rows 0.5pt apart (`0.5 > 0.48`) while still merging mixed-font same-baseline glyphs (10pt + 8pt: y-centre Δ ≈ 0.56pt < 0.80pt tolerance). The remaining gap is a rare regression for mixed font sizes that differ by more than one ratio width on the same line; a future adaptive-pass splitter could address it if real-world fixtures surface the case.
4. **Vertical and registry-keyed CMaps** — vertical-mode CMaps (`-V` variants) and registry-keyed CMaps such as `90ms-RKSJ-H` still error out. Vertical writing requires advance-direction handling beyond the current horizontal-only positioning model; registry-keyed CMaps would require embedding Adobe's CMap tables to translate the byte stream into CIDs.

---

## Design Notes on Intentional Limitations

**Explicit failure is a feature.** The engine is designed to fail loudly when it encounters something it cannot handle correctly. Silent degradation — producing output that appears redacted but leaks content — is the primary security risk the design guards against. Every `Unsupported` error is preferable to a false sense of security.

**No streaming or incremental output.** The engine operates on a fully-loaded in-memory document representation. It does not support streaming input or incremental output. In a WASM environment, the entire PDF is already resident in the browser's memory before any processing begins, so this constraint has no practical cost. A streaming architecture would complicate the correctness model without delivering a meaningful benefit in the target deployment context.

**No multi-threading.** The WebAssembly target is single-threaded. While the native Rust codebase does not contain any fundamental obstacle to parallelism (page processing is independent), multi-threading is not a current priority and the architecture is not designed to exploit it.
