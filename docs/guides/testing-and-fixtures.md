---
title: Testing and Fixtures
---

# Testing and Fixtures

## Test layers

### Unit tests

Used for:

- parser correctness
- geometry math
- target normalization
- string serialization and parsing
- search match coalescing

### Integration tests

Used for:

- fixture open
- text extraction
- search-driven targeting
- redaction apply and save
- reopen-after-save verification

## Fixture corpus

The corpus under `tests/fixtures/` exercises every supported path of the engine. Current fixtures:

- **simple-text.pdf** — baseline Type1 Helvetica with `WinAnsiEncoding`
- **multi-page.pdf** — multi-page traversal
- **rotated-text.pdf** — page rotation + rotated CTMs
- **vector-heavy.pdf** — path painting and vector bounds
- **vector-vy-curves.pdf** — `v`/`y` Bezier shorthand coverage
- **image-xobject.pdf** — Image XObject invocation redaction
- **inline-image.pdf** — inline `BI`/`ID`/`EI` + `BDC` with dictionary operand
- **annotations.pdf** — annotation dict removal
- **metadata-attachments.pdf** — `/Info`, `/Metadata`, and embedded-files stripping
- **type0-search.pdf** — `Type0` + `Identity-H` + `ToUnicode` search coverage
- **winansi-font.pdf** — Windows-1252 repertoire beyond ASCII
- **encoding-differences.pdf** — `/Encoding /Differences` + Adobe Glyph List subset
- **extgstate-font.pdf** — font installed via an ExtGState `gs` operator
- **form-xobject-text.pdf** / **form-xobject-nested.pdf** — Form XObject copy-on-write redaction (single + nested)
- **nested-cm.pdf** — nested `q`/`cm`/`Q` blocks
- **incremental-update.pdf** — incremental update chain (later revision replaces earlier content)
- **xref-object-stream.pdf** — PDF 1.5+ xref stream + object streams
- **ocg-hidden-layer.pdf** / **ocg-hidden-content.pdf** / **ocg-base-state-off.pdf** — Optional Content Group handling (rejection and `sanitizeHiddenOcgs` opt-in)
- **lzw-content.pdf** — `/Filter /LZWDecode` content stream with default `/EarlyChange 1`
- **run-length-content.pdf** — `/Filter /RunLengthDecode` content stream
- **bx-ex-compat.pdf** — BX/EX compatibility section enclosing an unrecognized operator
- **dense-layout.pdf** — dense tabular layout (4 rows, 2pt apart) to guard visual-line grouping

## Fixture generation

Fixtures are generated from:

```bash
node tests/fixtures/generate-fixtures.mjs
```

Keep generated PDFs and the generator script aligned when the subset expands.

## Required checks before merge

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
pnpm --filter @fabifont/open-redact-pdf build
pnpm --filter open-redact-pdf-demo-web build
pnpm --filter open-redact-pdf-demo-web test
```
