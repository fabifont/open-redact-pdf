import { readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const repoRoot = resolve(import.meta.dirname, "..");

const rustInputs = [
  join(repoRoot, "Cargo.toml"),
  join(repoRoot, "Cargo.lock"),
  ...collectFiles(join(repoRoot, "crates"), (path) => path.endsWith(".rs") || path.endsWith("Cargo.toml")),
];
const wasmOutput = join(repoRoot, "packages", "ts-sdk", "vendor", "pdf-wasm", "pdf_wasm_bg.wasm");

const tsInputs = [
  join(repoRoot, "packages", "ts-sdk", "package.json"),
  join(repoRoot, "packages", "ts-sdk", "tsconfig.json"),
  ...collectFiles(join(repoRoot, "packages", "ts-sdk", "src"), (path) => path.endsWith(".ts") || path.endsWith(".d.ts")),
];
const tsOutputs = [
  join(repoRoot, "packages", "ts-sdk", "dist", "index.js"),
  join(repoRoot, "packages", "ts-sdk", "dist", "index.d.ts"),
  join(repoRoot, "packages", "ts-sdk", "dist", "types.d.ts"),
];

const wasmStale = isStale(wasmOutput, rustInputs);
if (wasmStale) {
  run("pnpm", ["wasm:build"]);
}

const tsStale = tsOutputs.some((output) => isStale(output, tsInputs)) || wasmStale;
if (tsStale) {
  run("pnpm", ["--filter", "@open-redact-pdf/sdk", "build"]);
}

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: repoRoot,
    stdio: "inherit",
  });
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

function isStale(outputPath, inputPaths) {
  let outputTime;
  try {
    outputTime = statSync(outputPath).mtimeMs;
  } catch {
    return true;
  }
  return inputPaths.some((inputPath) => statSync(inputPath).mtimeMs > outputTime);
}

function collectFiles(root, include) {
  const entries = readdirSync(root, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const path = join(root, entry.name);
    if (entry.isDirectory()) {
      files.push(...collectFiles(path, include));
      continue;
    }
    if (include(path)) {
      files.push(path);
    }
  }
  return files;
}
