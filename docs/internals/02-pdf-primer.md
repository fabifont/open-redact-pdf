# PDF Primer for Contributors

This document explains the PDF file format from first principles. It is aimed at contributors who are comfortable with systems programming and binary formats but have not worked with PDF internals before. Every term is defined the first time it appears.

---

## 1. What is a PDF file?

PDF (Portable Document Format) is a **binary format**, not a text format. While it contains some ASCII-readable tokens, many parts are compressed binary data and arbitrary byte sequences. Opening a PDF in a text editor will show mostly noise.

The file is structured into four major sections:

```
%PDF-1.7                    <- header (version)
...body (objects)...        <- indirect objects
xref                        <- cross-reference table
0 N                         <- N entries
0000000000 65535 f          <- entry 0 (always free)
0000000009 00000 n          <- entry 1: object 1 at byte offset 9
...
trailer
<< /Size N /Root 1 0 R >>  <- trailer dictionary
startxref
441                         <- byte offset of xref
%%EOF
```

The critical insight is that **PDF is read from the end**. A PDF reader:

1. Seeks to the end of the file to find `%%EOF`.
2. Searches backward to find `startxref` and reads the byte offset that follows it.
3. Seeks to that offset to load the **xref table** (or xref stream).
4. Reads the **trailer dictionary** which points to the document root via the `/Root` key.
5. Follows object references from there to render the document.

This design allows incremental appending (covered in section 5) without rewriting the entire file.

---

## 2. Objects

PDF defines eight basic value types (plus null):

| Type | Example | Notes |
|---|---|---|
| Boolean | `true`, `false` | |
| Integer | `42`, `-7` | |
| Real | `3.14`, `-0.5` | |
| Literal string | `(Hello, world\n)` | Backslash escapes, parentheses nest |
| Hex string | `<48656C6C6F>` | Pairs of hex digits |
| Name | `/FlateDecode` | Starts with `/`, no spaces |
| Array | `[1 2 3]` | Space-separated values in brackets |
| Dictionary | `<< /Key Value >>` | Key-value pairs; keys are names |
| Stream | dictionary + binary data | Covered in section 3 |
| Null | `null` | Absence of a value |

### Indirect objects

Any object can be made **indirect** — given a permanent identity so other parts of the file can reference it. An indirect object looks like:

```
5 0 obj
<< /Type /Page ... >>
endobj
```

The `5` is the **object number** and `0` is the **generation number** (almost always 0 in modern files; it increments when an object is deleted and reused). Together they form the **object reference** `5 0 R`.

To look up object `5 0 R`, a reader consults the xref table to find its byte offset, seeks there, and reads the object.

In this engine, object references are represented by `ObjectRef { object_number: u32, generation: u16 }` in `crates/pdf_objects/src/types.rs`. The full object model is in `PdfValue` and `PdfObject` in the same file.

---

## 3. Streams

A **stream** is a dictionary followed by an arbitrary sequence of bytes:

```
7 0 obj
<< /Length 44 /Filter /FlateDecode >>
stream
<compressed bytes here>
endstream
endobj
```

Rules:
- The `/Length` key gives the byte count of the data between `stream\n` and `endstream`.
- If `/Filter` is present, the data is encoded. After reading the raw bytes you must decode them.
- `FlateDecode` means zlib/deflate compression — by far the most common filter.
- Multiple filters can be chained in an array: `/Filter [/ASCII85Decode /FlateDecode]`.

Streams are used for: page content, images, fonts, ICC color profiles, and the xref table itself (in PDF 1.5+).

In this engine, a decoded stream is a `PdfStream { dict: PdfDictionary, data: Vec<u8> }`. Decoding is done in `crates/pdf_objects/src/stream.rs`.

---

## 4. The Cross-Reference Table (xref)

The **xref table** maps object numbers to their byte offsets in the file. It has a fixed-width format to allow O(1) random access:

```
xref
0 6
0000000000 65535 f \r\n
0000000009 00000 n \r\n
0000000058 00000 n \r\n
0000000115 00000 n \r\n
0000000266 00000 n \r\n
0000000557 00000 n \r\n
```

Anatomy:
- `xref` keyword introduces the table.
- `0 6` means "starting at object 0, there are 6 entries."
- Each entry is exactly 20 bytes: 10-digit byte offset, space, 5-digit generation, space, `n` (in-use) or `f` (free), two whitespace bytes (CRLF or space+LF).
- Object 0 is always a free entry with generation 65535.

