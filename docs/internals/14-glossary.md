# Glossary

This document defines every term used across the internals documentation. Keep it open as a reference tab while reading the other docs. Terms within a definition that are themselves defined here are written in **bold** on their first appearance in that entry.

---

## PDF Specification Terms

### AABB (Axis-Aligned Bounding Box)
A rectangle whose sides are parallel to the x and y axes, used to approximate the extent of an arbitrarily oriented shape. This engine uses AABB intersection for glyph-to-target overlap tests; rotated quads with narrow slivers can produce false negatives. See `12-security-model.md` for the known gap.

### BT / ET (Begin Text / End Text)
Operator pair that brackets a text object in a **content stream**. `BT` resets the **text matrix** and **line matrix** to the identity matrix but does not reset font, size, or spacing parameters. `ET` is a no-op in this engine. See `06-text-system.md` for why `BT` must not reset the full text state.

### CID (Character Identifier)
A numeric index into a **composite font**'s character set. With `Identity-H` encoding, the CID is the two-byte big-endian integer read directly from the string operand. CIDs are mapped to Unicode via the **ToUnicode** CMap.

### CMap
A PostScript-derived mapping structure that translates character codes to Unicode scalar values or glyph names. In PDF, CMaps appear as stream objects and are referenced from a font's `ToUnicode` entry. See `02-pdf-primer.md §11`.

### cm operator
A content stream operator that concatenates a new affine matrix onto the **CTM**. Takes six numbers `a b c d e f`. The new matrix is **pre-multiplied**: `CTM' = M_new * CTM_old`. See `05-graphics-state.md §4` for why the multiplication order is critical.

### Composite font (Type0)
A font that uses variable-width glyph codes (**CIDs**) instead of single-byte codes. Type0 fonts carry a `DescendantFonts` array and a `ToUnicode` CMap. This engine supports Type0 only with `Identity-H` encoding.

### Content stream
A **stream** object (or array of stream objects) attached to a page via its `/Contents` entry. It contains a sequence of **operators** and **operands** in postfix notation that instruct a renderer how to draw the page. See `02-pdf-primer.md §8`.

### CropBox
A rectangle (in **user space** points) defining the visible region of the page shown to users. Defaults to **MediaBox** when absent. This engine uses `CropBox` as the visible page boundary and removes the crop offset in **normalized page space**. See `02-pdf-primer.md §7`.

### CTM (Current Transformation Matrix)
The accumulated affine matrix that maps **user space** coordinates to **device space** (or, in this engine, to **normalized page space**). Updated by the `cm` operator; saved and restored by `q`/`Q`. Represented as **`Matrix`** in the codebase. See `05-graphics-state.md §1`.

### Device space
The final output coordinate system — pixels on a screen or dots on a printer. In this engine, content is not rendered to device pixels; the engine's equivalent terminal space is **normalized page space**.

### Embedded file
A file attached to a PDF document via the `Names/EmbeddedFiles` name tree. Embedded files exist outside the **content stream** and are invisible to content-stream redaction. The engine always removes them when `strip_attachments` is enabled.

### Encoding (font)
The mapping from byte values in a string operand to glyph codes. Simple fonts use a single-byte encoding (commonly standard Latin or WinAnsiEncoding). Composite fonts use multi-byte encodings; this engine supports `Identity-H` only.

### ExtGState
A named graphics state parameter dictionary stored in the page's `/Resources/ExtGState` subdictionary. Applied with the `gs` operator. Can carry a `Font` array that sets the current font and size; the engine pre-loads these fonts at page initialization. See `06-text-system.md §1`.

### Filter
A compression or encoding algorithm applied to a **stream**'s raw bytes. Common filters: `FlateDecode` (zlib/deflate), `ASCII85Decode`, `LZWDecode`. Multiple filters can be chained. The engine supports `FlateDecode` and rejects unknown filters with an explicit error.

### Font descriptor
A dictionary associated with a font object that describes global font metrics and properties: `FontBBox`, `Ascent`, `Descent`, `CapHeight`, `Flags`, and a reference to the embedded font program. This engine does not parse font descriptor metrics; glyph height is estimated by heuristic. See `06-text-system.md §3`.

