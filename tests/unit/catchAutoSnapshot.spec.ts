import { afterEach, expect, it, vi } from 'vitest';
import autoViewSource from '../../src-tauri/src/catch/auto_view.rs?raw';

const snapshotStart = 'const FINAL_SNAPSHOT: &str = r#"';
const start = autoViewSource.indexOf(snapshotStart) + snapshotStart.length;
const snapshotScript = autoViewSource.slice(start, autoViewSource.indexOf('"#;', start));

afterEach(() => {
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

it.each([
  ['ad iframe and store link', '<iframe width="300" height="250"></iframe><a href="/store">Get it on Google Play</a>', false],
  ['store badge', '<a href="/store" role="button" class="google-play-badge">Get it on Google Play</a>', false],
  ['article and navigation links', '<a href="/more">Lire la suite</a><a href="/watch">Watch</a>', false],
  ['reading time', '<button title="Lecture : 5 min">Info</button>', false],
  ['cookie notice', '<aside class="cookie-requires-consent">Accept cookies</aside>', false],
  ['fullscreen embed', '<iframe allowfullscreen></iframe>', true],
  ['autoplay embed', '<iframe allow="encrypted-media; autoplay *"></iframe>', true],
  ['play control', '<button class="vjs-big-play-button" aria-label="Regarder la vidéo"></button>', true],
])('final snapshot checks for a player on a text page with %s', (_label, content, expected) => {
  document.body.innerHTML = `<article>News and analysis</article>${content}`;
  vi.spyOn(document, 'readyState', 'get').mockReturnValue('complete');
  for (const frame of document.querySelectorAll('iframe')) {
    vi.spyOn(frame, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 0, 300, 250));
  }
  const evaluate = new Function('window', 'document', `return ${snapshotScript}`);
  const raw = evaluate({ __nezzelCatch: { snapshot: () => '[]' } }, document);
  expect(JSON.parse(raw)[1]).toBe(expected);
});