Multiple subsections can appear in one xref table (when not all objects are numbered consecutively):

```
xref
0 1
0000000000 65535 f
5 3
0000000123 00000 n
0000000234 00000 n
0000000345 00000 n
```

**PDF 1.5+ xref streams**: The xref can also be stored as a compressed stream object (`/Type /XRef`) whose body is a sequence of fixed-width binary rows describing each entry. The same stream object can also point at an **object stream** (`/Type /ObjStm`) where most indirect objects are packed together and Flate-compressed. The engine parses both forms and maps them into the same internal object table; on save, it always re-emits a classic xref table and inline indirect objects.

---

## 5. Incremental Updates

PDF supports **incremental updates**: instead of rewriting the whole file, new or changed objects are appended to the end, followed by a new xref table and trailer.

```
... original file ...
%%EOF

(appended revision)
8 0 obj ... endobj   <- new or changed objects
xref
8 1
0000001234 00000 n
trailer
<< /Size 9 /Root 1 0 R /Prev 441 >>
startxref
1234
%%EOF
```

The `/Prev` key in the new trailer points to the byte offset of the previous xref. A reader follows this chain from newest to oldest; **the newest definition of any object wins**.

This engine reads the full Prev chain and flattens it into a single in-memory object map during parsing (see `crates/pdf_objects/src/parser.rs`). On save, it rewrites the entire file as a single revision — incremental structure is not preserved in the output.

---

## 6. Document Structure

The structure from trailer to visible content:

```
Trailer dictionary
    /Root → Catalog (1 0 R)

Catalog
    /Pages → Pages tree root (2 0 R)

Pages tree root (node)
    /Kids → [3 0 R, 4 0 R, ...]   <- child nodes or leaf pages
    /Count → total page count
    /MediaBox, /Resources, ...     <- inherited by children

Page (leaf)
    /Parent → parent node
    /MediaBox → [0 0 612 792]      <- required; page boundary
    /Resources → resource dict
    /Contents → content stream (or array of streams)
```

**Inheritance**: Many keys (`MediaBox`, `Resources`, `Rotate`, `CropBox`) can appear at any node in the Pages tree and are inherited by all descendants. A child value overrides a parent value.

In this engine, `PageInfo` (defined in `crates/pdf_objects/src/document.rs`) is a flattened representation of a single page after inheritance is resolved.

---

## 7. Page Geometry

### MediaBox and CropBox

`/MediaBox` is a required rectangle `[x_min y_min x_max y_max]` that defines the physical page boundary in **user space units** (points, where 1 point = 1/72 inch).

`/CropBox` (optional) is the visible area shown to users. It defaults to `MediaBox`. This engine uses `CropBox` as the visible page boundary.

Standard US Letter: `[0 0 612 792]` (8.5 × 11 inches).
Standard A4: `[0 0 595.28 841.89]`.

### Coordinate system

PDF uses a **lower-left origin**: x increases to the right, y increases upward. This is opposite to screen coordinates (which increase downward). When converting PDF coordinates to screen coordinates, you must flip the y-axis.

### Rotate

`/Rotate` is an integer (0, 90, 180, or 270) giving the clockwise rotation to apply when rendering the page. A rotation of 90 degrees means the long edge of the MediaBox is displayed as the page width.

The engine's `PageBox::normalized_transform()` produces a matrix that handles both the crop-box offset and the rotation, producing a **normalized page space** where `(0, 0)` is the bottom-left of the visible area and all coordinates are non-negative.

---

## 8. Content Streams

A page's content stream is a sequence of **operators** with their **operands**. PDF uses **postfix (reverse Polish) notation**: operands come before the operator they apply to.

```
% Set line width to 2 points
2 w

% Move to (100, 700), draw line to (200, 700), stroke it
100 700 m
200 700 l
S

% Show text at position (72, 600), 12-point Helvetica
/Helvetica 12 Tf
72 600 Td
(Hello, PDF) Tj
```

All operators are ASCII keywords. Operands are PDF values (numbers, names, strings, arrays). Comments start with `%`.

A page may have multiple content streams listed in `/Contents` as an array; they are concatenated and treated as one stream.

The engine parses content streams into a `ParsedPageContent` (via `parse_page_contents`) or a `ContentStream` (via `parse_content_stream`) in `crates/pdf_content/src/content.rs`. A `ContentStream` contains a `Vec<Operation>`, where each `Operation` holds an operator string and a `Vec<PdfValue>` of operands.

