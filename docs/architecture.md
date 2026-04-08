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

## Design Rules

- Page-space geometry is the canonical input to redaction application.
- UI authoring concepts are kept outside the engine.
- Unsupported features fail explicitly when they affect correctness.
- Output preserves unredacted text when content can be safely rewritten.

