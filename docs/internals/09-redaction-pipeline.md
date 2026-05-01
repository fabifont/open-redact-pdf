# Redaction Application Pipeline

## 1. Entry point

`apply_redactions(file, pages, plan)` in `crates/pdf_redact/src/redact.rs`.

Accepts the parsed PDF file, the extracted page representations, and a `NormalizedRedactionPlan`. Returns a modified in-memory PDF ready for serialization by `pdf_writer`.

## 2. Phase 1: Global operations

Global operations run once before any per-page work.

### `reject_hidden_optional_content` / `sanitize_hidden_optional_content`

Before anything else, the catalog's `/OCProperties` is inspected. If it declares Optional Content Groups that are off in the default configuration — either via a non-empty `/OFF` array or via `/BaseState /OFF` / `/Unchanged` — the engine's default behaviour is to refuse the document with `PdfError::Unsupported`. Hidden-layer content that the user never saw cannot be targeted through the visible glyph list, so silently redacting only the visible portion would be a correctness hole.

Setting `sanitize_hidden_ocgs: true` on the plan replaces the rejection with an in-place sanitization pass:

1. Collect the set of hidden OCG object refs from the catalog (`/OFF`, or `/BaseState /OFF` minus `/ON`).
2. For each page, resolve `/Resources /Properties` to the set of names that map to hidden OCGs.
3. Walk each page content stream, tracking marked-content nesting, and strip `BDC /OC /<name> ... EMC` blocks whose name is in the hidden set.
4. Rewrite the catalog's `/OCProperties /D` to clear `/OFF` and set `/BaseState /ON`, so the saved output no longer advertises the hidden state.

Form XObject content is not rewritten yet — a warning is emitted on any sanitized page that also has XObjects so callers can audit.

### `strip_metadata`

Removes the `Info` dictionary from the trailer and the `Metadata` stream from the document catalog. Both can contain author names, creation timestamps, software identifiers, and other identifying information.

### `strip_attachments`

Walks the `Names/EmbeddedFiles` name tree via depth-first search with cycle detection (to handle malformed PDFs with circular references). Removes all reachable embedded file objects. Attached files can contain the original unredacted source document or other sensitive material.

## 3. Phase 2: Per-page processing

For each page that has at least one redaction target, the engine runs the following steps in order.

### Step 1: `analyze_page_text`

Extracts all glyphs from the page with their page-space positions and dimensions. This is the same extraction path used by the search subsystem, ensuring that what the engine sees during redaction matches what the user saw when they identified the match.

### Step 2: `parse_page_contents`

Parses the page content stream into a structured list of PDF operators and their operands.

### Step 3: `ensure_supported_operators`

Checks every operator in the content stream against a whitelist of operators the engine understands. Any unknown operator causes an explicit error rather than silent pass-through. This prevents an attacker or a malformed PDF from smuggling content through an unrecognized operator.

### Step 4: `load_xobjects`

Identifies all XObjects referenced by `Do` operators in the content stream and classifies each as an Image XObject or a Form XObject.

### Step 5: `collect_glyph_removals`

Intersects each glyph's bounding quad against each target's bounding box. Glyphs that intersect are recorded for removal. The result is a set of glyph indices that should not appear in the output stream.

### Step 6: `rewrite_text_operations`

Walks the content stream operators. For text-painting operators (`Tj`, `TJ`, `'`, `"`), removes or compensates each glyph that appears in the removal set. The exact behavior depends on the redaction mode (see kern compensation below).

### Step 7: `neutralize_vector_operations`

Scans path construction and painting operators. If the accumulated path's bounding box intersects any target, the painting operator is replaced with `n` (no-paint), which discards the path without drawing it.

### Step 8: `neutralize_image_operations`

For each `Do` operator referencing an Image XObject, transforms the unit square `[0,0,1,1]` by the current CTM to find the image's page-space footprint. The pass distinguishes three cases:

1. **No overlap** — leave the `Do` intact.
2. **Full cover** — the union of intersecting target AABBs (mapped back to image-unit-square space) contains the entire `[0,1] × [0,1]`. The `Do` is replaced with `n` and the XObject is marked for deferred removal (current behaviour, unchanged).
3. **Partial overlap** — the targeted region maps to a strict sub-rectangle of the image. The pass records a `PendingImageMask` with the original `ObjectRef`, the image-space pixel rectangle, and the redaction's `fill_color`.

After the per-page neutralization completes, each pending mask is applied via `apply_partial_mask`:

