/** "Catch from browser" (#106): the Rust ↔ bar contract, docs/catch/contract.md (v1). */

export type CatchPhase = 'looking' | 'adPlaying' | 'found' | 'drm' | 'live' | 'notFound' | 'signIn' | 'saving' | 'crashed' | 'geoBlocked' | 'loadFailed';

export interface CatchCandidate {
  id: string;
  kind: 'hls' | 'dash' | 'direct';
  title?: string;
  durationSec?: number;
  width?: number;
  height?: number;
  sizeBytes?: number;
  audioOnly?: boolean;
  alreadySaved?: boolean;
  /** false = "Other videos on this page" (short clips and ads, < 60 s). */
  main: boolean;
  /** Main videos: a small JPEG `data:` URL Rust fetched through an admitted request. */
  thumbnail?: string;
}

export interface CatchState {
  windowId: string;
  url: string;
  host: string;
  title?: string;
  canBack: boolean;
  canForward: boolean;
  loading: boolean;
  phase: CatchPhase;
  popupsBlocked: number;
  /** Ad blocking is on for this page's site (absent from an older engine: on). */
  adblock?: boolean;
  /** Requests the ad blocker stopped on this page. */
  adsBlocked?: number;
  played: boolean;
  playedSec: number;
  candidates: CatchCandidate[];
  savedDownloadId?: string;
  firstRun: boolean;
  /** v3 (tabs): every field above describes the active tab. Absent from a v1 engine: one tab, the window itself. */
  tabs?: CatchTab[];
  activeTabId?: string;
  /** The page shows a sign-in wall and a browser profile is signed in to this site: offer the browser's sign-in. */
  signinHint?: 'offer';
  /** The site that offer is for (one per sign-in, whatever the host): sent back with the import. */
  signinSite?: string;
}

/** One tab of a Nezzel browser window (contract v3). `url: ''` is the home page. */
export interface CatchTab {
  id: string;
  url: string;
  host: string;
  title?: string;
  loading: boolean;
  phase: CatchPhase;
  /** A main candidate exists: the tab's dot. */
  found: boolean;
  private: boolean;
  /** Opened by this tab as a pop-up the user caused: listed under it. */
  openerId?: string;
  asleep: boolean;
  mobile: boolean;
}

/** `catch_home`: what the home page lists. Recent sites stay on this computer. */
export interface CatchHome {
  recentSites: { host: string; title?: string }[];
  saved: { downloadId: string; title: string; host: string; durationSec?: number; sizeBytes?: number; height?: number }[];
}

/** Errors `catch_use_browser_signin` rejects with. */
export type CatchSigninError = 'catchSigninNone' | 'catchSigninDenied' | 'catchSigninUnsupported';

export interface CatchFormat {
  id: string;
  label: 'video' | 'audio';
  height?: number;
  ext: string;
  sizeBytes?: number;
  best: boolean;
}

export type CatchNavigation = 'back' | 'forward' | 'reload' | { url: string };

/** Event Rust emits to the bar webview on every change. */
export const CATCH_STATE_EVENT = 'catch://state';

/** The queue publishes only row display fields. The captured request stays in Rust. */
export interface CatchQueuedDownload {
  id: string;
  url: string;
  title: string | null;
  kind: 'audio' | 'video' | 'file';
  height: number | null;
  sizeBytes?: number | null;
  /** The caught video's picture (`data:` URL), when one was found. */
  thumbnail?: string | null;
}

/** Automatic catch (contract v2): a hidden session per download group, never shown. */
export type CatchAutoPhase = 'waiting' | 'looking' | 'found' | 'live' | 'encrypted' | 'noVideo' | 'needsStep' | 'cancelled';

export interface CatchAutoState {
  groupId: string;
  phase: CatchAutoPhase;
  /** true when the group already has a usable embedded result and this session runs beside it. */
  beside: boolean;
  /** Set from `found` / `live`. */
  title?: string;
  durationSec?: number;
  /** data: or asset URL fetched through an admitted request. */
  thumbnail?: string;
  kind?: 'hls' | 'dash' | 'direct';
  formats?: CatchFormat[];
}

/** Event Rust emits to the main window on every automatic-catch change. */
/** The tabs engine asks the shell to act on a key pressed inside a page. */
export const CATCH_SHORTCUT_EVENT = 'catch://shortcut';
export const CATCH_FILE_DOWNLOAD_EVENT = 'catch://file-download';
export type CatchFileDownload = { kind: 'opened' | 'saving' | 'failed'; name: string | null } | { kind: 'private' } | { kind: 'proxied' } | { kind: 'unsupported' };
/** macOS File menu > Open Nezzel Browser (item or ⌘⇧B), sent to the main window, which opens it (`menu.rs`). */
export const MENU_OPEN_BROWSER = 'menu-open-browser';
export const CATCH_AUTO_EVENT = 'catch://auto';
