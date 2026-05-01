# Writing and Saving Sanitized PDFs

## 1. Overview

The output pipeline is deliberately simple. `pdf_writer::save_document()` wraps `pdf_objects::serialize_pdf()`. The writer produces a clean, single-revision PDF whose cross-reference shape mirrors the input: classic-input PDFs save as classic xref + trailer; xref-stream-shaped inputs save as a `Type /XRef` stream with eligible objects packed into `Type /ObjStm` containers.

## 2. Serialization dispatcher (`serialize_pdf`)

`serialize_pdf` reads `PdfFile.xref_form` (set by the parser based on the section at `startxref`) and dispatches:

- `XrefForm::Classic` → `serialize_classic`
- `XrefForm::Stream` → `serialize_with_xref_stream`

Both paths write the same `%PDF-<version>\n%\xFF\xFF\xFF\xFF\n` header and serialize indirect objects with the same `serialize_value` / `serialize_dictionary` helpers; they differ only in how the cross-reference and the eligible objects are packaged.

## 3. Classic xref path (`serialize_classic`)

1. Write header.
2. Walk `file.objects` in `BTreeMap` order (deterministic by object number) and emit each indirect object. For streams: update `/Length` to `data.len()`, ensure a newline before `endstream`.
3. Build a classic xref table with one row per object number from 0 to `max_object_number`. Missing slots emit a free entry (`0000000000 65535 f \n`).
4. Write trailer dictionary; **always strip `/Prev` and `/XRefStm`** so readers cannot follow the chain to pre-redaction revisions.
5. Write `startxref <offset>\n%%EOF\n`.

## 4. Modern xref-stream path (`serialize_with_xref_stream`)

1. **Partition.** Split `file.objects` into "compressible" (`PdfObject::Value` with generation 0) and "direct" (everything else: streams, generation > 0). The Encrypt dict and the original ObjStm containers are already gone — the parser drops both at parse time so their bytes never reach the writer.
2. **Pack into ObjStms.** Group compressible objects into chunks of up to 100 members. For each chunk, build an ObjStm body: a header `objnum_1 offset_1 objnum_2 offset_2 …` followed by the serialized values back-to-back, Flate-compressed. Allocate fresh container object numbers from `file.max_object_number + 1`.
3. **Emit objects.** Write the header, then every direct object, then every freshly-built ObjStm container. Capture each emitted object's byte offset.
4. **Build xref rows.** One row per object number `0..xref_size`:
   - Object 0: type 0 (free).
   - Each direct object: type 1 (offset, generation).
   - Each compressed member: type 2 (objstm_container_objnum, member_index).
   - Each new ObjStm container: type 1 (offset, 0).
   - The xref-stream object itself: type 1 (its own offset, 0) — captured after the entry table is built.
5. **Pick W array.** Field 1 is always 1 byte (type fits 0..=2). Field 2 is `bytes_to_fit(max_offset)` (typically 3 for small files, 4 for >16 MB, 5 for >4 GB). Field 3 is `bytes_to_fit(max(member_index, max_object_number))`.
6. **Build the xref-stream dict.** Carry forward all trailer keys MINUS `/Prev`, `/XRefStm`, `/Encrypt`, `/Length`, `/Filter`, `/DecodeParms`, `/W`, `/Index`, `/Type`. Insert `/Type /XRef`, `/Size`, `/W`, `/Filter /FlateDecode`, and `/Length` (after Flate-compressing the entry body).
7. **Emit the xref-stream object** as the last indirect object; capture its offset as `startxref`.
8. Write `startxref <offset>\n%%EOF\n` (no separate trailer keyword in the stream form).

## 5. Object eligibility for ObjStm packing

Per ISO 32000-1 §7.5.7, object streams cannot contain:

- The xref stream itself.
- Other object streams (nested ObjStms are forbidden).
- Indirect objects with generation != 0.
- Stream objects.

The writer's partition step enforces all of these by inspection: `PdfObject::Stream` always falls into the direct bucket, generation > 0 always falls into direct, and the parser-side cleanup guarantees the Encrypt dict and source ObjStm containers are gone before the writer runs.

## 6. Value serialization (shared)

- **Numbers**: zero fractional part → no decimal point; otherwise 6 decimal places with trailing zeros trimmed.
- **Names**: `#XX` hex escapes for non-printable bytes, delimiter characters, and `#` itself.
- **Strings**: escape `(`, `)`, `\`; named escapes for control characters; octal for non-printable bytes.
- **Dictionaries**: keys written in BTree order (alphabetical).

## 7. Why incremental updates are flattened (both paths)

The writer always produces a single-revision document. This is critical for security:

- `/Prev` and `/XRefStm` removal prevents readers from following the chain to old revisions.
- A redacted document with old revisions would leak the unredacted content to any reader that walks the chain.
- Single revision = simpler output, no revision history, no need to preserve cross-revision invariants.

## 8. Parse-time cleanup of dangling objects

To support both the leak-prevention property and the writer's mirror-input-shape goal, the parser drops two classes of objects after their content has been consumed:

- **Encrypt dictionary** — `decrypt_document_if_encrypted` removes it from `file.objects` immediately after the trailer's `/Encrypt` reference is stripped. Without this, the decrypted password verifiers (`/O`, `/U`, `/OE`, `/UE`, `/Perms`) would survive as a dangling unreferenced object in the saved file.
- **ObjStm containers** — `materialize_object_streams` removes each ObjStm container after copying its members into the top-level objects map. Without this, the writer would re-emit the original Flate-compressed bytes — which mirror the pre-redaction state of every member dictionary.

## 9. Why new content streams are uncompressed

The redaction engine writes new content stream bytes without re-compression (no `FlateDecode` filter applied). Reasons:

- **Simplicity**: avoids re-encoding complexity for the rewriter.
- **Debuggability**: uncompressed streams are human-readable during development and debugging.
- The original compressed streams are replaced entirely, not patched.
- **Trade-off**: slightly larger output files.

(Note: the modern path *does* Flate-compress the xref stream and every ObjStm body, because those are produced by the writer itself and the spec expects them compressed.)

## 10. The pdf_writer crate

Still a one-function passthrough (`save_document` → `serialize_pdf`). It exists as a named boundary:

- Allows future enhancements (object renumbering, content-stream compression, linearization) without touching `pdf_objects`.
- Keeps the dependency graph clean: callers depend on `pdf_writer`, not on `pdf_objects` serialization internals.

## 11. What would break

| Omission | Consequence |
|---|---|
| Not removing `/Prev` | Old unredacted content accessible via Prev chain — security violation |
| Not updating `/Length` | Readers cannot find `endstream`; file corrupted |
| `HashMap` instead of `BTreeMap` | Non-deterministic output order; tests become flaky |
| Not flattening to single revision | Pre-redaction content survives in the file |
| Not dropping the Encrypt dict on parse | Decrypted password verifiers leak into saved bytes |
| Not dropping ObjStm containers on parse | Pre-redaction state of structural dictionaries leaks via the original compressed bytes |
| Mismatching `W` widths in xref stream | Readers misalign rows and decode wrong offsets |
| Forgetting to compress ObjStm bodies | Saved file is bigger than input; some readers reject uncompressed ObjStm |