1. Clone the original image stream.
2. Pass through `image_mask::mask_image_region` which detects the format (raw / `FlateDecode` / `DCTDecode`), decodes pixels (via `decode_stream` or `jpeg_decoder`), paints the rectangular pixel region with the plan's `fill_color`, and re-encodes (Flate-compressed for raw / Flate input, DCT-encoded at quality 85 for JPEG input).
3. Allocate a fresh `ObjectRef` for the masked stream; insert it into `file.objects`.
4. Repoint the page's `Resources.XObject[name]` at the new ref via copy-on-write of any indirect XObject dictionary, so other pages that share the same image stream are unaffected.

Unsupported image formats (`Indexed`, `ICCBased`, `JBIG2Decode`, `JPXDecode`, `CCITTFaxDecode`, non-8-bpc) and any decode error make `mask_image_region` return `PdfError::Unsupported`; in that case `apply_partial_mask` falls back to whole-invocation neutralization (the `Do` is rewritten to `n` and the original stream is queued for removal). The `ApplyReport.image_draws_masked` and `image_draws_removed` counters distinguish the two outcomes.

Inside Form XObject content streams the same neutralization runs but partial masks always fall back to drop, since per-Form COW would require a deeper rewrite of nested Resources.

### Step 9: `remove_annotations`

If `remove_annotations` is enabled in the plan (default: true), parses each annotation's `Rect`, transforms it to page space, and checks for intersection with any target. FileAttachment annotations are always removed regardless of intersection, because attached files bypass the content-stream redaction entirely. Annotations without a `Rect` entry are conservatively removed unless they are Link annotations.

### Step 10: `serialize_operations`

Converts the modified operator list back to content-stream bytes.

### Step 11: Overlay stream (Redact mode only)

If the mode is `Redact`, `overlay_stream_bytes` generates a separate content stream that paints filled colored quads over each target. The overlay is appended as an additional content stream after the rewritten page content.

### Step 12: Write new content stream

The new content bytes replace the page's content stream. Old content stream object references are queued for deferred removal.

## 4. Kern compensation (the heart of Redact/Erase modes)

When a glyph is removed in Redact or Erase mode, the surrounding text must not shift. PDF text positioning depends on each glyph's advance width; removing a glyph shortens the line.

`build_compensated_array(string, removed_indices, glyph_starts)` converts a `Tj` string into a `TJ` array:

1. Iterates through each character in the string.
2. If the character is in the removal set, accumulates its advance width in `kern_accum` (in text-space units).
3. If the character is not removed, and `kern_accum > 0`, emits a negative kern entry `PdfValue::Number(-kern_accum)` before emitting the character's byte. In the TJ operator, a negative number moves the text position to the right by that many thousandths of a text-space unit, compensating for the missing glyph width.
4. Emits the kept character's byte as a string fragment.
5. Changes the operator from `Tj` to `TJ`.

The `'` and `"` operators (move-to-next-line variants of `Tj`) produce side effects on the text position that are not safely reproducible through kern compensation alone. These fall back to Strip mode with a warning logged.

## 5. Vector neutralization

The engine simulates the current transformation matrix (CTM) through the content stream:

- `q` — pushes a copy of the CTM stack.
- `Q` — pops the CTM stack.
- `cm` — concatenates a matrix onto the current CTM.

Path construction operators (`m`, `l`, `c`, `h`, `re`) accumulate path segments into a working path bounding box. The `v` and `y` bezier curve operators are whitelisted (accepted without error) but their control points are not accumulated into the bounding box — a known gap that could cause a slightly undersized bounds estimate for paths that use them.

On any paint operator (`S`, `s`, `f`, `F`, `f*`, `B`, `B*`, `b`, `b*`), the engine inflates the path bounding box by the current stroke width and tests it against all targets. If any target intersects, the paint operator is replaced with `n`. The path is discarded; the surrounding non-intersecting paths are unaffected.

## 6. Image neutralization

For a `Do` operator referencing an Image XObject, the engine:

1. Takes the unit square corners `(0,0)`, `(1,0)`, `(1,1)`, `(0,1)`.
2. Applies the current CTM to transform them to page space (images are placed by setting the CTM before calling `Do`).
3. Computes the axis-aligned bounding box of the transformed corners.
4. Tests against all targets.
5. If any intersection is found: replaces the `Do` with `n`, adds the XObject reference to the deferred-removal set.

Form XObjects are handled intersection-aware. Each Form carries a `BBox` and an optional `Matrix`. At neutralization time the Form's rectangle is transformed through `Matrix × current CTM × page transform` and compared against the redaction targets.

