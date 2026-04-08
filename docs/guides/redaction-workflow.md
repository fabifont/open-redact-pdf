---
title: Redaction Workflow
---

# Redaction Workflow

## End-to-end pipeline

1. Parse the PDF structure
2. Traverse the page tree and normalize page boxes
3. Parse page content streams into operations
4. Extract glyph geometry and searchable text
5. Normalize authoring input into canonical page-space targets
6. Remove or neutralize intersecting text, vectors, images, and annotations
7. Paint visible redaction fills
8. Save a new deterministic PDF

## Manual rectangles

Manual rectangle authoring is a UI convenience layer. The engine still receives canonical page-space targets, not DOM coordinates.

## Search-driven redaction

Search works in visual glyph order and returns quad groups. These can be passed directly into `apply_redactions`.

## Apply semantics

- text glyphs intersecting a target are removed from rewritten text-showing operators
- intersecting path paints are neutralized
- intersecting image draws are removed conservatively at invocation level
- optional annotation removal can strip intersecting annotation objects from touched pages

## Save semantics

The writer emits a new PDF with a full save. The output does not rely on hidden references back to the original file.
