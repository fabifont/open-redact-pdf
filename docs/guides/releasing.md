---
title: Releasing a new version
---

# Releasing a new version

The repository ships the Rust workspace to crates.io and the TypeScript SDK to npm from a single Git tag. All artifacts must carry the same semver string — the CI job `release.yml` refuses to publish otherwise.

## One-shot release with cargo-release

Install [`cargo-release`](https://github.com/crate-ci/cargo-release) once:

```bash
cargo install cargo-release --locked
```

Then cut a release with a single command:

```bash
# pick one of: patch | minor | major | <exact version>
cargo release minor
```

That command, driven by `release.toml` at the workspace root, does the following in order:

1. Bumps `[workspace.package].version` in the root `Cargo.toml`.
2. Rewrites every `path = "../foo", version = "X.Y.Z"` inter-crate pin inside `crates/*/Cargo.toml` to the new version (`dependent-version = "upgrade"`).
3. Rewrites the `"version"` field in the root `package.json` and `packages/ts-sdk/package.json`.
4. Commits the result as `chore(release): vX.Y.Z`.
5. Creates and pushes the `vX.Y.Z` tag.

Pushing the tag triggers `.github/workflows/release.yml`, which runs `scripts/check-release-version.mjs` to verify all four places agree (defence in depth against a `pre-release-replacements` bug) and then publishes both targets in parallel:

- **npm** — `packages/ts-sdk` is built and pushed as `@fabifont/open-redact-pdf`.
- **crates.io** — `scripts/publish-crates.sh` publishes the eight Rust crates in topological order with retry-on-propagation-delay.

`pdf_wasm` is excluded from crates.io (`publish = false` in its `Cargo.toml`) because it is the wasm-bindgen wrapper bundled into the npm package.

## Manual fallback

If you cannot run `cargo-release` locally, the release is still reproducible by hand:

1. Edit `[workspace.package].version` in the root `Cargo.toml`.
2. For every crate under `crates/*`, edit every `path = ..., version = "X.Y.Z"` pin to the new version.
3. Edit the `"version"` field in `package.json` and `packages/ts-sdk/package.json`.
4. `cargo build --workspace` once to refresh `Cargo.lock`.
5. Commit, tag `vX.Y.Z`, push the tag.

Run `node scripts/check-release-version.mjs v$NEW_VERSION` before pushing to catch any mismatches locally; CI will run the same check and block the publish if it fails.

## Related files

- `release.toml` — cargo-release configuration
- `scripts/check-release-version.mjs` — cross-manifest version invariant check (run in CI)
- `scripts/publish-crates.sh` — ordered crates.io publish with propagation retries
- `.github/workflows/release.yml` — CI glue triggered by the `v*` tag

## Related docs

- [Development workflow](../development.md)
- [Publishing setup](../publishing.md)
