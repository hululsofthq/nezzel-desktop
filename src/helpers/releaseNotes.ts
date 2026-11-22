import bundled from '../release-notes/notes.json';

/** One version's notes in one language: up to 3 summary lines (update card), then lines grouped by site. */
export interface VersionNotes {
  summary: string[];
  groups: [group: string, lines: string[]][];
}
export type ReleaseNotesFile = Record<string, Record<string, VersionNotes>>;

export const RELEASE_NOTES = bundled as unknown as ReleaseNotesFile;
export const NOTES_LANGUAGES = ['en', 'ar', 'fr'] as const;
export const MAX_NOTE_LINES = 6;
export const MAX_SUMMARY_LINES = 3;

/** Numeric dotted compare ("0.1.10" > "0.1.9"); a pre-release suffix is ignored. */
export function compareVersions(a: string, b: string): number {
  const parts = (v: string) => v.split(/[-+]/)[0].split('.').map(n => Number.parseInt(n, 10) || 0);
  const pa = parts(a), pb = parts(b);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d) return d;
  }
  return 0;
}

/** Versions that have notes, newest first, none newer than the running one. */
export function noteVersions(current: string, file: ReleaseNotesFile = RELEASE_NOTES): string[] {
  return Object.keys(file)
    .filter(v => !current || compareVersions(v, current) <= 0)
    .sort((a, b) => compareVersions(b, a));
}

/** Notes in the user's language, else English, else nothing. */
export function notesFor(version: string, locale: string, file: ReleaseNotesFile = RELEASE_NOTES): VersionNotes | undefined {
  const entry = file[version];
  return entry?.[locale] ?? entry?.en;
}

/**
 * The quiet "Updated to X" line after an update. `record` = the version to store as last seen now.
 * Nothing recorded yet: a user who had completed the welcome before this run is updating from a version that
 * predates this line (show it); otherwise it is a fresh install (just remember the version). `welcomeDoneBefore`
 * is undefined until preferences load: decide nothing yet.
 */
export function afterUpdateLine(current: string, lastSeen: string, welcomeDoneBefore?: boolean): { show: boolean; record?: string } {
  if (!current) return { show: false };
  if (lastSeen) return { show: lastSeen !== current };
  if (welcomeDoneBefore === undefined) return { show: false };
  return welcomeDoneBefore ? { show: true } : { show: false, record: current };
}
