import { readFileSync } from "node:fs";
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

console.log(`release version ${version} matches root package, TS SDK, and Cargo workspace`);

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