### Form XObject
A self-contained reusable content unit stored as a **stream** with its own operators, resources, and coordinate space. Invoked with the `Do` operator. This engine does not support redaction inside form XObjects; a `Do` referencing one on a targeted page returns a hard error. See `09-redaction-pipeline.md §6`.

### Generation number
A 16-bit integer paired with an **object number** to form an **object reference**. Increments when an object is deleted and reused; almost always 0 in modern PDFs.

### Glyph
The visual representation of a character. In this engine, each glyph has a decoded Unicode character, a bounding quad in **normalized page space**, an advance width, and a visibility flag (`Tr=3` means invisible). Represented as **`Glyph`** in `crates/pdf_text`.

### Glyph space
The coordinate system internal to a font, typically in 1/1000 em units. A glyph's local rectangle is expressed in glyph space and is transformed to **text space** by the font size scaling, then to **user space** by the **text matrix**, then to **normalized page space** by the **CTM** and **page transform**.

### Image XObject
A stream containing raster pixel data. Placed on the page by setting the **CTM** before calling `Do`; the image maps to a 1×1 unit square in user space. The engine removes image XObjects whose page-space footprint intersects a redaction target. See `09-redaction-pipeline.md §6`.

### Incremental update
A revision appended to an existing PDF file without rewriting earlier content. A new xref table and trailer are appended; the trailer's `/Prev` key chains back to the previous xref. This engine reads the full `Prev` chain and flattens it into a single object map; the output is always a single-revision file. See `02-pdf-primer.md §5`.

### Indirect object
A PDF object given a permanent identity through an object number and generation number, allowing other objects to reference it. Written as `N G obj ... endobj`; referenced as `N G R`. See `02-pdf-primer.md §2`.

### MediaBox
A required rectangle (in **user space** points) defining the physical page boundary. A page without a MediaBox is malformed. Standard US Letter: `[0 0 612 792]`. See `02-pdf-primer.md §7`.

### Object number
A positive integer that uniquely identifies an **indirect object** within a file. Together with the **generation number** it forms an **object reference**.

### Object reference
A pair `(object_number, generation)` used to look up an **indirect object** in the **xref table**. Written as `N G R` in PDF syntax. Represented as **`ObjectRef`** in the codebase.

### Object stream
A stream that contains multiple compressed **indirect objects** packed together (PDF 1.5+). This engine does not support object streams; files that use them fail with an explicit error.

### Operand
A PDF value (number, name, string, array, or dictionary) that precedes an **operator** in a **content stream**. PDF uses postfix notation: operands come before the operator they apply to.

### Operator
An ASCII keyword in a **content stream** that performs a drawing, state-change, or text-painting action. Examples: `cm`, `Tf`, `Tj`, `S`, `Do`. See `02-pdf-primer.md §8`.

### Page tree
A balanced tree of Pages nodes and Page leaf nodes rooted at the document catalog's `/Pages` entry. Inheritable properties (`MediaBox`, `Resources`, `Rotate`, `CropBox`) flow from ancestors to descendants. The engine flattens this tree into a `Vec<PageInfo>` during document parsing.

### Postfix notation
The operator ordering used by PDF content streams: operands are listed before the operator they apply to. Also called reverse Polish notation.

### q / Q operators
Operators that push and pop the **graphics state** stack. `q` saves a copy; `Q` restores the previous copy. In the text extraction path the engine saves both the **CTM** and the full **text state** on each `q`. See `05-graphics-state.md §5` and `06-text-system.md`.

### Resources dictionary
A dictionary on a page (or inherited from a page tree ancestor) that maps resource names to their objects: fonts (`/Font`), XObjects (`/XObject`), graphics state parameters (`/ExtGState`), and others. Content stream operators refer to resources by name (e.g. `/F1 Tf`).

### Rotate
An integer page entry (0, 90, 180, or 270) specifying the clockwise rotation to apply when displaying the page. The engine applies this rotation inside **`PageBox::normalized_transform()`** so that all output coordinates are in a consistent frame. See `05-graphics-state.md §6`.

### Simple font (Type1, TrueType)
A font that uses one byte per glyph code. Width data comes from the `Widths` array indexed by `code - FirstChar`. This engine decodes simple font text as ASCII; non-ASCII codes without a `ToUnicode` map produce replacement characters.

