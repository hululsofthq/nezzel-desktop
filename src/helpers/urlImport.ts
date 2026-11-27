import { isValidUrl, normalizePastedUrl } from './url.ts';
import { i18n } from '../i18n';
import { ToastStyle } from '../stores/toast.ts';

export type ParsedUrlImport = {
  urls: string[];
  skipped: number;
};

export type UrlImportToast = {
  message: string;
  style: ToastStyle;
};

const URL_MATCHER = /https?:\/\/[^\s<>"'`]+/gi;
const LOCAL_FILE = /^(?:file:|[a-z]:[\\/]|\\\\|\/(?!\/)|(?:\.{1,2}|~)\/)/i;
const SCHEME = /^[a-z][a-z0-9+.-]*:/i;
const BARE_HOST = /^(?:[\p{L}\p{N}](?:[\p{L}\p{N}-]*[\p{L}\p{N}])?\.)+[\p{L}\p{N}](?:[\p{L}\p{N}-]*[\p{L}\p{N}])?\.?(?=[:/?#]|$)/u;

function unique(urls: string[]): string[] {
  return [...new Set(urls)];
}

function normalizeUrlCandidate(value: string): string {
  return normalizePastedUrl(value.trim().replace(/[),.;!?]+$/g, ''));
}

/** A single-line input strips newlines, so repair wrapped links before flattening the paste. */
export function flattenUrlInput(text: string): string {
  if (!/[\r\n]/.test(text)) return text;
  const parts = text.split(/([\s,]+)/);
  for (let i = 1; i < parts.length - 1; i += 2) {
    if (!/^(?:\r\n?|\n)$/.test(parts[i])) continue;
    const before = parts[i - 1];
    const after = parts[i + 1];
    if (!after || SCHEME.test(after) || isValidUrl(after)) continue;
    // A leading slash can be a URL path. Recognizable local paths are still separate items.
    if (LOCAL_FILE.test(after) && (!after.startsWith('/') || /^\/(?:Users|home|Volumes)\//.test(after))) continue;
    if ((!/^https?:\/\//i.test(before) && !before.includes('.')) || LOCAL_FILE.test(before)) continue;

    const joined = before + after;
    const hostPrefix = /^(?:https?:\/\/)?([^/:?#\\]+)$/i.exec(before)?.[1];
    // A lone label after www. can be a wrapped supported host, as in www.you + tube.com/watch.
    const splitHost = hostPrefix !== undefined && (
      (!BARE_HOST.test(hostPrefix) && BARE_HOST.test(hostPrefix + after) && !isValidUrl(before))
      || (/^www\.[^.]+$/i.test(hostPrefix) && !isValidUrl(hostPrefix) && isValidUrl(hostPrefix + after))
    );
    const continuation = /^(?:[/?#&=.]|%[\da-f]{2})/i.test(after) || /[=&?/-]$/.test(before);
    if (!splitHost && (BARE_HOST.test(after) || !continuation)) continue;
    if (!isValidUrl(joined)) continue;
    parts[i - 1] = parts[i] = '';
    parts[i + 1] = joined;
  }
  return parts.join('').replace(/[\r\n]+/g, ' ');
}

export type UrlSkipReason = 'invalid' | 'localFile' | 'unsupportedScheme';

export function parseUrlInputText(text: string): ParsedUrlImport & { skipReasons: UrlSkipReason[] } {
  const tokens = flattenUrlInput(text)
    .split(/[\s,]+/)
    .map(token => normalizePastedUrl(token))
    .filter(Boolean);

  const urls = new Set<string>();
  const skipReasons: UrlSkipReason[] = [];
  for (const token of tokens) {
    if (LOCAL_FILE.test(token)) skipReasons.push('localFile');
    else if (isValidUrl(token)) urls.add(token);
    else if (SCHEME.test(token) && !/^https?:|^[\w.-]+:\d+(?:[/?#]|$)/i.test(token)) skipReasons.push('unsupportedScheme');
    else skipReasons.push('invalid');
  }

  return { urls: [...urls], skipped: skipReasons.length, skipReasons };
}

export function parseUrlFileText(text: string): ParsedUrlImport {
  const matches = text.match(URL_MATCHER) ?? [];
  const urls = unique(
    matches
      .map(normalizeUrlCandidate)
      .filter(candidate => isValidUrl(candidate)),
  );

  const skipped = text
    .split(/[\s,]+/)
    .map(token => token.trim())
    .filter(token => token.startsWith('http://') || token.startsWith('https://'))
    .filter(token => !isValidUrl(normalizeUrlCandidate(token)))
    .length;

  return { urls, skipped };
}

export function isSupportedImportFile(file: File): boolean {
  const lowerName = file.name.toLowerCase();
  return lowerName.endsWith('.csv') || lowerName.endsWith('.txt');
}

export function mergeParsedUrlImports(results: ParsedUrlImport[]): ParsedUrlImport {
  return {
    urls: unique(results.flatMap(result => result.urls)),
    skipped: results.reduce((sum, result) => sum + result.skipped, 0),
  };
}

export function getUrlImportToast(result: ParsedUrlImport): UrlImportToast {
  const t = i18n.global.t;

  if (result.urls.length === 0) {
    return {
      message: t('common.urlImport.noneFound'),
      style: 'warning',
    };
  }

  if (result.skipped > 0) {
    return {
      message: t('common.urlImport.partial', { added: result.urls.length, skipped: result.skipped }),
      style: 'info',
    };
  }

  return {
    message: t('common.urlImport.added', result.urls.length),
    style: 'info',
  };
}

export function getUrlImportReadErrorToast(): UrlImportToast {
  return {
    message: i18n.global.t('common.urlImport.readError'),
    style: 'error',
  };
}

export function getUrlImportUnsupportedToast(): UrlImportToast {
  return {
    message: i18n.global.t('common.urlImport.unsupportedType'),
    style: 'warning',
  };
}

/** Files bigger than this get a "Reading…" toast first, so the wait after the dialog closes has feedback. */
export const BIG_IMPORT_BYTES = 256 * 1024;
