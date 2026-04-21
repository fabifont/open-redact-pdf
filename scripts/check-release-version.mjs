import { readdirSync, readFileSync, statSync } from "node:fs";
import { resolve } from "node:path";

const repoRoot = resolve(import.meta.dirname, "..");
const tag = process.argv[2];

if (!tag) {
  console.error("usage: node scripts/check-release-version.mjs <tag>");
  process.exit(1);
}

const version = tag.startsWith("v") ? tag.slice(1) : tag;
const rootPackage = JSON.parse(readFileSync(resolve(repoRoot, "package.json"), "utf8"));
const sdkPackage = JSON.parse(readFileSync(resolve(repoRoot, "packages/ts-sdk/package.json"), "utf8"));
const cargoToml = readFileSync(resolve(repoRoot, "Cargo.toml"), "utf8");
const cargoVersion = matchValue(cargoToml, /^\s*version\s*=\s*"([^"]+)"\s*$/m);

assertVersion("tag", version);
assertVersion("root package.json", rootPackage.version);
assertVersion("packages/ts-sdk/package.json", sdkPackage.version);
assertVersion("Cargo.toml [workspace.package].version", cargoVersion);

if (rootPackage.version !== version || sdkPackage.version !== version || cargoVersion !== version) {
  console.error(`release version mismatch: tag=${version}, root=${rootPackage.version}, sdk=${sdkPackage.version}, cargo=${cargoVersion}`);
  process.exit(1);
}

// Every crates/*/Cargo.toml inter-crate dep pin must match the workspace
// version. Otherwise `cargo publish` resolves path-dep siblings from
// crates.io at a stale version, which either fails to compile (if the new
// crate uses a symbol only the new sibling exposes) or silently ships a
// build that does not match what the workspace was tested against.
const cratesDir = resolve(repoRoot, "crates");
const crateEntries = readdirSync(cratesDir).filter((entry) =>
  statSync(resolve(cratesDir, entry)).isDirectory(),
);
const pinPattern = /^\s*[A-Za-z0-9_-]+\s*=\s*\{[^}]*package\s*=\s*"(open-redact-pdf[A-Za-z0-9_-]*)"[^}]*version\s*=\s*"([^"]+)"[^}]*\}/gm;
const pinMismatches = [];
for (const entry of crateEntries) {
  const manifestPath = resolve(cratesDir, entry, "Cargo.toml");
  let manifest;
  try {
    manifest = readFileSync(manifestPath, "utf8");
  } catch {
    continue;
  }
  pinPattern.lastIndex = 0;
  let match;
  while ((match = pinPattern.exec(manifest)) !== null) {
    const [, pkg, pinned] = match;
    if (pinned !== version) {
      pinMismatches.push(`${manifestPath}: ${pkg} pinned at "${pinned}" (expected "${version}")`);
    }
  }
}
if (pinMismatches.length > 0) {
  console.error("inter-crate version pin mismatch:");
  for (const line of pinMismatches) {
    console.error(`  ${line}`);
  }
  process.exit(1);
}

console.log(
  `release version ${version} matches root package, TS SDK, Cargo workspace, and ${crateEntries.length} inter-crate pins`,
);

function matchValue(source, pattern) {
  const match = source.match(pattern);
  if (!match) {
    console.error(`failed to extract version from ${pattern}`);
    process.exit(1);
  }
  return match[1];
}

function assertVersion(label, value) {
  if (!/^\d+\.\d+\.\d+([-.][0-9A-Za-z.]+)?$/.test(value)) {
    console.error(`${label} is not a valid semver-like version: ${value}`);
    process.exit(1);
  }
}