### Stream
A PDF object consisting of a dictionary followed by a sequence of bytes (possibly compressed). Streams are used for page content, images, fonts, and the xref table itself. Represented as **`PdfStream`** in the codebase. See `02-pdf-primer.md §3`.

### Text matrix (Tm)
An affine matrix that defines the current text position and orientation within **user space**. Set by the `Tm` operator; advanced horizontally by each glyph's advance width. Distinct from the **line matrix**, which tracks the start of the current line. See `06-text-system.md §2`.

### Text rendering mode (Tr)
An integer graphics state parameter controlling how glyphs are painted: 0 = fill (normal), 1 = stroke, 2 = fill+stroke, 3 = invisible (no pixels drawn). Mode 3 is used for OCR text layers. The engine includes invisible glyphs in redaction but excludes them from search results. See `06-text-system.md §4`.

### Text rise (Ts)
A vertical offset added to the baseline before placing a glyph. Used for superscripts and subscripts. Stored in **`RuntimeTextState`** as `text_rise`.

### Text space
The local coordinate system for glyph placement, defined by the **text matrix** relative to **user space**. A glyph's local rectangle is expressed in text space and transformed to user space by the text matrix.

### TJ operator
A text-showing operator that accepts an array of strings and numeric kern adjustments. Negative numbers shift the text position to the right (reducing spacing); positive numbers add space. The kern compensation logic in redact/erase mode produces TJ arrays from Tj strings. See `09-redaction-pipeline.md §4`.

### Tj operator
A text-showing operator that accepts a single byte string. Each byte is one glyph code for simple fonts; two-byte pairs are CIDs for composite fonts.

### ToUnicode CMap
A CMap stream in a font object that maps glyph codes to Unicode scalar values. Without it, non-ASCII text in composite fonts cannot be decoded. The engine parses `ToUnicode` into a `BTreeMap<u16, char>`. See `02-pdf-primer.md §11`.

### Trailer dictionary
A dictionary at the end of each PDF revision that provides the root entries: `/Root` (document catalog reference), `/Size` (total object count), and optionally `/Prev` (byte offset of previous xref) and `/Encrypt`. The engine reads the trailer to bootstrap document parsing. See `02-pdf-primer.md §1`.

### User space
The coordinate system in which a page's content stream is written. Origin is conventionally at the bottom-left; y increases upward. All drawing operators express coordinates in user space. Units are points (1 point = 1/72 inch). See `05-graphics-state.md §1`.

### Width units
Glyph advance width expressed in 1/1000 em (text units). Looked up from the font's `Widths` array (simple fonts) or `DW`/`W` entries (composite fonts). Used to advance the **text matrix** after each glyph and to compute kern compensation during redaction.

### XObject
A self-contained reusable content unit referenced by name and invoked with the `Do` operator. Two subtypes are relevant: **Image XObject** (raster pixels) and **Form XObject** (arbitrary operators). See `02-pdf-primer.md §13`.

### Xref stream
A compressed **stream** object that replaces the plain-text **xref table** in PDF 1.5+. This engine does not support xref streams; files that use them fail with an explicit parse error.

### Xref table (cross-reference table)
A fixed-width table near the end of a PDF revision that maps **object numbers** to their byte offsets in the file. Allows O(1) random access to any object. Each entry is 20 bytes: `OOOOOOOOOO GGGGG N\r\n`. See `02-pdf-primer.md §4`.

---

## Engine Concepts

### 80%/12% heuristic
The approximation used for glyph bounding box height when no font metric data is parsed: the box spans 80% of the font size, starting 12% below the baseline. Keeps bounding boxes clear of adjacent lines for normal body text without requiring font file parsing. See `06-text-system.md §7`.

### Apply pipeline
The sequence of operations performed by `apply_redactions` to produce a redacted PDF from a parsed document and a **normalized redaction plan**. Phases: global operations (metadata, attachments), then per-page processing (glyph removal, vector neutralization, image neutralization, annotation removal, overlay generation). See `09-redaction-pipeline.md`.

### Byte offset (vs. char offset)
The engine stores `char_start`/`char_end` in the search index and `TextItem` as byte offsets into the UTF-8 concatenated page text, not character counts. This is necessary because Rust string indexing is byte-based. Using character offsets caused a real bug (commit `fc85fcf`) where matches on non-ASCII text pointed to the wrong glyphs.

