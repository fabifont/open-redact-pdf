import { existsSync, readdirSync, rmSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const repoRoot = resolve(import.meta.dirname, "..");
const cacheRoot = join(homedir(), ".cache", ".wasm-pack");
const cachedBindgenDir = findCachedBindgenDir(cacheRoot);
const outDir = resolve(repoRoot, "packages/ts-sdk/vendor/pdf-wasm");

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

const result = run(args, env);
if (result.status === 0) {
  normalizeWasmOutput();
  process.exit(0);
}

const stderr = `${result.stderr ?? ""}`;
if (stderr.includes("Read-only file system")) {
  const retry = run([...args, "--no-opt"], env);
  if (retry.status === 0) {
    normalizeWasmOutput();
  }
  process.exit(retry.status ?? 1);
}

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

function run(args, env) {
  const result = spawnSync("wasm-pack", args, {
    cwd: repoRoot,
    stdio: "pipe",
    env,
    encoding: "utf8",
  });
  if (result.stdout) {
    process.stdout.write(result.stdout);
  }
  if (result.stderr) {
    process.stderr.write(result.stderr);
  }
  return result;
}

function normalizeWasmOutput() {
  for (const relativePath of [".gitignore", "package.json"]) {
    const path = join(outDir, relativePath);
    if (existsSync(path)) {
      rmSync(path);
    }
  }
}
