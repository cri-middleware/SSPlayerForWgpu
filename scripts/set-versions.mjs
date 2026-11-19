#!/usr/bin/env node
// Set the version of one or more packages -- the decision a release makes by hand.
//
//   node scripts/set-versions.mjs ssplayer-wgpu@7.0.0-beta.2
//
// Each argument is <package>@<version>. A package's version is written in more
// than one file, and this changes that one field in each of them:
//
//   ssplayer-wgpu   Cargo.toml's [workspace.package] version, which ssplayer-wgpu
//                   and ssplayer-wgpu-ffi inherit, and the version of its
//                   [workspace.dependencies] entry; their two entries in
//                   Cargo.lock; packages/ssplayer-wgpu/package.json; the
//                   version the CDN samples in examples/cdn pin; and the
//                   player's own line in THIRD_PARTY_NOTICES.md.
//   ssplayer-bevy   libs/ssplayer-bevy/Cargo.toml's [package] version and its
//                   entry in Cargo.lock. The adapter is versioned on its own and
//                   released by hand, not by release.yml, which refuses it
//                   (RELEASING.md, "Releasing the Bevy adapter").
//
// It also turns the `## [Unreleased]` notes in the package's CHANGELOG.md into
// `## [<version>] - <date>` under a fresh, empty `## [Unreleased]`: the changelog
// is where the notes are, and a Release only links it.
//
// release.yml runs this with its `versions` input in every job, so each one
// builds the same tree; scripts/release-version.sh then prints the result.
// Every argument is checked before any file is written, and a version
// may not go backwards: one published by mistake can never be withdrawn. A
// version the changelog already has a heading for is released, and nothing under
// `## [Unreleased]` leaves a release with no notes. Naming the version a package
// already has cuts its first release.