### Conservative annotation removal
The policy of removing annotations that lack a `/Rect` entry (except Link annotations) regardless of whether they intersect a redaction target. Applied because annotations without geometry may still carry sensitive metadata. See `09-redaction-pipeline.md §9` and `12-security-model.md §4`.

### Cycle detection
A guard applied in page tree traversal, `Prev` chain following, and reachable-reference collection to prevent infinite loops caused by malformed or adversarial PDFs with circular object references.

### Deferred removal
The technique of collecting object references to be deleted (old content streams, neutralized image XObjects) during per-page processing and executing the actual removal in a single post-loop pass. Required because PDF objects are shared by reference; removing a shared XObject during one page's processing would corrupt references from other pages. See `09-redaction-pipeline.md §7`.

### Erase mode
A **redaction mode** in which targeted glyphs are removed with kern compensation so surrounding text does not shift, but no colored overlay is painted. The visual result is blank space where the text was. Compare **Redact mode** and **Strip mode**.

### Fail-explicit philosophy
The engine's rule that unsupported PDF features must return `PdfError::Unsupported` or `PdfError::UnsupportedOption` rather than silently degrading. Silent degradation in a redaction context is a security defect: unredacted content may pass through without any error signal. See `12-security-model.md §5`.

### Final CTM simulation
The process of simulating the **CTM** through the entire rewritten content stream to determine what transformation was left active at the end. Required to correctly position the **overlay stream**, which must draw in page space regardless of what transformation the content stream ended in. See `09-redaction-pipeline.md §8`.

### Glyph advance
The horizontal distance by which the **text matrix** advances after a glyph is placed. Computed as `(width_units / 1000) * font_size + character_spacing [+ word_spacing if space] * (horizontal_scaling / 100)`. See `06-text-system.md §3`.

### Glyph quad
A four-point polygon in **normalized page space** representing the bounding parallelogram of a single glyph. Produced by transforming the glyph's local rectangle through the full transform chain. Stored in **`Glyph`** and used for intersection testing and overlay generation.

### Graphics state stack
A LIFO stack of **graphics state** snapshots managed by `q` and `Q` operators. In `pdf_text`, both the **CTM** and the **text state** are pushed; in `pdf_redact`, only the CTM is pushed. Implemented as `Vec` to guarantee LIFO order.

### Kern compensation
The technique of converting a `Tj` string to a `TJ` array when glyphs are removed, inserting negative kern adjustments equal to the removed glyphs' advance widths. Preserves horizontal text positioning so that surrounding text does not shift after removal. See `09-redaction-pipeline.md §4`.

### Normalized page space
The engine's canonical coordinate space for all glyph quads, match rectangles, and redaction targets. Origin is at the corner of the **CropBox**; page rotation is already applied; all coordinates are non-negative. Produced by applying the **page transform** to user-space coordinates. See `05-graphics-state.md §1`.

### Normalized redaction plan
The validated, page-space representation of a **redaction plan** after `normalize_plan` has processed it. All targets are expressed as **`NormalizedPageTarget`** structs with quads in **normalized page space** and pre-computed bounding rectangles. Represented as **`NormalizedRedactionPlan`**.

### Operator whitelist
A fixed set of PDF content stream operators that the engine recognizes on pages undergoing redaction. Any operator not in the whitelist causes a hard error rather than silent pass-through. This prevents unknown operators from carrying redactable content that escapes the pipeline undetected. See `09-redaction-pipeline.md §3` and `12-security-model.md §4`.

### Overlay stream
A content stream appended after a page's rewritten content stream in **Redact mode**. Contains `re`/`f` commands that paint filled colored rectangles over each target. The stream first inverts the **final CTM** and the **page transform** so that its coordinates are interpreted in page space regardless of what state the prior stream left. See `09-redaction-pipeline.md §8`.

### Page space
Shorthand for **normalized page space** when used in prose. Coordinates in this space represent positions in the visible area of the page, with the origin at the lower-left corner of the CropBox and rotation already applied.

### Page transform
The matrix produced by **`PageBox::normalized_transform()`** that converts from PDF **user space** to **normalized page space**. Accounts for the CropBox offset and the `/Rotate` value. All geometry passing through the text extraction and redaction pipelines is ultimately expressed in this space. See `05-graphics-state.md §6`.

