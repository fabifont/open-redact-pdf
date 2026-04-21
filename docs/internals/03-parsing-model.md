# Parsing Model

This document is a deep dive into how `pdf_objects` parses a raw PDF byte slice into a structured `ParsedDocument`. It covers the entry point, each stage of the pipeline, the cursor abstraction, stream handling, and document building — plus the reasoning behind every significant design decision.

---

## 1. Entry point

```rust
pub fn parse_pdf(bytes: &[u8]) -> Result<ParsedDocument, PdfError>
```

`pdf_objects::parse_pdf` is the single public entry point for parsing. It accepts a raw byte slice and returns a `ParsedDocument` that owns the parsed object table, trailer, and page tree. Every other parsing function in the crate is an implementation detail called transitively from here.

---

## 2. Parsing pipeline

Parsing proceeds in six ordered stages. Each stage depends on the output of the previous one.

### Stage 1: `parse_header`

Checks that the file begins with `%PDF-` and extracts the version string (e.g., `1.7` or `2.0`). The version is stored on the resulting document but is not used to gate behavior at this stage — the engine treats all version strings uniformly unless a later stage encounters a version-specific construct it cannot handle.

### Stage 2: `find_startxref`

Scans **backwards** from the end of the file for the `startxref` keyword, then reads the decimal integer that follows it on the same or next line. This integer is the byte offset of the cross-reference table, which may be either a classic `xref` section or an indirect xref stream object (PDF 1.5+) — Stage 3 dispatches on which form is present.

Scanning backwards rather than forwards is mandated by the PDF specification: the `startxref` entry is always near the end of the file, and scanning from the beginning would be fragile in the presence of binary content that happens to contain the ASCII bytes of `startxref`.

### Stage 3: `parse_xref_table`

This is the most structurally complex stage. It resolves the entire cross-reference chain and merges all revisions into a single flat object table.

**Classic vs. stream sections.** At each offset the parser peeks at the next non-whitespace token. If it is the keyword `xref`, the section is a classic cross-reference table. Otherwise the parser treats the offset as an indirect object and decodes it as a PDF 1.5 cross-reference stream (`/Type /XRef`). Both forms return the same `(BTreeMap<ObjectRef, XrefEntry>, trailer)` tuple, so the merge logic downstream does not care which form produced the data.

**Xref stream decoding.** For a stream section the parser reads `Size`, `W` (field widths), and optional `Index` (default `[0 Size]`) from the stream dictionary, decodes the stream body through the shared filter + PNG-predictor pipeline, then walks the decoded bytes row by row. Each row is `sum(W)` bytes wide and encodes `(type, field2, field3)` big-endian:
- type 0 → `XrefEntry::Free`
- type 1 → `XrefEntry::Uncompressed { offset, generation }`
- type 2 → `XrefEntry::Compressed { stream_object_number, index }`

An entry with `W[0] == 0` defaults to type 1.

**Cycle detection.** A `BTreeSet<usize>` of visited byte offsets is maintained across the chain walk. When a section's trailer carries a `Prev` pointer, or a legacy trailer carries an `XRefStm` pointer at a hybrid form PDF, both offsets are pushed onto a pending stack. Offsets already visited are skipped, so a malformed file with a self-referential `Prev` simply terminates the walk instead of looping forever.

**Merge policy.** The table is built as a `BTreeMap<ObjectRef, XrefEntry>` using `BTreeMap::entry().or_insert()`. The chain is walked newest-revision-first (from the most recent `startxref` backward through each `Prev` / `XRefStm` pointer), so the first time any given `ObjectRef` is inserted it comes from the newest revision. Subsequent (older) entries for the same ref are ignored. This correctly implements incremental update semantics.

**Output.** Stage 3 returns the merged `BTreeMap<ObjectRef, XrefEntry>` and the newest trailer dictionary — which, for an xref-stream section, is simply the stream's own dictionary.

### Stage 4: Parse each uncompressed object

For each `XrefEntry::Uncompressed` in the merged xref table, `parse_indirect_object` is called at the recorded byte offset. It reads:
1. The object number and generation number from the `N G obj` header.
2. The object body — either a `PdfValue` or a stream (dictionary followed by `stream ... endstream`).
3. The `endobj` keyword.

The resulting `PdfObject` is inserted into the object table keyed by its `ObjectRef`. `XrefEntry::Compressed` entries are collected into a side list for the next stage. Free entries are skipped.

