---
title: Security Model
---

# Security Model

## What counts as redaction

For this project, redaction means the output PDF no longer contains the redacted text or content in referenced page content that remains accessible after save. A visible overlay is only valid after the underlying targeted content has been removed or neutralized.

## Redaction mode semantics

Three modes are supported; all modes remove or neutralize targeted content before any visual treatment:

- **`strip`** — targeted text bytes are physically removed from text-showing operators; no overlay is painted.
- **`redact`** (default) — targeted text is replaced with blank space in the operator stream; a colored fill is painted over the region.
- **`erase`** — targeted text is replaced with blank space; no overlay is painted, leaving a visible gap.

The key invariant across all modes is that the underlying content is removed or neutralized first. The overlay in `redact` mode is a UI affordance, not a substitute for structural removal.

## Current guarantees

- Intersecting text glyphs are removed from rewritten text-showing operators.
- Intersecting vector paint operations are neutralized.
- Intersecting image draws are removed conservatively at the image invocation level.
- Form XObjects whose bounding quad intersects a target are redacted via per-page copy-on-write (text glyphs, vector paint, and inner Image `Do` invocations), with nested Forms handled recursively.
- Documents whose default Optional Content configuration hides any layer are refused outright so hidden-layer text cannot slip through unredacted.
- Optional intersecting annotations can be removed from touched pages.

## Current limitations

- Image redaction is conservative and removes whole image draws when they intersect a target (partial image rewriting is future work).
- Metadata and attachment stripping are opt-in and limited to supported object layouts.
- Composite fonts are limited to `Identity-H` with a `ToUnicode` map; other CID encodings still fail explicitly.

## Non-goals in the MVP

- pretending to sanitize PDFs outside the supported subset
- whole-page rasterization as a default fallback
- UI-only overlays that leave text searchable underneath