### Pre-multiplication
The matrix multiplication order required for the `cm` operator: `CTM' = M_new * CTM_old`. In code: `ctm = matrix.multiply(ctm)`. The opposite order (post-multiplication) produces incorrect coordinates for nested `cm` sequences. See `05-graphics-state.md §4`.

### Redact mode
The default **redaction mode**. Targeted glyphs are removed with **kern compensation** so surrounding text does not shift, and a colored **overlay stream** is appended to paint a filled rectangle over each target. The visual result is a solid colored bar with no text visible or selectable beneath it.

### Redaction mode
One of three choices controlling how text glyphs are removed: **Strip**, **Redact** (default), or **Erase**. The mode is set in the **redaction plan** and stored in the **normalized redaction plan**. See `crates/pdf_targets/src/targets.rs`.

### Redaction plan
The caller-supplied JSON-serializable input that specifies targets, mode, fill color, and optional global operations (strip metadata, strip attachments). Validated and converted to a **normalized redaction plan** by `normalize_plan`. Represented as **`RedactionPlan`**.

### Row-vector convention
The mathematical convention chosen to match the PDF specification directly: points are represented as row vectors `[x, y, 1]` and transformed by right-multiplying by the matrix. Applied consistently in **`Matrix::transform_point`** and all matrix composition in the engine. See `05-graphics-state.md §3`.

### Search index
A per-page data structure built from **`ExtractedPageText`** that supports case-insensitive, diacritic-normalizing substring search. It maintains a mapping from positions in the normalized query string back to glyph indices so that a match can be converted to a set of page-space quads. See `07-search-geometry.md`.

### Strip mode
A **redaction mode** in which targeted glyph bytes are physically removed from the content stream with no kern compensation and no overlay. Surrounding text shifts left to fill the gap. Use only when layout preservation is not required.

### Synthetic font key
The font map key format `__gs:NAME` used for fonts loaded via **ExtGState** entries, where `NAME` is the ExtGState resource name. The `__gs:` prefix prevents collision with normal font resource names. See `06-text-system.md §1`.

### Text state
The collection of text-rendering parameters that persist across text objects: font, font size, character spacing, word spacing, horizontal scaling, leading, text rise, and text rendering mode. Part of the graphics state; saved and restored by `q`/`Q`. Represented as **`RuntimeTextState`** in `pdf_text`. See `06-text-system.md §2`.

### Vector neutralization
The process of replacing path-painting operators (`S`, `f`, `B`, etc.) with `n` (no-paint) when the path's bounding box intersects a redaction target. The engine simulates the CTM through the content stream to compute path positions in page space. See `09-redaction-pipeline.md §5`.

### Visual reading order
The sort order applied to extracted glyphs so that they form a left-to-right, top-to-bottom sequence consistent with how a human would read the page. Used to assemble the concatenated page text string that the search index operates on. See `07-search-geometry.md`.

---

## Rust Types

### `ApplyReport`
A summary of the operations performed by one call to `apply_redactions`. Fields: `pages_touched`, `text_glyphs_removed`, `path_paints_removed`, `image_draws_removed`, `annotations_removed`, `warnings`. Defined in `crates/pdf_redact/src/redact.rs`.

### `Color`
An RGB triple (`r: u8, g: u8, b: u8`) used for the fill color of the **overlay stream**. `Color::BLACK` is the default redaction color. Defined in `crates/pdf_graphics/src/geometry.rs`.

### `DocumentCatalog`
A pair of `ObjectRef` values — the catalog object and the pages tree root — extracted during document parsing. Stored inside **`ParsedDocument`**. Defined in `crates/pdf_objects/src/document.rs`.

### `ExtractedPageText`
The output of one page's text extraction pass. Contains the concatenated Unicode page text, a `Vec<TextItem>`, and a `Vec<Glyph>`. Consumed by the search subsystem and by `collect_glyph_removals` in the redaction pipeline. Defined in `crates/pdf_text/src/text.rs`.

### `Glyph`
A single decoded glyph with its Unicode character, bounding `Rect`, bounding `Quad` in normalized page space, `page_char_index`, `operation_index`, `GlyphLocation`, visibility flag (`visible`), and `width_units`. Defined in `crates/pdf_text/src/text.rs`.