import { readFileSync, writeFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const APP = 'set-versions';
const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');

const escape = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

// Each edit takes a file's text and the version it must currently say, and
// returns the text with that one field changed, or throws.
const tomlSection = (section) => (text, from, to) => {
  const lines = text.split(/(?<=\n)/);
  let here = false;
  let hits = 0;
  const out = lines.map((line) => {
    if (/^\s*\[/.test(line)) {
      here = line.replace(/\s/g, '') === `[${section}]`;
      return line;
    }
    const m = here && /^(\s*version\s*=\s*")([^"]*)(".*)$/s.exec(line);
    if (!m) return line;
    hits += 1;
    if (m[2] !== from) throw new Error(`[${section}] version is '${m[2]}', not '${from}'`);
    return `${m[1]}${to}${m[3]}`;
  });
  if (hits !== 1) throw new Error(`[${section}] has ${hits} version lines`);
  return out.join('');
};

// The `version` of the one-line `name = { ... }` entry in [section].
const tomlDependency = (section, name) => (text, from, to) => {
  const entry = new RegExp(`^(\\s*${escape(name)}\\s*=\\s*\\{.*\\bversion\\s*=\\s*")([^"]*)(".*)$`, 's');
  const lines = text.split(/(?<=\n)/);
  let here = false;
  let hits = 0;
  const out = lines.map((line) => {
    if (/^\s*\[/.test(line)) {
      here = line.replace(/\s/g, '') === `[${section}]`;
      return line;
    }
    const m = here && entry.exec(line);
    if (!m) return line;
    hits += 1;
    if (m[2] !== from) throw new Error(`[${section}] ${name} is '${m[2]}', not '${from}'`);
    return `${m[1]}${to}${m[3]}`;
  });
  if (hits !== 1) throw new Error(`[${section}] has ${hits} ${name} entries with a version`);
  return out.join('');
};

// A file is read once and written once, so the edits of one file run in turn here.
const inTurn = (...edits) => (text, from, to) => edits.reduce((next, edit) => edit(next, from, to), text);

const lockEntries = (crates) => (text, from, to) => {
  let next = text;
  for (const crate of crates) {
    const re = new RegExp(`(\\[\\[package\\]\\]\\r?\\nname = "${escape(crate)}"\\r?\\nversion = ")([^"]*)(")`, 'g');
    const found = [...next.matchAll(re)];
    if (found.length !== 1) throw new Error(`${found.length} entries for ${crate}`);
    if (found[0][2] !== from) throw new Error(`${crate} is '${found[0][2]}', not '${from}'`);
    next = next.replace(re, `$1${to}$3`);
  }
  return next;
};

// The CDN samples pin the package by its exact version, in every URL they load.
const cdnPins = (text, from, to) => {
  const pin = '@spritestudio/ssplayer-wgpu@';
  if (!text.includes(`${pin}${from}`)) throw new Error(`no ${pin}${from} pin`);
  return text.split(`${pin}${from}`).join(`${pin}${to}`);
};

// cargo-about lists the player among the crates it covers, under its version.
const noticesEntry = (text, from, to) => {
  const entry = (v) => `- **ssplayer-wgpu ${v}**`;
  if (text.split(entry(from)).length !== 2) throw new Error(`no single '${entry(from)}' line`);
  return text.replace(entry(from), entry(to));
};

const jsonVersion = (text, from, to) => {
  const next = text.replace(new RegExp(`("version"\\s*:\\s*")${escape(from)}(")`), `$1${to}$2`);
  if (JSON.stringify({ ...JSON.parse(text), version: to }) !== JSON.stringify(JSON.parse(next))) {
    throw new Error(`"version" could not be changed alone`);
  }
  return next;
};

const PACKAGES = {
  'ssplayer-wgpu': {
    current: () => /^\s*version\s*=\s*"([^"]*)"/m.exec(
      readFileSync(join(ROOT, 'Cargo.toml'), 'utf8').split(/^\s*\[workspace\.package\]\s*$/m)[1] ?? '',
    )?.[1] ?? '',
    files: [
      ['Cargo.toml', inTurn(tomlSection('workspace.package'), tomlDependency('workspace.dependencies', 'ssplayer-wgpu'))],
      ['Cargo.lock', lockEntries(['ssplayer-wgpu', 'ssplayer-wgpu-ffi'])],
      ['packages/ssplayer-wgpu/package.json', jsonVersion],
      ['examples/cdn/element.html', cdnPins],
      ['examples/cdn/api.html', cdnPins],
      ['examples/cdn/README.md', cdnPins],
      ['THIRD_PARTY_NOTICES.md', noticesEntry],
    ],
    changelog: 'libs/ssplayer-wgpu/CHANGELOG.md',
  },
  'ssplayer-bevy': {
    current: () => /^\s*version\s*=\s*"([^"]*)"/m.exec(
      readFileSync(join(ROOT, 'libs/ssplayer-bevy/Cargo.toml'), 'utf8').split(/^\s*\[package\]\s*$/m)[1] ?? '',
    )?.[1] ?? '',
    files: [
      ['libs/ssplayer-bevy/Cargo.toml', tomlSection('package')],
      ['Cargo.lock', lockEntries(['ssplayer-bevy'])],
    ],
    changelog: 'libs/ssplayer-bevy/CHANGELOG.md',
  },
};

function parse(version) {
  const m = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/.exec(version);
  if (!m) return null;
  return { core: [Number(m[1]), Number(m[2]), Number(m[3])], pre: m[4] ? m[4].split('.') : [] };
}