---

## 9. Graphics State

The **graphics state** is a collection of rendering parameters maintained by a stack-based state machine as the content stream is interpreted:

- **CTM** (Current Transformation Matrix): maps user-space coordinates to device space.
- Current color (stroke and fill), line width, line cap, line join.
- Clip path.
- Text state (font, size, spacing, etc.).

Two operators manage the stack:
- `q` — push a copy of the current graphics state.
- `Q` — pop and restore the previous state.

These bracket isolated drawing operations that should not affect the surrounding state:

```
q
  0.5 g         % set fill gray to 50%
  100 100 200 50 re f  % draw a filled rectangle
Q               % restore previous fill color
```

In this engine, `GraphicsState` in `crates/pdf_content/src/content.rs` tracks the CTM and text state for the text extraction path. The redaction path re-interprets the same operations.

---

## 10. Text Rendering

Text is bracketed by `BT` (begin text) and `ET` (end text). Between these markers, text-specific operators apply:

| Operator | Operands | Effect |
|---|---|---|
| `Tf` | name size | Set current font and font size |
| `Tm` | a b c d e f | Set text matrix (and line matrix) |
| `Td` | tx ty | Move text position by (tx, ty) |
| `TD` | tx ty | Move text position, also set leading to −ty |
| `T*` | — | Move to start of next line using current leading |
| `Tj` | string | Show string |
| `TJ` | array | Show string with individual glyph adjustments |
| `'` | string | Move to next line, show string |
| `"` | aw ac string | Set word/char spacing, move, show |
| `Tr` | integer | Set text rendering mode |
| `Ts` | number | Set text rise (superscript/subscript offset) |
| `Tc` | number | Set character spacing |
| `Tw` | number | Set word spacing |
| `TL` | number | Set text leading |
| `Tz` | number | Set horizontal scaling (percent) |

### Text rendering mode (`Tr`)

The rendering mode controls how glyphs are painted:

| Mode | Name | Appearance |
|---|---|---|
| 0 | Fill | Normal filled text (default) |
| 1 | Stroke | Outlined text |
| 2 | Fill then stroke | Thick outlined text |
| 3 | Invisible | Glyph is positioned but not drawn |
| 4–7 | Clip variants | Used to clip subsequent drawing |

**Mode 3 is important for OCR layers**: scanned PDFs often place invisible text (mode 3) over an image so the PDF is searchable without showing a second copy of the text. This engine includes invisible glyphs in the extraction and redaction paths but excludes them from the visible search index.

---

## 11. Fonts

### Simple fonts (Type1, TrueType)

Simple fonts use **one byte per glyph code**. The glyph code is looked up in the font's encoding to determine which glyph to draw. The `/Widths` array (indexed from `/FirstChar`) gives the advance width of each glyph in **1/1000 em** units.

Decoding text: each byte in the string operand is one glyph code. Without a `ToUnicode` map, ASCII-range codes are usually their ASCII characters, but non-ASCII codes are unreliable.

### Composite fonts (Type0)

Composite fonts use **variable-width glyph codes called CIDs** (Character IDs). The most common encoding in practice is `Identity-H`: each CID is a 2-byte big-endian integer. So a 4-byte string represents 2 glyphs.

Type0 fonts carry a `DescendantFonts` array with one CIDFont object that provides widths via the `/W` array (a compact run-length format). They also carry a `ToUnicode` CMap stream.

This engine supports Type1, TrueType, and Type0 fonts with Identity-H encoding. Other Type0 encodings are rejected with `PdfError::Unsupported`.

### ToUnicode CMap

A `ToUnicode` stream maps glyph codes to Unicode strings using a subset of PostScript CMap syntax:

```
beginbfchar
<0041> <0041>    % CID 0x41 → U+0041 'A'
endbfchar
beginbfrange
<0020> <0039> <0020>   % CIDs 0x20–0x39 map linearly from U+0020
endbfrange
```

Without a `ToUnicode` map, text extraction falls back to treating ASCII-range codes as their ASCII equivalents. Non-ASCII codes without a map produce U+FFFD (replacement character).

---

## 12. The CTM and Coordinate Spaces

PDF defines a hierarchy of coordinate spaces:

```
Glyph space      (font units, typically 1/1000 em)
    |  Tf (font size scales glyph to text space)
    v
Text space       (em-relative units)
    |  Tm (text matrix) and line matrix
    v
User space       (points; origin, rotation per content stream)
    |  CTM (current transformation matrix)
    v
Device space     (physical pixels; handled by renderer)
```

