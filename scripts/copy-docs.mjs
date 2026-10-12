#!/usr/bin/env node
// Copy the documentation into an artifact, so what a user's tooling reads next to
// the library is the documentation of that library -- not the site's, which is
// built from the latest release and can be ahead. English only, as the Markdown
// source, with the layout kept so the links between pages still resolve.
//
//   node scripts/copy-docs.mjs <dest>
//
// <dest> is replaced: scripts/build.sh platform=web gives it the npm package's
// docs/, scripts/build-release.sh each C zip's docs/. Each artifact has its own
// front door in scripts/release-docs/, which the caller copies beside the pages.
//
// Left out of docs/en/:
//   index.md       the site's home page; the front door stands in for it.
//   contributing/  building this repository, which a user of the artifact does not do.
//   not *.md       assets/ holds the site's favicon and nothing a page shows.
// Everything else is taken by rule, not by list, so a page added to docs/en/ ships
// without anyone remembering to add it here.

import { cpSync, mkdirSync, readdirSync, rmSync } from 'node:fs';
import { dirname, join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const [dest, ...rest] = process.argv.slice(2);
if (!dest || rest.length > 0) {
  console.error('usage: node scripts/copy-docs.mjs <dest>');
  process.exit(2);
}

const src = resolve(dirname(fileURLToPath(import.meta.url)), '..', 'docs', 'en');
const out = resolve(dest);

const pages = readdirSync(src, { recursive: true })
  .filter((rel) => rel.endsWith('.md'))
  .filter((rel) => rel !== 'index.md' && !rel.startsWith(`contributing${sep}`))
  .sort();

rmSync(out, { recursive: true, force: true });
for (const rel of pages) {
  mkdirSync(dirname(join(out, rel)), { recursive: true });
  cpSync(join(src, rel), join(out, rel));
}
console.log(`   docs/ (${pages.length} pages)`);
