---
title: Architecture
---

# Architecture

The engine is organized as a Rust workspace with narrow internal crates and a single public facade crate.

## Pipeline

1. `pdf_objects` parses the file structure, page tree, and streams.
2. `pdf_content` tokenizes and parses page content streams into low-level operations.
3. `pdf_text` interprets text state to extract text items, glyph geometry, and search matches.
4. `pdf_targets` normalizes rectangle, quad, and quad-group authoring input into canonical page-space geometry.
5. `pdf_redact` plans and applies redactions against text, vector, image, and annotation content.
6. `pdf_writer` rewrites the document as a deterministic full save.
7. `pdf_wasm` exposes the same API to browser code.

## Layer boundaries

### Authoring layer

Examples:

- drag rectangles
- text selections
- search results
- future regex or OCR matches

This layer belongs in UI code or higher-level orchestration.

### Canonical target layer

All authoring tools are compiled into page-space geometry targets:

- rectangles
- quads
- quad groups

This keeps the apply pipeline independent from specific UI concepts.

### Apply layer

The apply pipeline works from geometry and page content. It does not care whether a target came from a drag interaction or a text search term.

## Design rules

- Page-space geometry is the canonical input to redaction application.
- UI authoring concepts are kept outside the engine.
- Unsupported features fail explicitly when they affect correctness.
- Output preserves unredacted text when content can be safely rewritten.
- Browser integrations should treat preview rendering as separate from redaction logic.
