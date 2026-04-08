---
title: Supported PDF Subset
---

# Supported PDF Subset

This project intentionally targets a narrow, explicit MVP.

## Supported now

- Unencrypted PDFs
- Classic xref tables
- Full-document rewrites on save
- Unfiltered or `FlateDecode` streams
- Page tree traversal with inherited resources, media boxes, crop boxes, and page rotation
- Common text operators
- Common path and paint operators
- Image XObject invocation detection
- `Type1` and `TrueType` fonts in the current text path
- `Type0` with `Identity-H`, two-byte CIDs, and `ToUnicode` maps
- Rectangle, quad, and quad-group redaction targets
- Metadata stripping for supported document layouts
- Attachment stripping for supported embedded-file layouts

## Explicitly unsupported or incomplete

- Encrypted PDFs
- Object streams and xref streams
- Incremental update preservation
- Form XObjects on targeted pages
- Type3 fonts
- broad CID font support beyond the current `Identity-H` path
- partial image rewriting
- optional-content group sanitization
- overlay text stamping

## Failure model

When unsupported content affects correctness, the engine returns an explicit error instead of pretending to succeed.

Typical behavior:

- unsupported operators on redacted pages return `Unsupported`
- unimplemented plan options return `UnsupportedOption`
- malformed structure returns parse or corruption errors

## Security posture

This subset is intentionally conservative. If the engine cannot safely rewrite the targeted content, it should fail rather than emit a misleading "sanitized" PDF.

## Related docs

- [Security model](../security-model/)
- [Roadmap](../roadmap/)
