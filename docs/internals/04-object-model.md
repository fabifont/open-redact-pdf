# Object Model and Serialization

This document is a deep dive into the core data types used to represent a parsed PDF in memory, the object resolution API, stream decoding, and the deterministic serialization pass. It explains the design choices behind each decision and identifies the failure modes that would result from changing them.

---

## 1. Core types

### `ObjectRef`

```rust
pub struct ObjectRef {
    pub object_number: u32,
    pub generation: u16,
}
```

`ObjectRef` is the PDF indirect object identifier. The `object_number` uniquely identifies the logical object slot; `generation` is incremented each time that slot is freed and reallocated (in practice almost always 0 in non-incremental PDFs).

`ObjectRef` derives `Ord` and `Eq`, which makes it usable as a `BTreeMap` key. The derived ordering is lexicographic on `(object_number, generation)`, which matches the natural ordering of PDF object numbers.

### `PdfValue`

```rust
pub enum PdfValue {
    Null,
    Bool(bool),
    Integer(i64),
    Number(f64),
    Name(String),
    String(PdfString),
    Array(Vec<PdfValue>),
    Dictionary(BTreeMap<String, PdfValue>),
    Reference(u32, u16),
}
```

Eight variants covering every PDF value type. Notable design choices:

- `Integer` and `Number` are separate variants rather than a single numeric type. PDF distinguishes them syntactically (no decimal point vs. decimal point), and several dictionary entries are required to be integers (e.g., `Length`, object numbers). Keeping them separate avoids silent precision loss and makes type mismatches detectable.
- `Dictionary` uses `BTreeMap<String, PdfValue>` rather than `HashMap`. See section 5 for the full rationale.
- `Reference` carries the raw `(object_number, generation)` pair rather than resolving eagerly. Resolution is explicit and on-demand (see section 2).

### `PdfString`

```rust
pub struct PdfString(pub Vec<u8>);
```

A PDF string is a raw byte sequence — not UTF-8, not Latin-1, not any specific encoding. PDF strings can carry arbitrary binary data (e.g., font glyph indices, encrypted payloads, raw Unicode in UTF-16BE). Storing them as `Vec<u8>` is correct; any encoding interpretation is the responsibility of the consumer, not the type.

### `PdfStream`

```rust
pub struct PdfStream {
    pub dict: BTreeMap<String, PdfValue>,
    pub data: Vec<u8>,
}
```

`data` contains the raw, still-encoded bytes as read from the file. Filters listed in the stream dictionary (e.g., `FlateDecode`) have not been applied. Decoding is done on demand via `decode_stream` (section 3).

### `PdfObject`

```rust
pub enum PdfObject {
    Value(PdfValue),
    Stream(PdfStream),
}
```

Every indirect object in the PDF file is either a plain value or a stream. This enum makes the distinction explicit at the type level, so callers that need a stream can pattern-match and get a compile error if they receive a value, and vice versa.

### `PdfFile`

```rust
pub struct PdfFile {
    pub version: String,
    pub objects: BTreeMap<ObjectRef, PdfObject>,
    pub trailer: BTreeMap<String, PdfValue>,
    pub max_object_number: u32,
}
```

The top-level document container. `objects` is the flat merged object table (all revisions collapsed). `trailer` is the newest trailer dictionary, with `Prev` and `XRefStm` removed (those keys are irrelevant after flattening). `max_object_number` tracks the highest object number allocated so far, used by `allocate_object_ref`.

---

## 2. Object resolution

### Basic lookups

```rust
fn get_object(&self, r: ObjectRef) -> Option<&PdfObject>
fn get_value(&self, r: ObjectRef) -> Option<&PdfValue>
fn get_dictionary(&self, r: ObjectRef) -> Option<&BTreeMap<String, PdfValue>>
```

These look up an `ObjectRef` in `self.objects` and return the result (or the inner variant) without any resolution. They are the building blocks for callers that already have a concrete `ObjectRef`.

### `resolve`

```rust
fn resolve<'a>(&'a self, value: &'a PdfValue) -> Option<&'a PdfValue>
```

