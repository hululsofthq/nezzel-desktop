import { describe, expect, it } from 'vitest';
import { clickWindowMs, popupAllowed, settleClick, siteOf, type ClickRecord } from '../../src-capture/popup-guard';

const page = 'https://movies.example/watch/1';
const now = 10_000;
const linkTo = (target: string | null, ago = 100, button = false): ClickRecord => ({ target, button, at: now - ago });

describe('popupAllowed', () => {
  const cases: [string, string, ClickRecord | null, 'element' | 'document' | 'none', boolean][] = [
    ['link click, same address', 'https://player.example/v/1', linkTo('https://player.example/v/1#t'), 'none', true],
    ['link click, same site as the link', 'https://www.player.example/go', linkTo('https://player.example/v/1'), 'document', true],
    ['document click opens an ad', 'https://ads.example/redirect', linkTo(null), 'document', false],
    ['link click, ad elsewhere', 'https://ads.example/redirect', linkTo('https://player.example/v/1'), 'document', false],
    ['same-site window.open, no click', '/gallery/2', null, 'none', true],
    ['same-site subdomain', 'https://cdn.movies.example/x', null, 'none', true],
    ['stale link click', 'https://player.example/v/1', linkTo('https://player.example/v/1', clickWindowMs + 1), 'none', false],
    ['blank pop-up from the document', 'about:blank', linkTo(null, 50, true), 'document', false],
    ['sign-in button, its own handler', 'https://accounts.example/o/auth', linkTo(null, 50, true), 'element', true],
    ['button, but a document-wide handler', 'https://ads.example/', linkTo(null, 50, true), 'document', false],
    ['two-part suffix keeps sites apart', 'https://ads.co.uk/', null, 'none', false],
  ];
  it.each(cases)('%s', (_name, requested, click, handler, expected) => {
    expect(popupAllowed(requested, page, click, now, handler, false)).toBe(expected);
  });

  it('allow once opens anything', () => {
    expect(popupAllowed('https://ads.example/', page, null, now, 'document', true)).toBe(true);
  });
});

describe('settleClick', () => {
  const press = linkTo('https://player.example/v/1', 50);
  const cases: [string, string, string][] = [
    ['unchanged link', 'https://player.example/v/1', 'https://player.example/v/1'],
    ['redirect on the page\'s own site', 'https://out.movies.example/?u=1', 'https://out.movies.example/?u=1'],
    ['redirect on the link\'s own site', 'https://go.player.example/v/1', 'https://go.player.example/v/1'],
    ['rewritten to an ad on another site', 'https://ads.example/hijack', 'https://player.example/v/1'],
  ];
  it.each(cases)('%s', (_name, after, kept) => {
    expect(settleClick(press, linkTo(after, 10), page).target).toBe(kept);
  });

  it('a press too long before the click is not compared', () => {
    const old = linkTo('https://player.example/v/1', clickWindowMs + 50);
    expect(settleClick(old, linkTo('https://ads.example/x', 10), page).target).toBe('https://ads.example/x');
  });
});

describe('siteOf', () => {
  it.each([
    ['a.b.example.com', 'example.com'],
    ['www.bbc.co.uk', 'bbc.co.uk'],
    ['example.com.', 'example.com'],
    ['127.0.0.1', '127.0.0.1'],
  ])('%s → %s', (host, site) => expect(siteOf(host)).toBe(site));
});
