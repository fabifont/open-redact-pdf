---
title: Publishing and Documentation Ingestion
---

# Publishing and Documentation Ingestion

## GitHub Pages

This repository is configured to publish the `docs/` directory through a GitHub Pages workflow using the official Jekyll build action and the official Pages deploy action.

The Pages workflow:

- treats `docs/` as the site source
- builds Markdown pages with Jekyll
- deploys the generated site to GitHub Pages

## Package publishing

This repository also includes a release workflow at `.github/workflows/release.yml`.

That workflow is triggered by tags that match `v*`, for example:

```bash
git tag v0.1.0
git push origin v0.1.0
```

The workflow:

- verifies that the Git tag version matches:
  - the root `package.json`
  - `packages/ts-sdk/package.json`
  - `Cargo.toml` workspace version
- runs the release validation build
- publishes `@open-redact-pdf/sdk` to npm
- publishes the Rust crate set to crates.io in dependency order

## Required repository secrets

Set these GitHub Actions secrets before using the release workflow:

- `NPM_TOKEN`: npm automation token with publish access for `@open-redact-pdf/sdk`
- `CARGO_REGISTRY_TOKEN`: crates.io API token with publish access

## Release versioning model

The repo currently uses one shared release version across:

- the root workspace version
- the TypeScript SDK package version
- the published Rust crates

Before tagging a release, update those versions together.

## Rust crate publishing model

The public Rust crate `open-redact-pdf` depends on internal crates in this workspace. Because of that, the release workflow publishes the internal crates and then publishes `open-redact-pdf`.

The internal `pdf_wasm` crate is not published to crates.io.

## Why the docs are organized this way

The documentation tree is intentionally structured for both human readers and later machine ingestion:

- stable page paths
- one major topic per page
- reference pages separated from guides
- code examples kept near the API they describe
- explicit supported-subset and failure-model pages

That structure makes the site suitable for later Context7 ingestion or any other documentation indexer that benefits from stable topical pages.

## Suggested publishing flow

1. Enable GitHub Pages in the repository settings
2. Use GitHub Actions as the Pages source
3. Let the `docs` workflow deploy the site from `main`

## Authoring rules for future docs

- Prefer stable file names over changelog-like page names
- Keep one API or workflow topic per page
- Update API reference pages in the same PR as public API changes
- Link from guides back to reference pages instead of duplicating signatures

## Related files

- `docs/index.md`
- `.github/workflows/docs.yml`
- `docs/_config.yml`