// SemVer precedence: the core numerically, then a version with a prerelease part
// below the same version without one, then the prerelease identifiers in turn.
function compare(a, b) {
  for (let i = 0; i < 3; i++) {
    if (a.core[i] !== b.core[i]) return a.core[i] < b.core[i] ? -1 : 1;
  }
  if (a.pre.length === 0 || b.pre.length === 0) {
    return a.pre.length === b.pre.length ? 0 : a.pre.length === 0 ? 1 : -1;
  }
  for (let i = 0; i < Math.max(a.pre.length, b.pre.length); i++) {
    const x = a.pre[i];
    const y = b.pre[i];
    if (x === undefined) return -1;
    if (y === undefined) return 1;
    const xn = /^\d+$/.test(x);
    const yn = /^\d+$/.test(y);
    if (xn && yn) {
      if (Number(x) !== Number(y)) return Number(x) < Number(y) ? -1 : 1;
    } else if (xn !== yn) {
      return xn ? -1 : 1;
    } else if (x !== y) {
      return x < y ? -1 : 1;
    }
  }
  return 0;
}

const args = process.argv.slice(2);
if (args.length === 0) {
  console.error(`${APP}: name at least one <package>@<version>, e.g. ssplayer-wgpu@7.0.0-beta.2`);
  process.exit(2);
}

// Dates the `## [Unreleased]` notes as `version`'s section, or throws.
const datedChangelog = (text, version, today) => {
  const lines = text.split('\n');
  if (lines.some((l) => l.startsWith(`## [${version}]`))) {
    throw new Error(`${version} is already released -- its CHANGELOG.md has a heading for it`);
  }
  const top = lines.findIndex((l) => l.trimEnd() === '## [Unreleased]');
  if (top < 0) throw new Error(`its CHANGELOG.md has no '## [Unreleased]' heading`);
  const end = lines.findIndex((l, i) => i > top && l.startsWith('## '));
  const notes = lines.slice(top + 1, end < 0 ? lines.length : end);
  if (!notes.some((l) => l.trim() !== '' && !l.startsWith('#'))) {
    throw new Error(`nothing under '## [Unreleased]' in its CHANGELOG.md -- write what ${version} gains first`);
  }
  lines.splice(top, 1, '## [Unreleased]', '', `## [${version}] - ${today}`);
  return lines.join('\n');
};

const today = new Date().toISOString().slice(0, 10);
const writes = [];
const summary = [];
const errors = [];
const seen = new Set();
for (const arg of args) {
  const at = arg.lastIndexOf('@');
  const name = at > 0 ? arg.slice(0, at) : '';
  const version = at > 0 ? arg.slice(at + 1) : '';
  const want = parse(version);
  if (!name || !want) {
    errors.push(`${arg}: expected <package>@<x.y.z[-prerelease]>`);
    continue;
  }
  const pkg = PACKAGES[name];
  if (!pkg) {
    errors.push(`${name}: not a package this script sets (${Object.keys(PACKAGES).join(', ')})`);
    continue;
  }
  if (seen.has(name)) {
    errors.push(`${name}: named twice`);
    continue;
  }
  seen.add(name);
  const current = pkg.current();
  const have = parse(current);
  if (!have) {
    errors.push(`${name}: its current version '${current}' is not one this script can compare`);
    continue;
  }
  const order = compare(want, have);
  if (order < 0) {
    errors.push(`${name}: ${version} is lower than its current ${current}`);
    continue;
  }
  const logPath = join(ROOT, pkg.changelog);
  let log;
  try {
    log = datedChangelog(readFileSync(logPath, 'utf8'), version, today);
  } catch (e) {
    errors.push(`${name}: ${e.message}`);
    continue;
  }
  try {
    if (order > 0) {
      for (const [file, edit] of pkg.files) {
        const path = join(ROOT, file);
        writes.push([path, edit(readFileSync(path, 'utf8'), current, version)]);
      }
    }
    writes.push([logPath, log]);
    summary.push(order > 0 ? `${name}: ${current} -> ${version}` : `${name}: ${version}`);
  } catch (e) {
    errors.push(`${name}: ${e.message} -- set it by hand`);
  }
}

if (errors.length > 0) {
  for (const e of errors) console.error(`${APP}: ${e}`);
  console.error(`${APP}: nothing was written`);
  process.exit(1);
}

for (const [path, text] of writes) writeFileSync(path, text);
for (const line of summary) console.log(line);
