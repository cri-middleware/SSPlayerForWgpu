#!/usr/bin/env node
// Copy the documentation into an artifact, so what a user's tooling reads next to
// the library is the documentation of that library -- not the site's, which is
// built from the latest release and can be ahead. English only, as the Markdown
// source, with the layout kept so the links between pages still resolve.
//
//   node scripts/copy-docs.mjs <dest> [<skill>]
//
// <dest> is replaced: scripts/build.sh platform=web gives it the npm package's
// docs/, scripts/build-release.sh each C zip's docs/. Each artifact has its own
// front door in scripts/release-docs/, which the caller copies beside the pages.
//
// <skill> is replaced by the same pages as an Agent Skill, named after the
// folder: build.sh platform=web gives it the npm package's skills/ssplayer-wgpu/.
// Its front door is SKILL.md, written here -- the npm package's front door under
// the frontmatter that names the skill. pnpm links skills/<name>/ of a direct
// dependency into the project's agent skill directories, and the npm skills CLI
// does the same, so a link that leaves the folder resolves against the agent's
// directory instead -- the pages are copied rather than pointed at.
//
// Left out of docs/en/:
//   index.md       the site's home page; the front door stands in for it.
//   development/   building this repository, which a user of the artifact does not do.
//   not *.md       assets/ holds the site's favicon and nothing a page shows.
// Everything else is taken by rule, not by list, so a page added to docs/en/ ships
// without anyone remembering to add it here.

import { cpSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const SKILL_DESCRIPTION =
  'Documentation of @spritestudio/ssplayer-wgpu (SpriteStudio Player for wgpu in the browser), ' +
  'for the version installed. Use when writing, reviewing or debugging code that plays ' +
  'SpriteStudio animations (.ssab, converted from .sspj) on WebGPU in a browser with ' +
  '@spritestudio/ssplayer-wgpu or its <ss-player-webgpu> element.';

const [dest, skill, ...rest] = process.argv.slice(2);
if (!dest || rest.length > 0) {
  console.error('usage: node scripts/copy-docs.mjs <dest> [<skill>]');
  process.exit(2);
}

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const src = join(root, 'docs', 'en');

const pages = readdirSync(src, { recursive: true })
  .filter((rel) => rel.endsWith('.md'))
  .filter((rel) => rel !== 'index.md' && !rel.startsWith(`development${sep}`))
  .sort();

function copyPages(out) {
  rmSync(out, { recursive: true, force: true });
  for (const rel of pages) {
    mkdirSync(dirname(join(out, rel)), { recursive: true });
    cpSync(join(src, rel), join(out, rel));
  }
}

copyPages(resolve(dest));
console.log(`   docs/ (${pages.length} pages)`);

if (skill) {
  const out = resolve(skill);
  const frontDoor = readFileSync(join(root, 'scripts', 'release-docs', 'npm-README.md'), 'utf8');
  copyPages(out);
  writeFileSync(
    join(out, 'SKILL.md'),
    `---\nname: ${basename(out)}\ndescription: ${JSON.stringify(SKILL_DESCRIPTION)}\n---\n\n${frontDoor}`,
  );
  console.log(`   skills/${basename(out)}/ (${pages.length} pages + SKILL.md)`);
}
