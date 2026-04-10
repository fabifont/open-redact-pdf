# Writing and Saving Sanitized PDFs

## 1. Overview

The output pipeline is deliberately simple. `pdf_writer::save_document()` wraps `pdf_objects::serialize_pdf()`. The writer produces a clean, single-revision PDF with a classic xref table.

## 2. Serialization process (`serialize_pdf`)

1. Write header: `%PDF-<version>\n%\xFF\xFF\xFF\xFF\n` (high bytes trigger binary detection in transfer tools)
2. Write each object in `BTreeMap` order (deterministic by object number)
3. For streams: update `Length` to current `data.len()`, ensure newline before `endstream`
4. Build single xref table with byte offsets
5. Write trailer dictionary — crucially **removes `Prev` and `XRefStm`** keys
6. Write `startxref` and `%%EOF`

## 3. Value serialization

- **Numbers**: zero fractional part → no decimal point; otherwise 6 decimal places with trailing zeros trimmed
- **Names**: `#XX` hex escapes for non-printable bytes, delimiter characters, and `#` itself
- **Strings**: escape `(`, `)`, `\`; named escapes for control characters; octal for non-printable bytes
- **Dictionaries**: keys written in BTree order (alphabetical)

## 4. Why incremental updates are flattened

The writer always produces a single-revision document. This is critical for security:

- `Prev` and `XRefStm` removal prevents readers from following the chain to old revisions
- A redacted document with old revisions would leak the unredacted content to any reader that follows the `Prev` pointer
- Single revision = simpler output, smaller file, no revision history

## 5. Why new content streams are uncompressed

The redaction engine writes new content stream bytes without re-compression (no `FlateDecode` filter applied). Reasons:

- **Simplicity**: avoids re-encoding complexity
- **Debuggability**: uncompressed streams are human-readable during development and debugging
- The original compressed streams are replaced entirely, not patched
- **Trade-off**: slightly larger output files

## 6. The pdf_writer crate

Currently a one-function passthrough (`save_document` → `serialize_pdf`). It exists as a named boundary:

- Allows future enhancements (object renumbering, compression, linearization) without touching `pdf_objects`
- Keeps the dependency graph clean: callers depend on `pdf_writer`, not on `pdf_objects` serialization internals

## 7. What would break

| Omission | Consequence |
|---|---|
| Not removing `Prev` | Old unredacted content accessible via Prev chain — security violation |
| Not updating `Length` | Readers cannot find `endstream`; file corrupted |
| `HashMap` instead of `BTreeMap` | Non-deterministic output order; tests become flaky |
| Not flattening to single revision | Pre-redaction content survives in the file |
