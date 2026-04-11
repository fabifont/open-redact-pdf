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
- publishes `@fabifont/open-redact-pdf` to npm
- publishes the Rust crate set to crates.io in dependency order

If you need to retry an existing release, you can also run the workflow manually with the `release_tag` input set to a tag like `v0.1.0`. The workflow checks out that tag directly instead of using the branch you launched the run from.

## Required repository secrets

Set these GitHub Actions secrets before using the release workflow:

- `NPM_TOKEN`: npm automation token with publish access for the `@fabifont` scope
- `CARGO_REGISTRY_TOKEN`: crates.io API token with publish access

Before the first npm release, make sure the token owner can publish to the `@fabifont` scope. If the token cannot publish to that scope, `npm publish` commonly fails with `E404` on the scoped package URL.

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

## Demo deployment (Cloudflare Pages)

The interactive demo at `apps/demo-web` is deployed separately to Cloudflare Pages via `.github/workflows/deploy-demo.yml`. It is decoupled from the docs deployment because the demo build requires a full Rust/WASM toolchain, while the docs are lightweight Jekyll.

The workflow:

- triggers on pushes to `main` that touch `crates/`, `packages/`, `apps/demo-web/`, `scripts/`, or lock files
- builds the WASM bundle, TS SDK, and demo
- deploys to Cloudflare Pages using `wrangler pages deploy`

### Required repository secrets

- `CLOUDFLARE_API_TOKEN`: Cloudflare API token with Pages edit permission
- `CLOUDFLARE_ACCOUNT_ID`: Cloudflare account ID

### Custom domain

After the first deployment creates the `open-redact-pdf` project in Cloudflare Pages, add a custom domain (e.g., `open-redact-pdf.fabifont.dev`) in the Cloudflare dashboard under Pages > Custom domains.

## Authoring rules for future docs

- Prefer stable file names over changelog-like page names
- Keep one API or workflow topic per page
- Update API reference pages in the same PR as public API changes
- Link from guides back to reference pages instead of duplicating signatures

## Related files

- `docs/index.md`
- `.github/workflows/docs.yml`
- `.github/workflows/deploy-demo.yml`
- `docs/_config.yml`
