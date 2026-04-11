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

Current fixtures include:

- simple text
- multi-page text
- rotated text
- vector-heavy content
- image XObjects
- annotations
- metadata and attachments
- `Type0` search coverage

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
