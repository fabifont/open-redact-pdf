---
title: Supported PDF Subset
---

# Supported PDF Subset

This project intentionally targets a narrow, explicit MVP.

## Supported now

- Unencrypted PDFs, and PDFs secured by the Standard Security Handler with `V` = 1 or 2 and `R` = 2 or 3 (RC4 up to 128-bit) when the user password is empty — the Encrypt dictionary is parsed, the file key is derived with the padded-password algorithm, each object's strings and stream data are decrypted with per-object RC4 keys, and the in-memory document no longer carries the `/Encrypt` entry
- Classic xref tables, including incremental update chains (multiple xref sections linked via `Prev`)
- PDF 1.5+ cross-reference streams (`/Type /XRef`) and the hybrid form where a legacy trailer carries an `XRefStm` pointer
- Object streams (`/Type /ObjStm`) — compressed objects are materialized into the regular object store during parsing
- Full-document rewrites on save (incremental updates and xref streams are flattened into a single classic-xref revision on output)
- Unfiltered or `FlateDecode` streams, including `FlateDecode` with the TIFF predictor (`/Predictor 2`) and PNG predictors 10–15 (via `DecodeParms /Predictor`)
- Page tree traversal with inherited resources, media boxes, crop boxes, and page rotation
- Inline images (`BI`/`ID`/`EI`) are safely skipped during content stream parsing
- Dictionary operands in content streams (e.g., `BDC` with `<</MCID 0>>`)
- Common text operators
- Common path, paint, and graphics-state operators (`q`, `Q`, `cm`, `gs`, `w`, `J`, `j`, `M`, `d`, `ri`, `i`)
- Clipping path operators (`W`, `W*`)
- Color operators for device and general color spaces (`rg`/`RG`, `g`/`G`, `k`/`K`, `cs`/`CS`, `sc`/`SC`, `scn`/`SCN`)
- Curve segment operators (`c`, `v`, `y`) — all three are included in path bounds used by vector paint neutralization
- Marked-content operators as safe pass-through (`BMC`, `BDC`, `EMC`, `MP`, `DP`)
- ExtGState font entries (fonts set via `gs` operator)
- Image XObject invocation detection
- `Type1` and `TrueType` fonts in the current text path, including `ToUnicode` CMap decoding, `/Encoding /WinAnsiEncoding` for non-ASCII bytes, and `/Encoding` dictionaries with a `/Differences` array resolved through an Adobe Glyph List subset
- Form XObjects (`/Subtype /Form`) traversed during text extraction and search, including the Form's `Matrix`, its own `Resources.Font` and `Resources.ExtGState`, and cycle-protected recursion
- `Type0` with `Identity-H`, two-byte CIDs, and `ToUnicode` maps
- Rectangle, quad, and quad-group redaction targets
- Three redaction modes: `strip` (remove bytes), `redact` (blank space + overlay), `erase` (blank space, no overlay)
- `overlayText` support for `redact` mode — labels are stamped inside each overlay rectangle using Helvetica sized to fit, with contrast-aware black or white text color against the fill
- Metadata stripping for supported document layouts
- Attachment stripping for supported embedded-file layouts

## Explicitly unsupported or incomplete

- Encrypted PDFs that require a non-empty user password, or that use Standard Security Handler V ≥ 3 (AES) — empty-password RC4 (V = 1/2, R = 2/3) is the only supported configuration today
- Public-key encryption handlers
- Incremental update preservation (output is always a flat rewrite; xref streams are rewritten as a classic xref table)
- Full redaction of text, vector paint, and Image XObject invocations inside Form XObjects via copy-on-write. Each Form whose `BBox × Matrix × CTM` intersects a target is cloned per page; the copy's content stream is rewritten to strip the targeted glyphs, neutralize any vector paint operator that falls under a target, and replace intersecting Image `Do` invocations with `n`. Nested Forms are handled recursively up to depth 8, with each outer Form's `Resources.XObject` repointed at the redacted inner copy; other pages that share the original Forms are left untouched.
- Redaction of documents whose catalog has `/OCProperties` with any layer off in the default configuration, or with `/BaseState /OFF`/`/Unchanged` — the engine rejects these up front because hidden-layer content would otherwise survive the redaction
- Type3 fonts
- broad CID font support beyond the current `Identity-H` path
- partial image rewriting
- optional-content group sanitization
- overlay text stamping

## Failure model

When unsupported content affects correctness, the engine returns an explicit error instead of pretending to succeed.

Typical behavior:

- unsupported operators on redacted pages return `Unsupported` (the operator allow-list covers common text, path, color, clipping, graphics-state, and marked-content operators)
- unimplemented plan options return `UnsupportedOption`
- malformed structure returns parse or corruption errors

## Security posture

This subset is intentionally conservative. If the engine cannot safely rewrite the targeted content, it should fail rather than emit a misleading "sanitized" PDF.

## Related docs

- [Security model](../security-model.md)
- [Roadmap](../roadmap.md)