Single-step indirect dereference. If `value` is `PdfValue::Reference(n, g)`, it looks up `ObjectRef { n, g }` in the object table and returns a reference to the stored value. If `value` is any other variant, it is returned unchanged.

`resolve` intentionally does **not** follow chains. If object A references object B which references object C, `resolve(A_ref)` returns the `Reference` to C, not the final value at C. PDF disallows reference chains (a reference must point to a direct object or a stream, never to another reference), so a single dereference is always sufficient for well-formed documents. Recursive resolution would silently tolerate malformed documents and would be vulnerable to infinite loops (section 6).

### `resolve_dict`

```rust
fn resolve_dict<'a>(&'a self, value: &'a PdfValue)
    -> Option<&'a BTreeMap<String, PdfValue>>
```

Convenience wrapper: `resolve` followed by an assertion that the resolved value is a `Dictionary`. This is the most common lookup pattern in the rest of the engine — nearly every named structure in a PDF (page, font, resources, annotation) is a dictionary stored as an indirect object.

### `allocate_object_ref`

```rust
fn allocate_object_ref(&mut self) -> ObjectRef
```

Increments `max_object_number` by 1 and returns `ObjectRef { object_number: max_object_number, generation: 0 }`. This is used by the redaction and writer stages when new objects (e.g., replacement content streams) must be added to the document. Generation 0 is always used for newly allocated objects, consistent with PDF convention.

---

## 3. Stream decoding (`decode_stream`)

```rust
pub fn decode_stream(stream: &PdfStream) -> Result<Vec<u8>, PdfError>
```

Applies the filters listed in the stream dictionary to decode `stream.data` into the raw uncompressed content.

### Supported filters

Only `FlateDecode` (zlib/deflate compression) is supported. If the stream dictionary specifies any other filter — `LZWDecode`, `CCITTFaxDecode`, `DCTDecode`, `JBIG2Decode`, `JPXDecode`, `ASCII85Decode`, `ASCIIHexDecode`, or any other — `decode_stream` returns `Err(PdfError::Unsupported)`.

This is a deliberate scope constraint. Supporting additional decoders would expand the attack surface and the maintenance burden without benefiting the redaction use case: redaction only needs to decode content streams (which are always `FlateDecode`) and font programs (also `FlateDecode`). Image data, which uses JPEG or JBIG2 filters, is not modified by redaction.

### Decompression bomb protection

```rust
const MAX_DECOMPRESSED_SIZE: u64 = 256 * 1024 * 1024; // 256 MiB
```

Decompression uses:

```rust
ZlibDecoder::new(data)
    .take(MAX_DECOMPRESSED_SIZE + 1)
    .read_to_end(&mut buf)
```

The `.take(limit + 1)` pattern reads at most `limit + 1` bytes. If the result is exactly `limit + 1` bytes, the decompressed stream exceeds the limit and an error is returned before the full expansion is allocated. This stops decompression bomb attacks — adversarial input that compresses to a small size but expands to gigabytes — without requiring a two-pass decompression.

---

## 4. Serialization (`serialize_pdf`)

```rust
pub fn serialize_pdf(file: &PdfFile) -> Vec<u8>
```

Produces a complete, valid PDF byte sequence from a `PdfFile`. The output is a full-save rewrite — no incremental updates, no `Prev` pointer, no revision history. This is the only serialization path.

### Determinism

The serializer produces byte-for-byte identical output for identical input. This is achieved by:
- Iterating `BTreeMap` (objects and dictionaries), which yields entries in a stable sorted order.
- Using no random number generators or hash-seed-dependent data structures anywhere in the output path.
- Applying the same number formatting rules unconditionally (see below).

Deterministic output is critical for testing: a round-trip test can assert exact byte equality rather than approximate structural equivalence.

### Trailer cleanup

Before serializing the trailer, two keys are unconditionally removed:
- `Prev` — the byte offset of the previous xref section. After a full-save rewrite the previous xref no longer exists, so a reader following `Prev` would jump to an invalid offset.
- `XRefStm` — a reference to an xref stream used in hybrid-reference PDFs. After rewriting, there is no xref stream.

The `Size` entry is updated to reflect the current number of objects.

