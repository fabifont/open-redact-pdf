---
title: Supported PDF Subset
---

# Supported PDF Subset

This project intentionally targets a narrow, explicit MVP.

## Supported now

- Unencrypted PDFs
- Classic xref tables, including incremental update chains (multiple xref sections linked via `Prev`)
- Full-document rewrites on save (incremental updates are flattened into a single revision on output)
- Unfiltered or `FlateDecode` streams
- Page tree traversal with inherited resources, media boxes, crop boxes, and page rotation
- Common text operators
- Common path, paint, and graphics-state operators (`q`, `Q`, `cm`, `gs`, `w`, `J`, `j`, `M`, `d`, `ri`, `i`)
- Clipping path operators (`W`, `W*`)
- Color operators for device and general color spaces (`rg`/`RG`, `g`/`G`, `k`/`K`, `cs`/`CS`, `sc`/`SC`, `scn`/`SCN`)
- Curve segment operators (`v`, `y`)
- Marked-content operators as safe pass-through (`BMC`, `BDC`, `EMC`, `MP`, `DP`)
- ExtGState font entries (fonts set via `gs` operator)
- Image XObject invocation detection
- `Type1` and `TrueType` fonts in the current text path
- `Type0` with `Identity-H`, two-byte CIDs, and `ToUnicode` maps
- Rectangle, quad, and quad-group redaction targets
- Three redaction modes: `strip` (remove bytes), `redact` (blank space + overlay), `erase` (blank space, no overlay)
- Metadata stripping for supported document layouts
- Attachment stripping for supported embedded-file layouts

## Explicitly unsupported or incomplete

- Encrypted PDFs
- Object streams and xref streams
- Incremental update preservation (output is always a flat rewrite)
- Form XObjects on targeted pages
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

- [Security model](../security-model/)
- [Roadmap](../roadmap/)
