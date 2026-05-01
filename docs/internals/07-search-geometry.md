# Search Geometry and Match Modeling

## 1. The search problem

PDF stores text in content-stream order, which may not match visual (reading) order. In a two-column layout, the content stream may interleave characters from the left column with characters from the right column. A simple left-to-right scan of the raw character sequence would produce nonsense strings and miss every real match.

Search must operate on visual text — the text a reader sees — not on raw content-stream text.

## 2. Visual display construction (`build_visual_display`)

The pipeline begins by converting the flat glyph list produced by the text extractor into a visually ordered sequence of characters.

### `build_visual_lines`

Groups visible glyphs into lines using an anchor-based y-proximity check:

1. Pre-sort all glyphs by `center_y` descending, then by `x` ascending (top-to-bottom, left-to-right within each approximate row).
2. Walk the sorted list. For each glyph, scan existing lines in order. A glyph joins a line if both conditions hold:
   - **Y-anchor check.** `|glyph_center_y − line.anchor_y| + 1e-6 < height_ref × 0.10`, where `line.anchor_y` is the y-centre of the first glyph placed on the line (fixed; never updated) and `height_ref` is the maximum `bbox.height` seen on the line so far (or the candidate glyph's height, whichever is larger). The anchor is fixed so the membership window cannot drift as glyphs accumulate.
   - **X-monotonicity check.** The glyph's `bbox.x` is at most `1pt` behind the last-placed glyph's `bbox.x`. This prevents a left-margin glyph from a lower row from joining a higher row once it passes the earlier row's x-watermark, which the (y-desc, x-asc) feeder sort guarantees is in place.
3. If no existing line accepts the glyph, a new line opens with that glyph as its anchor.

The 10% proportional tolerance accommodates intra-line y-jitter from mixed-size fonts on the same baseline (e.g. a 10pt + 8pt combination produces a y-centre delta of ≈ 0.56pt, well within the 0.80pt tolerance under a 10pt anchor) while keeping rows separated by more than one glyph-height-tenth distinct, including sub-1pt leading at small font sizes (a 6pt row 0.5pt below another splits cleanly because `0.5pt > 4.8pt × 0.10 = 0.48pt`).

### Building the display string

After grouping:

1. Sort each line's glyphs by `x` ascending (left-to-right reading order).
2. Sort lines by their average `center_y` descending (top-to-bottom in standard PDF coordinates where Y increases upward).
3. Walk consecutive glyphs within each line. If the horizontal gap between them exceeds `min(prev_height, curr_height) * 0.3`, insert a synthetic space character.
4. Insert a newline character between consecutive lines.

### Output

`build_visual_display` returns a pair:

- `display_chars: Vec<char>` — the visually ordered character sequence including synthetic spaces and newlines.
- `display_to_glyph: Vec<Option<usize>>` — parallel index array mapping each display position to the originating glyph index (or `None` for synthetic characters).

## 3. Search normalization (`build_search_index`)

The normalized index is built from `display_chars` for case-insensitive, whitespace-normalized matching:

1. Collapse all runs of whitespace (spaces and newlines) to a single space character.
2. Lowercase every character using `char::to_lowercase()`, which handles multi-character Unicode foldings (e.g., the German "ß" lowercases to "ss").
3. Build `normalized_to_display`: a mapping from byte positions in the normalized text to character positions in the display string.

## 4. The byte-vs-char offset bug (and fix)

Rust's `str::find()` returns **byte** offsets. `str::len()` returns **byte** length. But the original implementation built `normalized_to_display` indexed by **character** position — one entry per Unicode scalar value.

Any multi-byte UTF-8 character in the normalized text (e.g. an accented "é" from a European CV or a German umlauted name) caused `str::find()` to return a byte offset that landed in the middle of the character-indexed array. All match positions after the first multi-byte character were shifted, causing the engine to highlight wrong text.

**Fix:** `normalized_to_display` now has one entry per UTF-8 **byte** of the normalized text. When pushing a character `c` to the normalized string, we push `c.len_utf8()` copies of the corresponding display index into `normalized_to_display`. Byte offset `i` from `str::find()` maps directly to `normalized_to_display[i]`.

## 5. Match finding

Given a query string, the match pipeline is:

1. Normalize the query the same way (whitespace collapse, `to_lowercase`).
2. Call `str::find()` on the normalized text to get a byte-offset range `[start, end)`.
3. Map `start` and `end` through `normalized_to_display` to get display character positions.
4. Map display positions through `display_to_glyph` to get glyph indices (skipping `None` entries for synthetic characters).
5. Retrieve the page-space quad from each matched glyph.
6. Pass the quad set to `coalesce_match_quads`.

## 6. Quad coalescing

Individual per-glyph quads are merged into a smaller set of visually coherent highlight rectangles.

### Sort order

Quads are sorted before merging:

- Different visual lines: by descending Y center (top-to-bottom).
- Same visual line (Y centers within 1.5 pt): by ascending X (left-to-right).

### Merge condition

Two adjacent quads are merged if both:

- `vertical_overlap ≥ min_height * 0.45` — they share enough vertical extent to be on the same line.
- `horizontal_gap ≤ min_height * 0.8` — the gap between them is not too large.

### Padding

Each final merged quad is expanded outward:

- `padding_x = max(height * 0.08, 0.6)` points on each side horizontally.
- `padding_y = max(height * 0.12, 0.8)` points on each side vertically.

This ensures the highlight visually covers ascenders, descenders, and tight sidebearings.

## 7. Why it was coded this way

| Decision | Reason |
|---|---|
| Visual reordering before search | Content-stream order does not match reading order in multi-column or complex layouts. |
| Byte-indexed `normalized_to_display` | Rust's `str::find()` returns byte offsets, not char indices. Byte indexing makes the mapping direct and correct for all UTF-8 input. |
| Synthetic spaces for gaps | PDF glyph sequences contain no explicit inter-word spaces; gaps must be inferred from geometry. |
| Quad coalescing | Per-character highlight boxes produce hundreds of tiny rectangles for a single match and look incorrect in viewers. Coalesced quads produce clean word or line highlights. |
| Empirical thresholds (0.3, 0.55, 0.35, 0.45, 0.8) | Tuned against real-world PDFs with varying font sizes, leading, and column layouts. Hard boundaries do not exist in the PDF specification. |

## 8. What would break

- **Per-character indexing instead of per-byte:** Any UTF-8 multi-byte character in the matched text causes all subsequent match positions to shift, highlighting wrong glyphs.
- **No visual reordering:** Search finds text at the wrong positions in multi-column and mixed-direction layouts.
- **Too-aggressive line merging (raising thresholds):** Lines that are visually separate get merged; a query that spans the boundary of two paragraphs matches text that does not appear adjacent to the reader.
- **No quad coalescing:** Each matched glyph produces an individual highlight rectangle, resulting in hundreds of overlapping boxes per word and incorrect visual output in PDF viewers.
