---
title: Canonical Target Model
---

# Canonical Target Model

The engine accepts page-space geometry targets, not screen-space UI artifacts.

## Coordinate system

- Units are PDF user-space units after page normalization
- Coordinates are page-local
- The origin is the normalized page origin
- Page rotation and crop translation are normalized before redaction logic runs

## Target types

### Rectangle targets

Useful for drag-based authoring and coarse manual redaction.

### Quad targets

Useful for text-aligned authoring where a single four-point region is sufficient.

### Quad-group targets

Useful for:

- multi-line text matches
- discontinuous text
- search and regex results
- future OCR or entity-driven workflows

## Normalization rules

Normalization is handled by `pdf_targets`.

The normalization layer:

- validates page indices
- rejects empty or non-intersecting targets
- converts rectangles to canonical quads
- preserves quad groups as independent geometry regions
- computes merged bounds for efficient intersection checks

## Why the model is geometry-first

The apply pipeline does not need to know whether a target came from:

- a drag interaction
- a text selection
- a search term
- a regex pipeline
- a future OCR pass

Everything is compiled into page-space geometry before redaction is applied.

## Example

```json
{
  "targets": [
    {
      "kind": "rect",
      "pageIndex": 0,
      "x": 72,
      "y": 500,
      "width": 120,
      "height": 18
    },
    {
      "kind": "quadGroup",
      "pageIndex": 0,
      "quads": [
        [
          { "x": 320, "y": 530 },
          { "x": 378, "y": 530 },
          { "x": 378, "y": 544 },
          { "x": 320, "y": 544 }
        ]
      ]
    }
  ]
}
```
