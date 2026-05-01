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
- `Type0` composite font extraction, search, and redaction with `Identity-H` (CID + `ToUnicode`) and Adobe's predefined Unicode-keyed CJK CMaps `UniGB-UCS2-H`, `UniKS-UCS2-H`, `UniJIS-UTF16-H`, and `UniCNS-UTF16-H` — bytes are decoded directly to Unicode (UCS-2 BE or UTF-16 BE, including surrogate-pair SMP scalars); glyph widths fall back to the descendant font's `/DW` for the predefined CMaps
- Anchor-based visual line grouping — each line's y-tolerance is `height_ref × 0.10` against a fixed first-glyph anchor (no running-mean drift, no 1pt absolute cap), so dense layouts down to sub-1pt row spacing split correctly while mixed-font same-baseline rows still merge
- Cross-reference shape preserved on save — classic-input PDFs round-trip as classic xref + trailer; xref-stream-shaped inputs round-trip as `Type /XRef` streams with eligible objects packed into freshly-built `Type /ObjStm` containers. The parser also drops the Encrypt dictionary after decryption and the original ObjStm containers after materialisation so neither leaks into saved bytes.
- Public-key security handler — `/Filter /Adobe.PubSec` PDFs decrypt via a recipient X.509 certificate plus its RSA private key (DER-encoded, supplied as separate buffers). SubFilters `adbe.pkcs7.s4` (V=4, AES-128) and `adbe.pkcs7.s5` (V=5, AES-256) are supported; key-transport (RSA-PKCS1v15 and RSA-OAEP) recipient infos are matched by `IssuerAndSerialNumber` or `SubjectKeyIdentifier`. Once authenticated the file is decrypted in place and saved without `/Encrypt` (matches the password-handler behaviour).
- Partial Image XObject rewriting — when a redaction target overlaps only part of an Image XObject, the underlying raster is rewritten in place (copy-on-write so multi-page-shared images are unaffected) so the targeted pixel region is replaced with the plan's `fill_color` while the rest of the image survives. Supported formats: raw and `FlateDecode` for `DeviceGray` / `DeviceRGB` / `DeviceCMYK` at 8 bits per component (with optional TIFF/PNG predictors), plus `DCTDecode` (JPEG) for the same colour spaces. Other formats (`Indexed`, `ICCBased`, `JBIG2Decode`, `JPXDecode`, `CCITTFaxDecode`, non-8-bpc) and any decode error fall back to the existing whole-invocation `Do → n` neutralization.
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

The original MVP roadmap is complete. Future improvements that would broaden coverage beyond the MVP scope:

- Vertical writing-mode CMaps (`-V` variants) and registry-keyed predefined CMaps (e.g. `90ms-RKSJ-H`) for Type0 fonts that don't decode directly to Unicode.
- ECDH key-agreement recipients (`KeyAgreeRecipientInfo`) under `/Filter /Adobe.PubSec`, and `/SubFilter /adbe.pkcs7.s3` (V=1 RC4-40, deprecated).
- Partial image rewriting for `Indexed`, `ICCBased`, `JBIG2Decode`, `JPXDecode`, and `CCITTFaxDecode` formats and for `BitsPerComponent` other than 8 (these currently fall back to whole-invocation drop).
- Object renumbering / dead-object garbage collection on save (the writer leaves unreferenced indirect objects in place today).
- Writing encrypted PDFs (the save path always emits a plaintext rewrite).
- Linearized output ("fast web view").

## Documentation policy

When one of these priorities lands, the following docs should be updated in the same change:

- `README.md`
- `docs/reference/supported-subset.md`
- the relevant API reference page under `docs/reference/`
- any affected workflow guide under `docs/guides/`