### `GlyphLocation`
An enum describing where within the content stream a glyph's bytes are located: `Direct` (inside a `Tj` string operand) or `Array` (inside a `TJ` array element). Used by the redaction path to find and remove specific bytes. Defined in `crates/pdf_text/src/text.rs`.

### `GraphicsState`
A minimal graphics state snapshot used by the content stream interpreter in `pdf_content`. Tracks the **CTM** and stroke width. Distinct from `RuntimeTextState`, which tracks the full text state. Defined in `crates/pdf_content/src/content.rs`.

### `Matrix`
A 3×3 affine matrix in six-element row-major form `[a, b, c, d, e, f]`. Used for every coordinate transform in the engine: CTM, text matrix, page transform. Key methods: `multiply`, `transform_point`, `inverse`, `identity`, `translate`, `scale`, `rotate_degrees`. Defined in `crates/pdf_graphics/src/geometry.rs`. See `05-graphics-state.md §2`.

### `NormalizedPageTarget`
A validated, page-space redaction target for a single page. Contains `page_index`, a `Vec<Quad>` of page-space quads, and a pre-computed bounding `Rect`. Used by all intersection tests in the apply pipeline. Defined in `crates/pdf_targets/src/targets.rs`.

### `NormalizedRedactionPlan`
The validated, engine-ready form of a **`RedactionPlan`**. Contains a `Vec<NormalizedPageTarget>`, resolved **`RedactionMode`**, `Color`, and boolean flags for annotation removal, metadata stripping, and attachment stripping. Defined in `crates/pdf_targets/src/targets.rs`.

### `ObjectRef`
A pair `(object_number: u32, generation: u16)` that uniquely identifies an **indirect object**. Used as the key type in `PdfFile::objects`. Defined in `crates/pdf_objects/src/types.rs`.

### `Operation`
A single parsed content stream instruction: an `operator: String` and a `Vec<PdfValue>` of operands. The engine operates on `Vec<Operation>` slices throughout the redaction pipeline. Defined in `crates/pdf_content/src/content.rs`.

### `PageBox`
The geometric description of one page: `media_box: Rect`, `crop_box: Rect`, and `rotate: i32`. Provides `normalized_transform()` to produce the **page transform** matrix and `size()` to compute the visible page dimensions. Defined in `crates/pdf_graphics/src/geometry.rs`.

### `PageInfo`
A flattened, inheritance-resolved description of one page: `page_ref`, `resources`, `page_box`, `content_refs`, and `annotation_refs`. Built by `collect_pages` during document parsing. Defined in `crates/pdf_objects/src/document.rs`.

### `PageSearchIndex`
An internal struct built per page by the search subsystem. Stores normalized text, a mapping from normalized positions to display positions, and a mapping from display positions to glyph indices. Not part of the public API. Defined in `crates/pdf_text/src/text.rs`.

### `ParsedDocument`
The top-level result of parsing a PDF file: a `PdfFile`, a `DocumentCatalog`, and a `Vec<PageInfo>`. The engine's entry-point types accept or return this. Defined in `crates/pdf_objects/src/document.rs`.

### `ParsedPageContent`
The result of decoding and tokenizing one page's content stream(s): raw bytes and a `Vec<Operation>`. Produced by `parse_page_contents`. Defined in `crates/pdf_content/src/content.rs`.

### `PaintOperator`
An enum of path-painting operators: `Stroke`, `Fill`, `FillEvenOdd`, `StrokeFill`, `StrokeFillEvenOdd`, `CloseStroke`, `CloseFill`, `CloseFillEvenOdd`, `NoPaint`. Used by vector neutralization to identify paint commands and replace them with `n`. Defined in `crates/pdf_content/src/content.rs`.

### `PathSegment`
An enum of path construction commands: `MoveTo`, `LineTo`, `CurveTo`, `Rect`, `ClosePath`. Accumulated during vector neutralization to compute a path's bounding box. Defined in `crates/pdf_content/src/content.rs`.

### `PdfDictionary`
A type alias for `BTreeMap<String, PdfValue>`. Keys are PDF names without the leading `/`. Deterministic key ordering (from `BTreeMap`) is important for reproducible serialization. Defined in `crates/pdf_objects/src/types.rs`.

### `PdfError`
The engine's unified error enum. Variants: `Parse` (malformed input), `Corrupt` (structurally invalid PDF), `Unsupported` (valid PDF but out of scope), `UnsupportedOption` (unsupported plan option), `InvalidPageIndex`, `MissingObject`. Defined in `crates/pdf_objects/src/error.rs`.

