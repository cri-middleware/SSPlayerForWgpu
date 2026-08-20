#!/usr/bin/env node
// Fail the publish/pack of a package when the artifacts it advertises are not
// actually on disk.
//
// dist/ and wasm/ are gitignored build outputs. The package.json `files`
// allow-list ships them, so a `pnpm publish` that skipped
// `scripts/build.sh platform=web` would upload a tarball with those paths simply
// missing — npm accepts it silently and that version is then immutable
// (a permanently broken release). This script turns that silent hole into a
// loud abort: it checks every concrete path the package.json points at
// (main, types, unpkg, jsdelivr, every exports target) plus that each `files`
// directory entry (dist, wasm, …) exists and is non-empty.
//
// Wired as `prepack` in each publishable package, so it guards `pnpm pack`,
// `pnpm publish`, and `pnpm -r publish` alike — no matter how publish is
// invoked. Runs from the package directory (prepack's cwd).

import { existsSync, statSync, readdirSync, readFileSync } from 'node:fs';
import { join, dirname, resolve } from 'node:path';

const pkgDir = process.argv[2] ? resolve(process.argv[2]) : process.cwd();
const pkgJsonPath = join(pkgDir, 'package.json');

if (!existsSync(pkgJsonPath)) {
  console.error(`verify-publish-artifacts: no package.json at ${pkgDir}`);
  process.exit(1);
}

const pkg = JSON.parse(readFileSync(pkgJsonPath, 'utf8'));
const missing = [];

/** Does a directory exist and contain at least one file (recursively)? */
function dirHasFiles(absDir) {
  if (!existsSync(absDir) || !statSync(absDir).isDirectory()) return false;
  for (const entry of readdirSync(absDir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (dirHasFiles(join(absDir, entry.name))) return true;
    } else if (entry.isFile()) {
      return true;
    }
  }
  return false;
}

/** Require a concrete `./…` file reference to exist. */
function requireFile(ref, label) {
  if (typeof ref !== 'string' || !ref.startsWith('.')) return;
  const abs = join(pkgDir, ref);
  if (!existsSync(abs)) missing.push(`${label}: ${ref}`);
}

/** Does a directory (recursively) contain at least one *.wasm binary? */
function dirHasWasm(absDir) {
  if (!existsSync(absDir) || !statSync(absDir).isDirectory()) return false;
  for (const entry of readdirSync(absDir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (dirHasWasm(join(absDir, entry.name))) return true;
    } else if (entry.isFile() && entry.name.endsWith('.wasm')) {
      return true;
    }
  }
  return false;
}

/** For a glob like "./wasm/*", require its directory to be non-empty. */
function requireGlobDir(ref, label) {
  const dir = dirname(ref.replace(/\*.*$/, 'x')); // "./wasm/*" -> "./wasm"
  const abs = join(pkgDir, dir);
  if (!dirHasFiles(abs)) missing.push(`${label}: ${ref} (dir "${dir}" empty or absent)`);
}

// 1. Top-level entry fields.
requireFile(pkg.main, 'main');
requireFile(pkg.types, 'types');
requireFile(pkg.unpkg, 'unpkg');
requireFile(pkg.jsdelivr, 'jsdelivr');

// 2. Every string leaf under `exports` (concrete paths and glob dirs).
(function walkExports(node, path) {
  if (typeof node === 'string') {
    if (!node.startsWith('.')) return;
    if (node.includes('*')) requireGlobDir(node, `exports${path}`);
    else requireFile(node, `exports${path}`);
  } else if (node && typeof node === 'object') {
    for (const [k, v] of Object.entries(node)) walkExports(v, `${path}["${k}"]`);
  }
})(pkg.exports, '');

// 3. Each `files` entry: directories must exist and be non-empty; concrete
//    files must exist. (README/LICENSE are git-tracked, so this effectively
//    guards the build-output dirs — dist, wasm.)
for (const entry of pkg.files ?? []) {
  if (entry.includes('*')) continue; // globs handled well enough via exports/entries
  const abs = join(pkgDir, entry);
  if (!existsSync(abs)) {
    missing.push(`files: ${entry} (missing)`);
  } else if (statSync(abs).isDirectory()) {
    if (!dirHasFiles(abs)) {
      missing.push(`files: ${entry} (directory is empty)`);
    } else if ((entry === 'wasm' || entry.endsWith('/wasm')) && !dirHasWasm(abs)) {
      // Non-emptiness is not enough: a wasm/ populated with only VERSION /
      // LICENSE but missing the actual *_bg.wasm binary would still pass and
      // ship a broken, immutable release. Require the binary itself.
      missing.push(`files: ${entry} (no *.wasm binary present)`);
    }
  }
}

if (missing.length > 0) {
  console.error(`\n✗ ${pkg.name}: refusing to publish — advertised artifacts are missing:\n`);
  for (const m of missing) console.error(`    - ${m}`);
  console.error(
    `\n  These are gitignored build outputs. From the repo root, run:\n` +
      `    ./scripts/build.sh platform=web    # wasm-pack -> wasm/, tsdown -> dist/\n` +
      `  then retry the publish.\n`
  );
  process.exit(1);
}

// stderr, not stdout: `npm pack --json` / `npm publish --json` emit their result
// on stdout, and a prepack script writing there corrupts that JSON for anything
// parsing it. The success line is progress output, so stderr is where it belongs.
console.error(`✓ ${pkg.name}: publish artifacts present`);
