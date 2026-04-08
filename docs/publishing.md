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