Stream parsing at this stage only reads and stores the raw (still-encoded) bytes. Decompression happens lazily on demand, never during the initial parse pass.

### Stage 5: Materialize object streams

For each `XrefEntry::Compressed { stream_object_number, index }` entry, the parser looks up the enclosing `/Type /ObjStm` stream (already in the object table from Stage 4), decodes its body, and parses its header. The header is `N` pairs of `(member_obj_num, relative_offset)` separated by whitespace; the decoded body after `First` bytes holds the serialized member values.

For each requested index the parser slices to `First + relative_offset`, parses a single `PdfValue` with the same cursor used for direct parsing, and inserts the result as `PdfObject::Value`. Streams are not allowed inside an ObjStm (per ISO 32000-1 § 7.5.7); a compressed member whose parsed value is itself a `/Type /ObjStm` dictionary is rejected with `PdfError::Unsupported`.

Compressed objects always have generation 0. The materialized object is keyed by the `ObjectRef` that the xref stream declared, and `max_object_number` is updated so that later allocations via `PdfFile::allocate_object_ref` do not collide.

### Stage 6: `build_document`

Validates the object table and page tree structure, then builds the final `ParsedDocument`. Details in section 5 below.

---

## 3. Cursor-based parsing

All low-level parsing is done through a `Cursor` struct:

```rust
struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}
```

The cursor holds a reference to the original byte slice and a mutable position. There are no heap allocations for intermediate tokens; the cursor reads directly from the input.

### `skip_ws_and_comments`

Advances past any combination of:
- Space (0x20), tab (0x09), carriage return (0x0D), line feed (0x0A), form feed (0x0C), NUL (0x00)
- `%` comment lines — consumes everything up to and including the next line ending

PDF defines whitespace broadly and allows comments almost anywhere between tokens. Handling them in a single method called before every token read keeps the rest of the parsing code clean.

### `parse_value`

The main dispatch method. Peeks at the first non-whitespace byte and routes to the appropriate sub-parser:

| First byte(s) | Target |
|---|---|
| `t`, `f` | Boolean (`true`, `false`) |
| `n` | Null |
| `(` | Literal string |
| `<` followed by `<` | Dictionary |
| `<` | Hex string |
| `[` | Array |
| `/` | Name |
| `0`–`9`, `-`, `+`, `.` | Number or indirect reference |

### `parse_name`

Reads a `/`-prefixed PDF name. Handles `#XX` hex escapes: a `#` followed by two hex digits is decoded to the corresponding byte value. This allows names to contain bytes that would otherwise be syntactically significant (e.g., `/Type#20Name` → `/Type Name`).

### `parse_literal_string`

