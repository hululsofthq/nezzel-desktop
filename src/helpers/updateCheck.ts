import { MAX_SUMMARY_LINES } from './releaseNotes';
import type { UpdaterCheckPayload } from '../tauri/types/updater';

const text = (v: unknown) => (typeof v === 'string' && v.trim() ? v.trim() : undefined);
/** A manifest line longer than this is cut (on a whole character) with an ellipsis, so the card keeps its size. */
export const MAX_NOTE_CHARS = 140;

function capLine(line: string): string {
  if (line.length <= MAX_NOTE_CHARS) return line;
  const chars = typeof Intl.Segmenter === 'function'
    ? Array.from(new Intl.Segmenter(undefined, { granularity: 'grapheme' }).segment(line), s => s.segment)
    : Array.from(line);
  return chars.length <= MAX_NOTE_CHARS ? line : `${chars.slice(0, MAX_NOTE_CHARS - 1).join('').trimEnd()}…`;
}

/**
 * Up to 3 summary lines from the manifest `notes` (`{"en":[..],"ar":[..],"fr":[..]}`) in the user's language,
 * else English, else none (releases before 0.1.5 sent one plain English sentence).
 */
export function summaryLines(notes: string | undefined, locale: string): string[] {
  if (!notes) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(notes);
  } catch {
    return [];
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return [];
  const byLang = parsed as Record<string, unknown>;
  const clean = (v: unknown) => (Array.isArray(v) ? v.map(text).filter((l): l is string => !!l) : []);
  const own = clean(byLang[locale]);
  return (own.length ? own : clean(byLang.en)).slice(0, MAX_SUMMARY_LINES).map(capLine);
}

/**
 * Whether the background check may show the card. A skipped version stays hidden until a newer one
 * arrives; a manual check (About) shows it anyway because the user asked.
 */
export function cardAllowed(info: UpdaterCheckPayload | undefined, skippedVersion: string, manualCheck: boolean): boolean {
  if (!info?.available || info.channel) return false;
  if (manualCheck) return true;
  return !info.version || info.version !== skippedVersion;
}

/** The desktop download page for this OS on the Nezzel site, in the user's language (Arabic is the root). */
export function downloadPageUrl(platform: string, locale: string): string {
  const slug = ({ macos: 'mac', windows: 'windows', linux: 'linux' } as Record<string, string>)[platform];
  const prefix = locale === 'en' || locale === 'fr' ? `/${locale}` : '';
  return `https://nezzel.hululsoft.mr${prefix}/${slug ? `${slug}/` : ''}`;
}
