# Changelog

All notable changes to this project are documented in this file.

The format is loosely based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to pre-1.0 [Semantic Versioning](https://semver.org/spec/v2.0.0.html):
under `0.x.y`, every minor bump may include behavioural changes that callers should review.

## [0.6.0] — 2026-05-02

### Added

- **text**: Anchor-based visual line grouping for dense layouts
- **writer**: Mirror cross-reference shape on save (xref streams + ObjStm)
- **crypto**: Add Adobe.PubSec public-key security handler
- **redact**: Partial Image XObject rewriting

### Fixed

- **demo-web**: Pass password to pdfjs preview for encrypted PDFs
- **text**: Stricter width rejection and remove search quad padding
- **redact**: Three composition bugs in partial Image XObject rewriting
- **redact**: Isolate overlay from page-level graphics state

## [0.5.0] — 2026-04-23

### Added

- **pdf_objects**: Support LZWDecode filter with EarlyChange 0/1
- **pdf_objects**: Support RunLengthDecode filter
- **pdf_redact**: Accept BX/EX compatibility sections around unknown ops
- **api**: Cache per-page text extraction and invalidate on redaction
- **pdf_objects**: Resolve indirect /Length references for stream framing
- **pdf_text**: Support /Encoding /MacRomanEncoding for simple fonts
- **pdf_text**: Support /Encoding /StandardEncoding for simple fonts

### Fixed

- **pdf_text**: Split 1pt-apart rows in line grouping against float epsilon

## [0.4.0] — 2026-04-22

### Added

- **pdf_objects**: Authenticate user and owner passwords for RC4
- **api**: Thread password through PdfDocument, WASM, and TS SDK
- **pdf_objects**: Add AES-128-CBC primitive and crypt-filter resolution
- **pdf_objects**: Decrypt Standard Security Handler V=4 R=4 (AES-128)
- **pdf_objects**: Add AES-256 primitives and V=5 handler
- **demo**: Password prompt for encrypted PDFs
- **pdf_redact**: Opt-in sanitization of hidden OCG content
- **demo**: Toggle for sanitizeHiddenOcgs in the redaction plan sidebar
- **pdf_objects**: Support ASCII85Decode, ASCIIHexDecode, and filter chains

### Fixed

- **pdf_text**: Compose text-matrix updates in text space

## [0.3.0] — 2026-04-21

### Added

- **pdf_objects**: Decrypt Standard Security Handler V=1/2 R=2/3 (RC4, empty password)

## [0.2.0] — 2026-04-21

### Added

- **parser**: Support xref streams and object streams
- **pdf_text**: Decode simple fonts via ToUnicode and WinAnsiEncoding
- **pdf_text**: Recurse into Form XObjects for text extraction
- **pdf_redact**: Narrow Form XObject error to intersecting targets only
- **pdf_redact**: Include v and y curve shorthands in path bounds
- **pdf_text**: Resolve /Encoding /Differences through a glyph-name table
- **pdf_objects**: Support TIFF predictor (Predictor 2)
- **pdf_redact**: Refuse redaction of documents with hidden OCG layers
- **pdf_redact**: FlateDecode-compress rewritten content streams
- **pdf_redact**: Rewrite Form XObject content on intersecting redactions
- **api**: Report form_xobjects_rewritten from applyRedactions
- **pdf_redact**: Stamp overlayText labels in redact mode
- **pdf_redact**: Recurse into nested Form XObjects on redaction
- **pdf_redact**: Neutralize paint and images inside redacted Forms
- **demo**: Surface form_xobjects_rewritten in the redaction report

## [0.1.0] — 2026-04-11

### Added

- Add browser-first PDF redaction MVP
- **demo**: Auto-rebuild wasm artifacts and improve search overlay
- **pdf_text**: Sort search results in visual glyph order
- Add strip, redact, and erase redaction modes
- **demo**: Add collapsible pages, zoom controls, and improve variable names
- Expand operator allow-list and surface errors in sidebar
- **parser**: Support incremental PDF update chains via Prev pointer

### Fixed

- **demo**: Decouple rendering from text extraction
- **demo**: Clone pdf bytes before pdf.js loads
- Correct demo preview rendering and Type0 search redaction
- Correct search result rendering and graphics operator handling
- Correct saved-PDF overlay placement and browser download
- Resolve critical redaction integrity, security, and correctness issues
- Address remaining pre-release risks across engine, CI, SDK, and demo
- **demo**: Prevent double-free of PdfHandle on reload after redaction
- Correct glyph bounding box overlap and overhaul demo components
- **demo**: Allow page scroll overflow and fix collapsed page display
- **demo**: Keep collapsed page header at full width
- **demo**: Size collapsed page header to match rendered canvas width
- Defer object removal to prevent crash on shared objects across pages
- Support ExtGState fonts and correct BT and q/Q text state behavior
- **pdf_content**: Parse dict operands and skip inline images in content streams
- Correct cm operator CTM concatenation order per PDF spec
- **pdf_text**: Correct search index byte-vs-char offset mapping
- **release**: Require a tag for manual runs

### Changed

- **demo**: Simplify collapsed header layout using gap

### Build

- Run wasm build from root build script
- Add npm and crates.io release workflow
- Add Cloudflare Pages demo deployment and fix internals doc inaccuracies
- Fix cargo fmt violations and update CI action versions
- Fix wrangler-action package manager conflict in deploy workflow
- Opt into Node.js 24 runtime across all GitHub Actions workflows
- Add rust-toolchain, justfile, prettier, and cache wasm-pack in CI
- **ts-sdk**: Rename npm package to @fabifont/open-redact-pdf
- **crates**: Rename workspace packages for publishing
- **release**: Publish crates in dependency order