Reads a `(`...`)` string with the following escape handling:
- `\n` → LF, `\r` → CR, `\t` → tab, `\b` → backspace, `\f` → form feed
- `\(` → `(`, `\)` → `)`, `\\` → `\`
- `\ddd` (1–3 octal digits) → the corresponding byte value
- `\` followed by a line ending → line continuation (the newline is discarded)
- Nested parentheses are tracked with a depth counter; the string ends at the unmatched `)`.

### `parse_hex_string`

Reads a `<`...`>` hex string. Whitespace within the angle brackets is filtered out (PDF allows it). If the number of hex digits is odd, a trailing `0` is appended before decoding, producing the correct behavior per the spec.

### `parse_number_or_reference`

PDF's indirect reference syntax (`N G R`) is ambiguous at the point of reading the first integer: it could be a standalone integer or the start of a three-token reference. This method resolves the ambiguity speculatively:

1. Parse the first integer `N`.
2. Save the cursor position.
3. Skip whitespace; try to parse a second integer `G`.
4. Skip whitespace; check for `R`.
5. If all three succeed → return `PdfValue::Reference(N, G)`.
6. Otherwise → restore the saved position and return `PdfValue::Integer(N)` (or `PdfValue::Number` if a decimal point was present).

The rollback is cheap (one integer assignment) and avoids the need for a lookahead buffer or a separate tokenization pass.

---

## 4. Stream parsing

A PDF stream object is a dictionary followed by the keyword `stream`, raw bytes, then `endstream`. The parser must know how many bytes to read, which creates a subtle bootstrap problem.

### Length resolution

The stream dictionary contains a `Length` entry giving the byte count of the raw data. The ideal approach would be to resolve `Length` as an indirect reference (many PDFs write `Length N 0 R`). But at the time each stream is being parsed, the object table is not yet fully built — we are currently in the process of building it. Resolving `Length` as a reference would require looking up an object that may not have been parsed yet.

The parser therefore reads `Length` only as a direct integer literal. If `Length` is a reference, the direct integer read fails and the parser falls through to the fallback.

### Fallback: scan for `endstream`

When `Length` is missing, wrong, or an unresolvable reference, the parser scans forward byte-by-byte for the literal sequence `endstream`. This is slower but handles the large fraction of real-world PDFs that use a reference for `Length` or have an inaccurate `Length` value.

### `consume_stream_line_break`

The PDF spec requires exactly one line ending (CR, LF, or CRLF) between the `stream` keyword and the first byte of data. The parser consumes exactly that sequence before recording the data start offset. Reading more or fewer bytes here would cause all stream offsets to be off by one.

---

## 5. Document building (`build_document`)

### Encryption check

If the trailer dictionary contains an `Encrypt` key, `parse_pdf_with_password` runs decryption in place before `build_document` sees the objects: `StandardSecurityHandler::open` authenticates the supplied password against the Encrypt dictionary and every encrypted string / stream is rewritten in-place to its plaintext bytes. Object streams are decrypted before materialization so their members parse as plaintext. The trailer's `/Encrypt` entry is then removed so downstream stages never observe the ciphertext.

Supported Encrypt configurations: V = 1 or 2 with R = 2 or 3 (RC4 up to 128-bit); V = 4 with R = 4 using the `/StdCF` crypt filter configured for `/CFM /V2` (RC4-128) or `/CFM /AESV2` (AES-128-CBC); V = 5 with R = 5 or R = 6 using `/CFM /AESV3` (AES-256-CBC) with file key unwrapped from `/OE` / `/UE` via Algorithm 2.A (and Algorithm 2.B's iterative hash on R=6). Either the user password or the owner password authenticates (Algorithm 2 + 4/5 and Algorithm 7 for V=1/2/4; Algorithm 2.A + 2.B for V=5). When `/EncryptMetadata false` is set on a V=4 document, the Algorithm 2 step-5 `0xFFFFFFFF` suffix is applied to the file key and `/Type /Metadata` streams are left in plaintext. Unsupported configurations (public-key handlers, `/CFM` methods other than `/V2`, `/AESV2`, and `/AESV3`) fail with `PdfError::Unsupported`; a wrong password fails with `PdfError::InvalidPassword`.

### Page tree traversal: `collect_pages`

PDF pages are organized in a `Pages` tree of arbitrary depth. `collect_pages` recurse walks the tree and flattens it into an ordered `Vec<Page>`, carrying inherited properties downward at each level.

**Inheritance.** The following properties are inherited from ancestor nodes if not defined on the leaf:
- `Resources` — font and graphics state dictionaries
- `MediaBox` — the nominal page rectangle
- `CropBox` — defaults to `MediaBox` when absent
- `Rotate` — page rotation in degrees; defaults to 0

**Depth limit.** `MAX_PAGE_TREE_DEPTH = 64`. If the recursion depth exceeds this value, `collect_pages` returns an error. A deeply nested page tree is almost certainly either malformed or adversarial; 64 levels is far beyond what any real document requires.

**Cycle detection.** A `BTreeSet<ObjectRef>` of visited page tree nodes is passed through the recursion. If a node references an object that is already in the visited set, traversal stops with an error. This prevents infinite loops on PDFs with cyclic `Kids` arrays.

---

## 6. Step-by-step walkthrough: parsing a tiny PDF

Consider this minimal valid PDF (simplified, showing structure):

```
%PDF-1.4
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj

2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 1 >>
endobj

3 0 obj
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>
endobj

