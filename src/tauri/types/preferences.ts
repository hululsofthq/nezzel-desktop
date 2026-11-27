import { TrackType } from './media.ts';

export interface PathPreferences {
  audioDownloadDir: string | null;
  videoDownloadDir: string | null;
  videoDirectoryTemplate: string | null;
  audioDirectoryTemplate: string | null;
}

export const defaultPathPreferences: PathPreferences = {
  audioDownloadDir: null,
  videoDownloadDir: null,
  videoDirectoryTemplate: null,
  audioDirectoryTemplate: null,
};

export interface RecentPreferences {
  recent: Record<string, string[]>;
}

export const defaultRecentPreferences: RecentPreferences = {
  recent: {},
};

export interface FormatPreferences {
  trackType: TrackType;
}

export const defaultFormatPreferences: FormatPreferences = {
  trackType: TrackType.both,
};

/** One-time first-run surfaces. Saved preferences from before onboarding count as done (Rust side). */
export interface OnboardingPreferences {
  welcomeDone: boolean;
  closeNoteDone: boolean;
  /** One-time "reports are now on" notice for installs from before reports existed. */
  reportsNoticeDone?: boolean;
}

export type SearchEngine = 'duckduckgo' | 'google' | 'bing' | 'brave';

/** Name and query URL of each engine (brand names are not translated); the same on every OS. */
export const SEARCH_ENGINES: Record<SearchEngine, { name: string; url: string }> = {
  duckduckgo: { name: 'DuckDuckGo', url: 'https://duckduckgo.com/?q={q}' },
  google: { name: 'Google', url: 'https://www.google.com/search?q={q}' },
  bing: { name: 'Bing', url: 'https://www.bing.com/search?q={q}' },
  brave: { name: 'Brave Search', url: 'https://search.brave.com/search?q={q}' },
};

export interface Preferences {
  paths: PathPreferences;
  recents: RecentPreferences;
  formats: FormatPreferences;
  /** Absent until loaded from the backend. */
  onboarding?: OnboardingPreferences;
  searchEngine?: SearchEngine;
  searchHintShown?: boolean;
}

export const defaultPreferences: Preferences = {
  paths: defaultPathPreferences,
  recents: defaultRecentPreferences,
  formats: defaultFormatPreferences,
  searchEngine: 'duckduckgo',
  searchHintShown: false,
};
