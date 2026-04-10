# Redaction Target Model

## 1. Why rectangles alone are not enough

PDF text is not restricted to horizontal baselines. The text matrix can rotate, skew, or scale individual characters or entire text blocks. A word placed at 45 degrees occupies a diamond-shaped area in page space, not an axis-aligned rectangle.

An axis-aligned bounding box around a rotated word is significantly larger than the word itself. Using that enlarged box as a redaction target would either catch adjacent content that should not be redacted or require per-case manual correction. A quadrilateral that follows the actual glyph corners avoids both problems.

## 2. The three target types

All coordinates are in normalized page space: crop-box-relative, with page rotation applied. This means callers do not need to know the internal PDF coordinate system; they work in the coordinate space the viewer presents.

### `Rect`

```
Rect { page_index, x, y, width, height }
```

An axis-aligned rectangle. The simplest and most common case. Used when the caller knows the approximate region to redact without needing glyph-level precision.

### `Quad`

```
Quad { page_index, points: [Point; 4] }
```

An arbitrary quadrilateral defined by four corner points in page space. Handles rotated or skewed text exactly. Points are conventionally ordered bottom-left, bottom-right, top-right, top-left, matching the PDF specification's quad point convention.

### `QuadGroup`

```
QuadGroup { page_index, quads }
```

A collection of quads that together form a single logical match — for example, the quads for each word in a multi-word phrase, or the quads spanning multiple lines. Grouped as a unit so the engine treats them as a single redaction target and can apply a single overlay across the group.

## 3. Normalization (`normalize_plan`)

User-facing `RedactionPlan` values are converted to `NormalizedRedactionPlan` before any engine work begins. Normalization:

- Validates page indices against the document page count.
- Validates that `Rect` dimensions are positive and finite.
- Validates that `QuadGroup` lists are non-empty.
- Converts each `Rect` to a single-quad `NormalizedPageTarget` (four corners derived from x, y, width, height).
- Converts each `QuadGroup` to a single `NormalizedPageTarget` carrying the full quad list and a pre-computed axis-aligned union bounding box.
- Rejects `overlay_text` if present (replacement text is not implemented; returning an explicit error is safer than silently dropping the field).
- Applies option defaults:
  - `mode` → `Redact`
  - `fill_color` → black
  - `remove_annotations` → `true`
  - `strip_metadata` → `false`
  - `strip_attachments` → `false`

Normalization is the only place where input validation occurs. All downstream pipeline stages receive already-validated data.

## 4. Intersection testing

`NormalizedPageTarget` exposes two intersection methods used by the pipeline to decide which content overlaps a target:

- `intersects_rect(rect)` — compares the axis-aligned bounding box of the target's quads against the provided rect. O(1).
- `intersects_quad(quad)` — compares the bounding box of the target's quads against the bounding box of the provided quad. O(1).

Both are AABB approximations. For axis-aligned text (the overwhelmingly common case), the approximation is exact. For rotated text, the bounding box is a conservative over-approximation: it may flag content as intersecting when the actual quads do not overlap. The consequence is a false-positive redaction in a rare case, which is the safe direction for a security tool.

Precise polygon intersection (e.g. SAT or Sutherland-Hodgman) would be more accurate for rotated quads but would complicate the implementation without meaningful benefit for the current supported text subset.

## 5. The three redaction modes

| Mode | Text bytes | Glyph positioning | Visual overlay | Use case |
|---|---|---|---|---|
| `Strip` | Removed from stream | Text reflows | None | Minimal output size; layout preservation not required |
| `Redact` | Kern-compensated in place | Preserved | Colored filled quad | Default; visually marks the redacted region with a solid rectangle |
| `Erase` | Kern-compensated in place | Preserved | None | Clean appearance; redacted areas become blank space without a visible marker |

`Strip` is simpler to implement but changes line lengths and can shift surrounding text. `Redact` and `Erase` both use kern compensation (see [Redaction Application Pipeline](09-redaction-pipeline.md)) to hold the positions of surrounding glyphs constant, which is critical for legal and regulatory documents where layout integrity is a requirement.

## 6. Why it was coded this way

| Decision | Reason |
|---|---|
| `#[serde(tag = "kind")]` on target variants | Produces JSON like `{ "kind": "rect", ... }` that is unambiguous and human-readable. Tag-based discrimination avoids wrapper objects and keeps the TypeScript SDK bindings clean. |
| Three distinct types instead of a generic polygon | `Rect` covers the dominant case with minimal input from callers. `Quad` and `QuadGroup` are progressively more expressive. Keeping `Rect` as a named type prevents callers from needing to compute quad corners for the common case. |
| `QuadGroup` for multi-glyph matches | A text search match spans many glyphs. Returning one target per glyph would produce hundreds of independent targets and complicate overlay generation. Grouping at the target level lets the overlay stage draw one visual region per match. |
| Pre-computed union bounding box in `NormalizedPageTarget` | Intersection tests are called once per glyph per target per page. Storing the bounding box avoids recomputing it on every call. |