### `PdfFile`
The raw, flattened in-memory representation of a parsed PDF. Contains `version: String`, `objects: BTreeMap<ObjectRef, PdfObject>`, `trailer: PdfDictionary`, and `max_object_number: u32`. All incremental update chains have been resolved; the newest definition of each object wins. Defined in `crates/pdf_objects/src/types.rs`.

### `PdfObject`
An enum with two variants: `Value(PdfValue)` for ordinary objects and `Stream(PdfStream)` for stream objects. The value stored in `PdfFile::objects` for each `ObjectRef`. Defined in `crates/pdf_objects/src/types.rs`.

### `PdfStream`
A decoded stream: `dict: PdfDictionary` (the stream dictionary) and `data: Vec<u8>` (the decoded bytes after filter application). Defined in `crates/pdf_objects/src/types.rs`.

### `PdfString`
A wrapper around `Vec<u8>` representing a PDF literal or hex string. Byte-oriented because PDF strings are not required to be UTF-8. Provides `to_lossy_string()` for display purposes. Defined in `crates/pdf_objects/src/types.rs`.

### `PdfValue`
The enum of all non-stream PDF value types: `Null`, `Bool`, `Integer`, `Number`, `Name`, `String`, `Array`, `Dictionary`, `Reference`. Used everywhere the engine works with PDF objects and content stream operands. Defined in `crates/pdf_objects/src/types.rs`.

### `Point`
A 2D coordinate `(x: f64, y: f64)` in whatever space the context implies (user space, page space, etc.). Used as the element type of `Quad` and the argument type of `Matrix::transform_point`. Defined in `crates/pdf_graphics/src/geometry.rs`.

### `Quad`
A four-point polygon `[Point; 4]` representing the parallelogram bounding box of a glyph or redaction target in normalized page space. Intersection tests use the AABB of the quad's four corners. Defined in `crates/pdf_graphics/src/geometry.rs`.

### `Rect`
An axis-aligned rectangle `(x: f64, y: f64, width: f64, height: f64)`. The `x`,`y` fields are the bottom-left corner in whatever coordinate space the context implies. Provides `normalize()` (flip negative dimensions), `intersects()`, `union()`, and `to_quad()`. Defined in `crates/pdf_graphics/src/geometry.rs`.

### `RedactionMode`
An enum with three variants: `Strip`, `Redact` (default), `Erase`. Controls how text glyphs are removed and whether an overlay is drawn. Defined in `crates/pdf_targets/src/targets.rs`.

### `RedactionPlan`
The caller-supplied redaction request. Carries a `Vec<RedactionTarget>`, optional `RedactionMode`, optional `FillColor`, and optional boolean flags. Validated by `normalize_plan` into a **`NormalizedRedactionPlan`**. Defined in `crates/pdf_targets/src/targets.rs`.

### `RedactionTarget`
An enum of caller-supplied target shapes: `Rect` (axis-aligned rectangle), `Quad` (arbitrary four-point polygon), `QuadGroup` (multiple quads on one page). All coordinates are in **normalized page space**. Defined in `crates/pdf_targets/src/targets.rs`.

### `RuntimeTextState`
The full text state tracked during content stream interpretation in `pdf_text`. Extends the minimal `TextState` with `font: Option<String>` and `text_render_mode: i64`. Saved and restored on each `q`/`Q`. See `06-text-system.md §2`.

### `TextItem`
The output of a single text-showing operation. Fields: `text` (coalesced Unicode string), `bbox` (bounding rect in page space), `quad`, `char_start`, and `char_end` (byte offsets into the page-level text string). The search system maps match byte offsets back to `TextItem` ranges to retrieve quads. Defined in `crates/pdf_text/src/text.rs`.

### `TextMatch`
A search result. Fields: `text` (matched substring), `page_index`, and `quads` (a `Vec<Quad>` covering the matched glyphs in page space). Returned by `search_page_text`. Defined in `crates/pdf_text/src/text.rs`.

### `XrefEntry`
A parsed entry from the **xref table**: `offset: usize`, `generation: u16`, `in_use: bool`. Defined in `crates/pdf_objects/src/types.rs`.
