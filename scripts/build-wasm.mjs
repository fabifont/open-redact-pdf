import { existsSync, readdirSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const repoRoot = resolve(import.meta.dirname, "..");
const cacheRoot = join(homedir(), ".cache", ".wasm-pack");
const cachedBindgenDir = findCachedBindgenDir(cacheRoot);

const args = [
  "build",
  "crates/pdf_wasm",
  "--target",
  "bundler",
  "--out-dir",
  "../../packages/ts-sdk/vendor/pdf-wasm",
  "--release",
];
if (cachedBindgenDir) {
  args.push("--mode", "no-install");
}

const env = { ...process.env };
if (cachedBindgenDir) {
  env.PATH = `${cachedBindgenDir}:${env.PATH ?? ""}`;
}

const result = spawnSync("wasm-pack", args, {
  cwd: repoRoot,
  stdio: "inherit",
  env,
});

process.exit(result.status ?? 1);

function findCachedBindgenDir(cacheDir) {
  if (!existsSync(cacheDir)) {
    return null;
  }
  const candidates = readdirSync(cacheDir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => join(cacheDir, entry.name))
    .filter((directory) => existsSync(join(directory, "wasm-bindgen")))
    .sort((left, right) => statSync(right).mtimeMs - statSync(left).mtimeMs);
  return candidates[0] ?? null;
}
