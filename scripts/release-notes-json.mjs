#!/usr/bin/env node
// Prints the `notes` string for latest.json: compact `{"en":[..],"ar":[..],"fr":[..]}` with the summary
// lines of one version from src/release-notes/notes.json. Exits 1 when the version or a language is missing/empty.
//   node scripts/release-notes-json.mjs <version> [notes.json]
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

export const LANGS = ['en', 'ar', 'fr'];

export function notesFor(all, version) {
  const entry = all?.[version];
  if (!entry || typeof entry !== 'object') throw new Error(`no release notes for version ${version}`);
  const out = {};
  for (const lang of LANGS) {
    const summary = entry[lang]?.summary;
    const lines = Array.isArray(summary) ? summary.filter(l => typeof l === 'string' && l.trim() !== '') : [];
    if (lines.length === 0) throw new Error(`release notes ${version}: empty or missing "${lang}" summary`);
    out[lang] = lines;
  }
  return JSON.stringify(out);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [version, file = new URL('../src/release-notes/notes.json', import.meta.url)] = process.argv.slice(2);
  try {
    if (!version) throw new Error('usage: release-notes-json.mjs <version> [notes.json]');
    process.stdout.write(notesFor(JSON.parse(readFileSync(file, 'utf8')), version));
  } catch (e) {
    console.error(`FAIL ${e.message}`);
    process.exit(1);
  }
}
