# List available recipes
default:
    @just --list

# ── Build ─────────────────────────────────────────────────────────────────────

# Full build: wasm → ts-sdk → demo
build: wasm
    pnpm -r build

# Build the WASM bundle only
wasm:
    node scripts/build-wasm.mjs

# Build the TS SDK only
sdk:
    pnpm --filter @fabifont/open-redact-pdf build

# Build the demo app only (requires wasm + sdk already built)
demo:
    pnpm --filter open-redact-pdf-demo-web build

# ── Dev ───────────────────────────────────────────────────────────────────────

# Start the demo dev server
dev:
    pnpm --filter open-redact-pdf-demo-web dev

# Serve the docs site locally
docs:
    mkdocs serve

# ── Test ──────────────────────────────────────────────────────────────────────

# Run all tests (Rust + JS)
test: test-rust test-js

# Run Rust tests only
test-rust:
    cargo test --workspace

# Run JS/TS type checks only
test-js:
    pnpm -r test

# ── Lint ──────────────────────────────────────────────────────────────────────

# Lint everything (Rust + JS)
lint: lint-rust lint-js

# Run Clippy
lint-rust:
    cargo clippy --workspace --all-targets -- -D warnings

# Run TS type checks
lint-js:
    pnpm -r lint

# ── Format ────────────────────────────────────────────────────────────────────

# Format everything (Rust + JS)
fmt: fmt-rust fmt-js

# Format Rust
fmt-rust:
    cargo fmt --all

# Format JS/TS with Prettier
fmt-js:
    pnpm -r format

# ── Check ─────────────────────────────────────────────────────────────────────

# Type-check everything without building
check:
    cargo check --workspace && pnpm -r typecheck

# ── Install ───────────────────────────────────────────────────────────────────

# Install JS dependencies
install:
    pnpm install
