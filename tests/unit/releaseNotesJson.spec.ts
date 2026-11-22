import { spawnSync } from 'node:child_process';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

const script = join(process.cwd(), 'scripts/release-notes-json.mjs');
const lang = (lines: string[]) => ({ summary: lines, groups: [] });
const entry = { en: lang(['One', 'Two']), ar: lang(['واحد']), fr: lang(['Un']) };

function run(notes: unknown, version: string) {
  const file = join(mkdtempSync(join(tmpdir(), 'notes-')), 'notes.json');
  writeFileSync(file, JSON.stringify(notes));
  return spawnSync(process.execPath, [script, version, file], { encoding: 'utf8' });
}

describe('release-notes-json script', () => {
  it('prints the compact summary-only JSON for the version', () => {
    const r = run({ '0.2.0': entry, '0.1.9': { en: lang(['x']) } }, '0.2.0');
    expect(r.status).toBe(0);
    expect(r.stdout).toBe('{"en":["One","Two"],"ar":["واحد"],"fr":["Un"]}');
  });

  it('fails when the version has no entry', () => {
    const r = run({ '0.2.0': entry }, '0.3.0');
    expect(r.status).toBe(1);
    expect(r.stderr).toContain('0.3.0');
  });

  it('fails when a language is missing or has no summary lines', () => {
    expect(run({ '0.2.0': { en: entry.en, fr: entry.fr } }, '0.2.0').status).toBe(1);
    expect(run({ '0.2.0': { ...entry, fr: lang(['  ']) } }, '0.2.0').status).toBe(1);
  });
});
