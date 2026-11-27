import type { CatchAutoState } from '../tauri/types/catch';

/**
 * When a failed link gets "Try in Nezzel browser" (#106): yt-dlp can't read the page by itself, but the page plays its video in a
 * browser, so the Nezzel browser window can catch the stream. Only for web pages (http/https links).
 */

/**
 * Failed-row codes that offer "Try in Nezzel browser" (audit 2026-11-26): the page itself may still play its video,
 * so the Nezzel browser can catch the stream. Never DRM (nothing can save it), removed/private/legal/unavailable pages, live
 * timing, format choices, connection or local errors: the browser would fail the same way, or the fix is elsewhere.
 */
export const CATCH_CODES = [
  // Not a site yt-dlp knows, or nothing to save on the page (the preview's `unsupported`, minus text/app posts).
  'urlNotSupported', 'urlUnsupported', 'unknownUrlType', 'noMediaFound', 'embedOnly',
  // The site's extractor broke; the page may still play.
  'unableToExtractIdOrInfo', 'unableToExtractInitialData',
  // A bot wall or a page-specific player: a real browser session gets through.
  'accessForbidden403', 'blockedGeneric',
  // Sign in inside the Nezzel browser, then play (an age gate is a sign-in wall too).
  'signInRequired', 'signInRequiredForBotDetection', 'membersOnly', 'videoPasswordRequired', 'ageRestricted',
  // Unclassified: the best candidate for a catch.
  'unknown',
] as const;

/** Preview reasons (quick bar and card) that offer the Nezzel browser: the reason's codes are mostly in CATCH_CODES. */
export const CATCH_REASONS = ['unsupported', 'login', 'unknown'] as const;

export function isPageLink(url: string | null | undefined): boolean {
  if (!url) return false;
  try {
    const { protocol } = new URL(url);
    return protocol === 'http:' || protocol === 'https:';
  } catch {
    return false;
  }
}

/** A failed list row: its last diagnostic code and its link. */
export function rowCanCatch(code: string | null | undefined, url: string | null | undefined): boolean {
  return (CATCH_CODES as readonly string[]).includes(code ?? '') && isPageLink(url);
}

/**
 * A ready result whose media lives on another site than the pasted page (the page only embeds it, e.g. a film
 * page with a YouTube trailer): the page to open in the Nezzel browser, or null. The download stays the main action.
 */
export function embedCatchPage(embeddedIn: string | null | undefined): string | null {
  return isPageLink(embeddedIn) ? embeddedIn! : null;
}

/** A failed read in the quick bar or card: the preview's reason word and the link. */
export function readCanCatch(reason: string | null | undefined, url: string | null | undefined): boolean {
  return (CATCH_REASONS as readonly string[]).includes(reason ?? '') && isPageLink(url);
}

/** What a row (or the quick bar / card) shows for an automatic catch (contract v2). */
export type AutoCatchKind = 'looking' | 'found' | 'note' | 'needsStep';
export type AutoCatchAction = 'cancel' | 'download' | 'options' | 'openPage';
export interface AutoCatchView {
  kind: AutoCatchKind;
  /** i18n key of the line; `sub` is the muted second part. */
  text: string;
  sub?: string;
  actions: AutoCatchAction[];
  /** The session runs beside a usable embedded result: a quiet second line, the embedded row stays. */
  beside: boolean;
}

/**
 * Phase → view. Nothing when the setting is off, there is no session, or it was cancelled (the row is what it was).
 * Beside an embedded result only looking and found show: a live, encrypted or stuck page stays silent, the embedded
 * row keeps its own "Open the page" link. needsStep never offers Retry: opening the page is the one step left.
 */
export function autoCatchView(state: CatchAutoState | null | undefined, enabled: boolean): AutoCatchView | null {
  if (!enabled || !state) return null;
  const { beside } = state;
  switch (state.phase) {
    case 'waiting':
      return { kind: 'looking', text: beside ? 'catch.auto.besideWaiting' : 'catch.auto.waiting', actions: ['cancel'], beside };
    case 'looking':
      return { kind: 'looking', text: beside ? 'catch.auto.besideLooking' : 'catch.auto.looking', actions: ['cancel'], beside };
    case 'found':
      return {
        kind: 'found',
        text: beside ? 'catch.auto.foundFull' : 'catch.auto.found',
        actions: (state.formats?.length ?? 0) > 1 ? ['download', 'options'] : ['download'],
        beside,
      };
    case 'live':
      return beside ? null : { kind: 'note', text: 'catch.live.title', sub: 'catch.live.sub', actions: [], beside };
    case 'encrypted':
      return beside ? null : { kind: 'note', text: 'catch.drm.title', actions: [], beside };
    case 'noVideo':
      return beside ? null : { kind: 'note', text: 'errors.runner.playlistEmpty.shortMessage', actions: [], beside };
    case 'needsStep':
      return beside ? null : { kind: 'needsStep', text: 'catch.row.hint', actions: ['openPage'], beside };
    case 'cancelled':
      return null;
  }
}
