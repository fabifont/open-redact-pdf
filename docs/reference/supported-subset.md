---
title: Supported PDF Subset
---

# Supported PDF Subset

This project intentionally targets a narrow, explicit MVP.

## Supported now

- Unencrypted PDFs
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
- Metadata stripping for supported document layouts
- Attachment stripping for supported embedded-file layouts

## Explicitly unsupported or incomplete

- Encrypted PDFs
- Incremental update preservation (output is always a flat rewrite; xref streams are rewritten as a classic xref table)
- Redaction of pages where a redaction target actually falls inside a Form XObject — the engine checks each Form's `BBox` (transformed through its `Matrix` and the current CTM) against the targets and only errors when they intersect; Forms that sit away from the targets are left untouched and the page is redacted normally
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