xref
0 4
0000000000 65535 f
0000000009 00000 n
0000000058 00000 n
0000000115 00000 n
trailer
<< /Size 4 /Root 1 0 R >>
startxref
210
%%EOF
```

**Step 1 (`parse_header`):** The cursor reads `%PDF-1.4`. Version string `"1.4"` is recorded. Position advances past the newline.

**Step 2 (`find_startxref`):** The parser scans backward from the end. It finds `startxref` followed by `210`. Byte offset 210 is the xref table start.

**Step 3 (`parse_xref_table`):** The parser positions the cursor at byte 210 and peeks the next token. It is `xref`, so the classic section parser is used. It reads:
- Keyword `xref`
- Subsection header `0 4` — starting object number 0, four entries
- Four 20-byte entries:
  - Object 0, gen 65535: free (type `f`) — `XrefEntry::Free`
  - Object 1, gen 0: in-use at byte 9 (`n`) — `XrefEntry::Uncompressed`
  - Object 2, gen 0: in-use at byte 58 (`n`)
  - Object 3, gen 0: in-use at byte 115 (`n`)
- Trailer dictionary `<< /Size 4 /Root 1 0 R >>`

No `Prev` or `XRefStm` key in the trailer, so the chain walk stops. The merged xref table has three uncompressed entries. The trailer is stored.

**Step 4 (uncompressed object parsing):** For each `Uncompressed` entry:
- Byte 9: cursor reads `1 0 obj`, then dictionary `<< /Type /Catalog /Pages 2 0 R >>`, then `endobj`. Stored as `ObjectRef { 1, 0 } → PdfObject::Value(PdfValue::Dictionary(...))`.
- Byte 58: object 2 — the Pages node.
- Byte 115: object 3 — the Page leaf, containing a `MediaBox` array.

**Step 5 (object stream materialization):** No `Compressed` entries in this example, so the pass is a no-op.

**Step 6 (`build_document`):** No `Encrypt` key. Trailer has `/Root 1 0 R`. Object 1 is resolved; its `/Type` is `/Catalog` and `/Pages` points to object 2. `collect_pages` starts at object 2:
- Object 2 is a `Pages` node with `/Kids [3 0 R]` and `/Count 1`.
- Recursion enters object 3. It is a `Page` leaf.
- `MediaBox [0 0 612 792]` is read directly from object 3.
- `CropBox` defaults to `MediaBox`. `Rotate` defaults to 0.
- One `Page` is appended to the result vector.

`ParsedDocument` is returned with one page, version `"1.4"`, and three objects in the table.

---

## 7. Design rationale

**Cursor-based rather than tokenizer-based.** A separate tokenizer would produce an intermediate `Vec<Token>` before any structure is parsed. The cursor approach parses structure directly from bytes in a single pass. The code is simpler, the allocation profile is lower, and there is no impedance mismatch between the token stream and the byte-level details (like stream data offsets) that must be tracked precisely.

**Speculative reference parsing.** The three-token `N G R` syntax is not prefixed by a unique sigil, so the parser cannot know at byte 0 of the value whether it is reading an integer or a reference. The speculative rollback strategy handles this without backtracking buffers or two-pass parsing.

**Length hint with scan fallback.** Real-world PDFs routinely use `Length N 0 R` in stream dictionaries. Requiring a directly-encoded integer would reject those PDFs outright. The scan fallback is slower but makes the parser useful on documents generated by the majority of PDF producers.

**Incremental update flattening at parse time.** The xref chain walk and `or_insert` merge policy collapse all revisions into a single object table before any consumer sees the document. This means the rest of the engine — extraction, redaction, serialization — never needs to reason about revision history. The tradeoff is that revision history cannot be recovered after parsing; that is acceptable for a redaction engine, which only cares about the current logical state of the document.

---

## 8. What breaks if you change this carelessly

| Change | Consequence |
|---|---|
| Remove xref cycle detection (`BTreeSet<usize>`) | Infinite loop on any PDF with a self-referential `Prev` pointer |
| Change `or_insert` to `insert` | Old revisions overwrite new ones; deleted or replaced objects reappear |
| Resolve `Length` as an indirect reference during parse | Deadlock or panic — the object table is not built yet |
| Remove the `endstream` fallback | Any PDF where `Length` is a reference (common) produces a parse error |
| Remove the `endstream` fallback | Any PDF with an inaccurate `Length` (common) produces corrupt stream data |
| Remove `MAX_PAGE_TREE_DEPTH` | Stack overflow on deeply nested or cyclically structured page trees |
| Remove page tree cycle detection (`BTreeSet<ObjectRef>`) | Infinite recursion on cyclic `Kids` arrays |
| Materialize object streams before uncompressed objects | ObjStm lookup fails — the enclosing stream object is not yet in the table |
| Allow a compressed member to be another `/Type /ObjStm` | Silent corruption — nested object streams are forbidden by the spec and not unpacked here |
| Assume `W[0]` is always present | Xref streams that omit `W[0]` (default type 1) look like free entries and lose their data |