### Stream length update

Before writing each stream object, the `Length` entry in the stream dictionary is updated to the current byte length of `stream.data`. This ensures that the serialized `Length` value is always accurate and never a stale reference to the original source offset.

### Number formatting

```
if value.fract() == 0.0:
    write as integer (no decimal point)
else:
    write with up to 6 decimal places, trailing zeros removed
```

Examples:
- `1.0` → `1`
- `72.5` → `72.5`
- `0.333333...` → `0.333333`
- `612.000000` → `612`

This matches the convention used by most PDF producers and keeps output compact.

### Name encoding

PDF names are written with a leading `/`. Any byte that is not printable ASCII (bytes 0x00–0x20 or 0x7F–0xFF) or that is syntactically significant (`#`, `/`, `(`, `)`, `<`, `>`, `[`, `]`, `{`, `}`, `%`) is encoded as `#XX` where `XX` is the uppercase hexadecimal representation of the byte value.

### String encoding

Literal strings are written with `(` and `)` delimiters. Within the string:
- `(` and `)` are escaped as `\(` and `\)`.
- `\` is escaped as `\\`.
- CR (0x0D) → `\r`, LF (0x0A) → `\n`, tab (0x09) → `\t`, backspace (0x08) → `\b`, form feed (0x0C) → `\f`.
- Other non-printable bytes are escaped as `\ddd` (octal).

Printable ASCII bytes that are not syntactically significant are written as-is.

---

## 5. Why `BTreeMap` everywhere

All maps in the object model — the object table, dictionary values, and the trailer — use `BTreeMap` rather than `HashMap`. The reasons:

**Deterministic iteration order.** `BTreeMap` iterates keys in sorted order. `HashMap` iterates in an order that depends on the hash seed, which is randomized at program startup (since Rust 1.0) to prevent hash-flooding denial-of-service attacks. The same input processed by `HashMap` will produce different output byte sequences on different runs.

**Testability.** Deterministic output means that integration tests can assert `output == expected_bytes` rather than parsing the output and comparing it structurally. Structural comparison is harder to write, harder to read, and more likely to miss regressions in fields that are present but have wrong values.

**No external seed.** In a WASM environment, seeding a `HashMap` from the OS entropy source may behave differently than in a native environment. `BTreeMap` has no seed dependency and behaves identically everywhere.

The performance cost of `BTreeMap` (O(log n) vs. O(1) average for `HashMap`) is negligible for PDF object counts, which are typically in the hundreds to low thousands.

---

## 6. What breaks if you change this carelessly

| Change | Consequence |
|---|---|
| Replace `BTreeMap` with `HashMap` for objects or dictionaries | Non-deterministic output byte order; round-trip tests become flaky or must be rewritten as structural comparisons |
| Replace `BTreeMap` with `HashMap` for the object table | Object serialization order becomes non-deterministic; xref byte offsets in the output are still correct but testing is harder |
| Remove `Prev` from the serialized trailer | No immediate failure, but readers following the stale `Prev` offset will land in the middle of an unrelated object; some readers reject the document |
| Remove `XRefStm` from the serialized trailer | Readers may attempt to locate a compressed xref stream that does not exist in the rewritten file |
| Make `resolve` follow reference chains recursively | Infinite loop on any PDF where object A's value is `Reference(B)` and object B's value is `Reference(A)` — forbidden by spec, easily constructed by an adversary |
| Remove `MAX_DECOMPRESSED_SIZE` | A single compressed stream in a malicious PDF can allocate gigabytes of memory, causing OOM or process termination in the browser tab |
| Change `take(limit + 1)` to `take(limit)` | A stream of exactly `limit` bytes returns without error even if the decompressor has more data to produce; the cap becomes silent truncation rather than an error |
| Not updating `Length` before serializing streams | The serialized `Length` value may be stale (e.g., after redaction shrinks a content stream); readers that trust `Length` exactly will misparse subsequent objects |
| Eager resolution in `resolve` (follow chains) | Breaks on any compliant PDF where the spec would require a direct reference (e.g., `Length` must resolve to an integer, not to a reference to an integer); also enables infinite loop on adversarial input |