For this engine, there is an additional step — **page transform** — that converts from PDF user space into the engine's normalized page space:

```
User space
    |  CTM
    v
(intermediate)
    |  page_transform (crop + rotate normalization)
    v
Normalized page space   (origin at bottom-left of CropBox, non-negative coords)
```

### The cm operator

The `cm` operator takes six numbers `a b c d e f` and **concatenates** a new matrix onto the CTM:

```
CTM' = M_new * CTM_old
```

This is **pre-multiplication**: the new matrix is applied first, then the old CTM. This matches the mathematical convention where later transforms appear to the left.

In the engine source (`crates/pdf_text/src/text.rs`):

```rust
"cm" => {
    let matrix = matrix_from_operands(&operation.operands)?;
    ctm = matrix.multiply(ctm);  // matrix.multiply(ctm) = M_new * CTM_old
}
```

### Matrix representation

The six values `a b c d e f` represent a 3×3 affine matrix (the third row is always `0 0 1`):

```
| a  b  0 |
| c  d  0 |
| e  f  1 |
```

Point transformation (row-vector convention):

```
[x' y' 1] = [x y 1] * M
x' = x*a + y*c + e
y' = x*b + y*d + f
```

### Glyph position computation

The full pipeline from glyph local space to normalized page space:

```
local_rect (in text units)
    → .transform(text_matrix * CTM * page_transform)
    → quad in normalized page space
```

The text matrix (`Tm`) combines the Tm/Td/TD/T* operators. Each glyph advances the text matrix horizontally by its width in user-space units before the next glyph is placed.

---

## 13. XObjects

An **XObject** is a self-contained content unit referenced by name from the page's Resources dictionary and invoked with the `Do` operator:

```
/Logo Do
```

There are two kinds relevant to this engine:

### Image XObjects

An image XObject is a stream containing pixel data. Key dictionary entries:

| Key | Meaning |
|---|---|
| `/Width`, `/Height` | Pixel dimensions |
| `/ColorSpace` | Color space (e.g. `/DeviceRGB`) |
| `/BitsPerComponent` | Bits per channel |
| `/Filter` | Compression filter |

Image XObjects have no internal coordinate system. When invoked, the CTM determines where and how they are painted — a `1×1` unit square in user space maps to the full image.

### Form XObjects

A form XObject is a stream containing arbitrary PDF operators — essentially a self-contained sub-page. It has:
- Its own content stream.
- Its own Resources dictionary.
- A `/BBox` defining its coordinate extent.
- An optional `/Matrix` applied when it is invoked.
- Its own graphics state scope (implicitly wrapped in q/Q).

Form XObjects are used for repeated elements (logos, headers, backgrounds), watermarks, and PDF stamps. When `Do` invokes a form XObject, the content stream executes within the current graphics state plus the form's own matrix.

This engine neutralizes `Do` operators for image XObjects that intersect a redaction target. Full form XObject traversal for redaction is currently not supported.

---

## ASCII Structure Diagram

```
File
 ├─ Header (%PDF-x.y)
 ├─ Body
 │   ├─ 1 0 obj (Catalog)
 │   ├─ 2 0 obj (Pages root)
 │   ├─ 3 0 obj (Page)
 │   │   ├─ /MediaBox [0 0 612 792]
 │   │   ├─ /Resources << /Font << /F1 4 0 R >> >>
 │   │   └─ /Contents 5 0 R
 │   ├─ 4 0 obj (Font)
 │   └─ 5 0 obj (Content stream)
 │       └─ BT /F1 12 Tf 72 720 Td (Hello) Tj ET
 ├─ xref (cross-reference table)
 ├─ trailer (<< /Size 6 /Root 1 0 R >>)
 ├─ startxref
 └─ %%EOF
```

---

## Further Reading

The normative specification is **ISO 32000-2:2020** (PDF 2.0). Adobe also makes the PDF 1.7 specification publicly available at no cost:

- Adobe PDF 1.7 reference: https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf
- ISO 32000-2:2020 can be purchased from ISO. A committee draft is sometimes available through national standards bodies.

Key sections in the spec:
- Section 7: Syntax (objects, streams, xref, file structure)
- Section 8: Graphics (graphics state, coordinate spaces, CTM)
- Section 9: Text (fonts, encodings, text operators)
- Section 10: Rendering
- Section 12: Interactive features (annotations)