When the resulting quad does not touch any target, the Form is left untouched and the page redacts normally. When it does touch a target, the engine allocates a per-page copy of the Form (a new `ObjectRef` with a cloned stream dictionary), rewrites the copy's content stream to strip the targeted glyph bytes — using the glyphs that were already tagged with that Form's ref during extraction — and re-emits the bytes with FlateDecode compression. The page's `Resources.XObject` entry is then rewritten on the page dictionary to point at the per-page copy, so other pages that still use the original Form are unaffected.

If the Form's content itself invokes another Form whose bounding quad also intersects a target, the rewrite recurses: the inner Form is copied too, its content is rewritten, and the outer Form's own `Resources.XObject` is repointed at the inner copy. Recursion is capped at depth 8 with a warning. After the text rewrite, the same `neutralize_vector_operations` and `neutralize_image_operations` passes used on the page are invoked on the Form's operations — with the Form's invocation CTM threaded in as the base — so vector paint and `Do` of Image XObjects that fall under a target inside the Form are neutralized alongside the text.

Text extraction and search still recurse into Form XObjects — see `06-text-system.md §1`. The pipeline above is the redaction side of the story.

## 7. Deferred cleanup

Old content streams and neutralized Image XObjects are not removed during per-page processing. They are removed in a single pass after all pages have been processed.

This is necessary because PDF objects are shared by reference. A logo image used on every page of a document is stored as one XObject referenced from every page. If that XObject is removed when the first page is processed, all subsequent pages will reference a missing object and the output PDF will be corrupt.

Deferring to a post-loop cleanup also makes the page loop idempotent: each page sees a consistent object graph.

## 8. Overlay generation

`overlay_stream_bytes(targets, color, page_transform, final_ctm)` produces the content stream for the Redact-mode colored rectangle overlay.

The overlay is appended after the page's main content stream. At that point, the CTM may not be the identity matrix — the content stream may have left an active transformation. The overlay must draw in page space regardless of what the prior stream did.

Steps:

1. Compute `final_page_ctm(operations)` by simulating the CTM through the entire rewritten content stream (same logic as vector neutralization).
2. Compute the inverse of `page_transform` (the crop-box and rotation normalization applied to convert PDF coordinates to page space).
3. Emit `q` to save the graphics state.
4. Emit `cm` with the product of the inverse CTM and the inverse page transform, so that subsequent coordinates are interpreted in page space.
5. Emit `rg` with the fill color.
6. For each target quad: emit a `re` or path sequence followed by `f` to paint a filled rectangle or quadrilateral.
7. Emit `Q` to restore the graphics state.

## 9. Annotation removal

For each annotation on the page:

1. Parse the annotation's `Rect` array (PDF rectangle in the page's user space).
2. Transform the rectangle to normalized page space using the page transform.
3. Test against all targets using AABB intersection.
4. If any intersection: mark the annotation for removal from the page's `Annots` array.

Additional rules applied regardless of intersection:

- **FileAttachment annotations** are always removed. They reference embedded files that exist outside the content stream and would survive content-stream redaction intact.
- **Annotations without a `Rect`** are conservatively removed, with the exception of Link annotations (which are positional by definition and cannot contain content).

## 10. Why it was coded this way

| Decision | Reason |
|---|---|
| Deferred cleanup | Prevents removal of shared XObjects before all referencing pages are processed. Removing during the loop corrupts the object graph. |
| Operator whitelist instead of blacklist | Unknown operators are rejected with an explicit error. A blacklist approach would silently pass through operators the engine does not understand, potentially allowing redacted content to survive in an unrecognized form. |
| Kern compensation | Legal and regulatory documents depend on layout stability. Removing glyphs without compensation shifts surrounding text, changes line breaks, and may alter the visible meaning of adjacent content. |
| Final CTM simulation for overlay | The overlay stream is appended after the page content stream. The content stream may have left any CTM active. Without inverting the final CTM, the overlay coordinates would be interpreted in whatever space the content stream ended in, not in page space. |
| FileAttachment always removed | File attachments contain the attached file's bytes directly in the PDF. They do not appear in the content stream and are invisible to content-stream redaction. Always removing them closes the data exfiltration path. |

## 11. What would break

- **Removing objects during the page loop:** Shared XObjects (logos, repeated images) disappear before other pages use them. The output PDF references missing objects and is corrupt in any conforming viewer.
- **Not inverting the final CTM:** The overlay is drawn in the coordinate space left active by the content stream, not in page space. The colored rectangles appear at the wrong position, size, or orientation.
- **Not removing FileAttachment annotations:** The attached file survives redaction. An adversary with access to the output PDF can extract the original unredacted document from the attachment.
- **Using a blacklist for operators:** Any operator not in the blacklist passes through silently. A PDF crafted with a non-standard operator carrying redacted text would survive the pipeline unchallenged.
