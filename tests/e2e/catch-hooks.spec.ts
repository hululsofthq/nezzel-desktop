import { test as base, expect, type Page } from '@playwright/test';
import { once } from 'node:events';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import type { CaptureEntry } from '../../src-capture/catch-hooks';

const hooksPath = new URL('../../src-tauri/src/catch/hooks.js', import.meta.url);
const playlist = '#EXTM3U\n#EXT-X-ENDLIST\n';
const signedPath = '/nested.mp4?token=fixture-secret';
const framePlay = `<button aria-label="Play video" style="width:100px;height:60px"></button><script>
  window.__fixturePlayed = [];
  document.querySelector('button').onclick = () => {
    window.__fixturePlayed.push('play');
    const video = document.createElement('video');
    video.src = '/frame-press.mp4';
    document.body.append(video);
  };
</script>`;

function silentWave() {
  const rate = 8000;
  const bytes = rate * 2 * 2;
  const wave = Buffer.alloc(44 + bytes);
  wave.write('RIFF', 0);
  wave.writeUInt32LE(wave.length - 8, 4);
  wave.write('WAVEfmt ', 8);
  wave.writeUInt32LE(16, 16);
  wave.writeUInt16LE(1, 20);
  wave.writeUInt16LE(1, 22);
  wave.writeUInt32LE(rate, 24);
  wave.writeUInt32LE(rate * 2, 28);
  wave.writeUInt16LE(2, 32);
  wave.writeUInt16LE(16, 34);
  wave.write('data', 36);
  wave.writeUInt32LE(bytes, 40);
  return wave;
}

declare global {
  interface Window {
    __fixtureNativeWorker: typeof Worker;
    __fixtureBlobBlocked: boolean;
    __fixtureWindowMessages: unknown[];
    __fixturePlayed: string[];
    __fixtureLoaded: boolean;
    __fixtureVisibility?: string;
    __fixtureRelayWrites: { index: number; probe: number };
  }
}

const workerSource = `
  importScripts('helper.js');
  async function load(reply) {
    const response = await fetch(new Request('relative.m3u8'));
    reply(await response.text());
  }
  if ('onconnect' in self) {
    let connections = 0;
    self.onconnect = ({ ports: [port] }) => {
      port.postMessage(++connections);
      load(text => port.postMessage(text));
    };
  } else {
    load(text => self.postMessage(text));
  }
`;

async function startFixtureServer(pages: Record<string, string>) {
  const server = createServer((request, response) => {
    const path = new URL(request.url ?? '/', 'http://localhost').pathname;
    if (path in pages) {
      response.setHeader('content-type', 'text/html');
      if (path === '/legal') response.statusCode = 451;
      if (path === '/worker-csp') response.setHeader('content-security-policy', 'worker-src \'self\'');
      response.end(pages[path]);
    } else if (path === '/mse.m3u8') {
      response.writeHead(302, { location: '/playlist.js?token=fixture' }).end();
    } else if (path === '/playlist.js') {
      response.writeHead(200, { 'content-type': 'application/javascript' }).end(playlist);
    } else if (path.startsWith('/scripts/') && path.endsWith('.js')) {
      const scripts: Record<string, string> = {
        '/scripts/worker.js': workerSource,
        '/scripts/helper.js': 'self.fixtureLoaded = true;',
        '/scripts/location.js': 'postMessage(new URL("data.bin", self.location.href).href);',
      };
      response.writeHead(200, { 'content-type': 'text/javascript' }).end(scripts[path]);
    } else if (path === '/unavailable.m3u8') {
      response.writeHead(404, { 'content-type': 'application/vnd.apple.mpegurl' }).end(playlist);
    } else if (path.endsWith('.m3u8') || path === '/manifest') {
      response.writeHead(200, {
        'content-type': 'application/vnd.apple.mpegurl',
        'access-control-allow-origin': '*',
      }).end(playlist);
    } else if (path === '/stall.mp4' || path === '/stall.jpg') {
      // Still loading: an element given this source can be played (fixture media never decodes),
      // and a page holding the picture never finishes its load.
      response.writeHead(200, { 'content-type': path.endsWith('.jpg') ? 'image/jpeg' : 'video/mp4' });
    } else if (path.endsWith('.mp4')) {
      response.writeHead(200, { 'content-type': 'video/mp4' }).end('fixture');
    } else {
      response.writeHead(404).end();
    }
  });

  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('Missing fixture port');
  return { origin: `http://127.0.0.1:${address.port}`, close: () => {
    server.closeAllConnections();
    server.close();
  } };
}

const workerFixture = `<!doctype html><button>Start worker</button><script>
  document.querySelector('button').onclick = () => {
    const blob = location.pathname === '/worker-revoke';
    const url = blob
      ? URL.createObjectURL(new Blob(['postMessage("blob-ok")'], { type: 'text/javascript' }))
      : location.pathname === '/worker' ? '/scripts/worker.js' : '/scripts/location.js';
    const worker = new Worker(url);
    if (blob) URL.revokeObjectURL(url);
    worker.onmessage = ({ data }) => { document.body.dataset.workerBody = data; };
    worker.onerror = () => { document.body.dataset.workerBody = 'worker-error'; };
  };
</script>`;

const test = base.extend<{
  deep: boolean; auto: boolean; stubPlay: boolean; fakeClock: boolean; hiddenDocument: boolean;
}, { origins: { main: string; player: string } }>({
  deep: [false, { option: true }],
  // As the hidden page does: its switch runs just before the hooks and names the document.
  auto: [false, { option: true }],
  // play() records `id:muted:volume` instead of loading (fixture media does not decode). Set
  // before the hooks, which keep the play() they find.
  stubPlay: [false, { option: true }],
  // Fake timers from before the hooks run, so the timers they set at document start are fake too.
  fakeClock: [false, { option: true }],
  hiddenDocument: [false, { option: true }],
  origins: [async ({}, use) => {
    const player = await startFixtureServer({
      '/outer': `<script>window.__fixtureWindowMessages = [];
        addEventListener('message', event => window.__fixtureWindowMessages.push(event.data));
      </script><iframe src="/inner"></iframe>`,
      '/inner': `<video src="${signedPath}"></video><script>document.querySelector("video").dispatchEvent(new Event("playing"));</script>`,
      '/position-outer': '<!doctype html><iframe src="/position-player" style="position:absolute;top:47px;border:0"></iframe>',
      // The button stays above the player: GTK's taller button runs under its controls.
      '/position-player': `<!doctype html><button style="position:relative;z-index:1">Start audio</button>
        <audio controls style="display:block;position:absolute;top:23px"></audio><script>
          const audio = document.querySelector('audio');
          const samples = Uint8Array.from(atob('${silentWave().toString('base64')}'), c => c.charCodeAt(0));
          audio.src = URL.createObjectURL(new Blob([samples], { type: 'audio/wav' }));
          document.querySelector('button').onclick = () => audio.play();
        </script>`,
      '/relay-check': `<script>
        const mode = location.search === '?page-catch-globals' ? 'page-globals' : location.search ? 'keyless' : 'keyed';
        parent.postMessage(mode + ':' + typeof globalThis.__nezzelCatchRelayKey, '*');
      </script>`,
      '/opaque': '<video><source src="/opaque.mp4" type="video/mp4"></video>',
      '/frame-press': framePlay,
      '/nested-press': '<iframe src="/frame-press" style="width:320px;height:180px"></iframe>',
      '/pending-source': `<video src="/stall.mp4" preload="none"></video><script>
        Object.defineProperty(document.querySelector('video'), 'paused', { get: () => false });
      </script>`,
    });
    try {
      const main = await startFixtureServer({
        '/': '<!doctype html><main>Fixture</main>',
        '/mse': `<video></video><script>
          const video = document.querySelector('video');
          video.src = URL.createObjectURL(new MediaSource());
          const xhr = new XMLHttpRequest();
          xhr.open('GET', '/mse.m3u8');
          xhr.onload = () => { document.body.dataset.xhrBody = xhr.responseText; };
          xhr.send();
          fetch('/manifest').then(response => response.text()).then(text => {
            document.body.dataset.fetchBody = text;
          });
        </script>`,
        '/native': '<video src="/native.mp4"></video>',
        '/sorry': '<h1>We are sorry</h1>\n<p>This site is   not currently available in your area.</p>',
        '/long': `<p>${'word '.repeat(2000)}</p>`,
        '/legal': '<p>Unavailable</p><script>Object.defineProperty(PerformanceResourceTiming.prototype, "responseStatus", { get: () => 200 });</script>',
        // A server-rendered block page: little to read, a large data script beside it.
        '/ssr': `<p>Not available in your country</p><style>${'p{color:red}'.repeat(500)}</style><script type="application/json">${JSON.stringify({ props: 'x'.repeat(30000) })}</script>`,
        '/article': `<p>${'word '.repeat(400)}</p>`,
        '/framed': '<p>Welcome</p><iframe src="/sorry"></iframe>',
        '/pictures': '<meta property="og:image" content="/og.jpg"><link rel="image_src" href="/link.jpg"><video poster="/small.jpg" width="20" height="20"></video><video poster="poster.jpg?x=1" width="300" height="200"></video><video poster="javascript:void(0)" width="900" height="900"></video>',
        '/shadow': `<fixture-player></fixture-player><script>
          customElements.define('fixture-player', class extends HTMLElement {
            connectedCallback() {
              const outer = this.attachShadow({ mode: 'open' });
              outer.innerHTML = '<div></div>';
              const inner = outer.firstElementChild.attachShadow({ mode: 'closed' });
              inner.innerHTML = '<video src="/shadow.mp4"></video>';
              inner.querySelector('video').dispatchEvent(new Event('playing'));
            }
          });
        </script>`,
        '/autoplay': `<video id="small" src="/small.mp4" style="display:block;width:100px;height:50px"></video>
          <video id="main" src="/main.mp4" style="display:block;width:400px;height:300px"></video>
          <video id="unseen" src="/unseen.mp4" style="display:none;width:800px;height:600px"></video>
          <video id="invisible" src="/invisible.mp4" style="display:block;visibility:hidden;width:900px;height:600px"></video>
          <video id="empty" style="display:block;width:700px;height:400px"></video>
          <audio id="sound" src="/sound.mp4"></audio>`,
        // A click-to-play page: the player loads only when its own small play button is pressed.
        // Larger "play" controls beside it: an ad, a link away, a card holding a link, one under
        // a notice, a store badge, a playlist, and the player's own wrapper around the button.
        // The button holds stacked icons marked as buttons too, the labelled one covered (as on
        // Al Jazeera): the button is the control to press.
        // The page's utility class names an ad height, not an ad slot.
        '/click-to-play': `<div class="flex [--sticky-footer-ad-height:0px]" style="display:flex;flex-wrap:wrap;gap:4px">
          <div class="ad-banner"><button id="adplay" aria-label="Play" style="display:block;width:300px;height:150px"></button></div>
          <a id="trailer" href="https://films.invalid/trailer" aria-label="Play trailer" style="display:block;width:320px;height:150px"></a>
          <div id="card" role="button" aria-label="Play" style="width:320px;height:150px"><a id="cardlink" href="https://films.invalid/card" style="display:block;height:100%"></a></div>
          <div style="position:relative;width:340px;height:150px"><button id="covered" aria-label="Play" style="display:block;width:340px;height:150px"></button>
            <div style="position:absolute;inset:0;background:#fff">We use cookies</div></div>
          <button id="store" aria-label="Get it on Google Play" style="width:300px;height:100px"></button>
          <button id="playlist" aria-label="قائمة التشغيل" style="width:300px;height:100px"></button>
          <div id="player" role="button" tabindex="0" aria-label="Video Player" class="video-js vjs-big-play-centered video-play" style="position:relative;width:320px;height:180px">
            <button id="facade" class="vjs-big-play-button play-toggle vjs-paused" title="Play Video" style="position:absolute;left:130px;top:60px;width:60px;height:60px"><span class="vjs-icon-placeholder"></span>
              <svg role="button" aria-label="play video" style="position:absolute;inset:0;width:60px;height:60px"></svg><svg role="button" style="position:absolute;inset:0;width:60px;height:60px"></svg></button>
          </div>
          </div><audio id="silent"></audio><script>
          window.__fixturePlayed = [];
          for (const control of document.querySelectorAll('button, a, div[role="button"]')) {
            control.addEventListener('click', event => {
              event.preventDefault();
              event.stopPropagation();
              window.__fixturePlayed.push(control.id);
            });
          }
          document.getElementById('facade').addEventListener('click', () => {
            const video = document.createElement('video');
            video.addEventListener('play', () => window.__fixturePlayed.push('video:' + video.muted));
            video.src = '/stall.mp4';
            document.getElementById('player').append(video);
          });
          // An element played with no source plays nothing: it is no running player. The button
          // shows up late, while the page keeps changing (ads, lazy images) for 10 s.
          document.getElementById('silent').play().catch(() => {});
          const facade = document.getElementById('facade');
          facade.remove();
          setTimeout(() => document.getElementById('player').append(facade), 6500);
          let changes = 0;
          const busy = setInterval(() => {
            document.body.append(document.createElement('span'));
            if (++changes === 200) clearInterval(busy);
          }, 50);
        </script>`,
        '/playing': `<video id="main" src="/native.mp4" style="display:block;width:400px;height:300px"></video>
          <button id="play" aria-label="Play" style="display:block;width:80px;height:80px"></button><script>
          window.__fixturePlayed = [];
          Object.defineProperty(document.getElementById('main'), 'paused', { get: () => false });
          document.getElementById('play').addEventListener('click', () => window.__fixturePlayed.push('play'));
        </script>`,
        // A page that wires its play button once loaded, and whose load waits on a slow picture.
        // No player at all: a page of text with a frame of its own site.
        '/no-player': `<p>Nothing to watch here.</p><iframe src="/text" style="width:240px;height:100px"></iframe>`,
        '/text': `<p>More text.</p>`,
        // A player from a site the embed list does not know, before anything plays.
        '/other-embed': `<iframe src="${player.origin}/inner"></iframe>`,
        '/frame-press': framePlay,
        '/same-origin-press': `<iframe src="/frame-press" style="width:320px;height:180px"></iframe>
          <iframe src="/frame-press?second" style="width:320px;height:180px"></iframe>`,
        '/cross-origin-press': `<iframe src="${player.origin}/nested-press" style="width:420px;height:240px"></iframe>`,
        '/covered-frame-press': `<div style="position:relative;width:320px;height:180px">
          <iframe src="/frame-press" style="width:100%;height:100%"></iframe>
          <div style="position:absolute;inset:0;background:white"></div></div>`,
        '/relay-properties': `<script>
          const writes = window.__fixtureRelayWrites = { index: 0, probe: 0 };
          const replaced = () => { throw new Error('Page-owned Array method'); };
          Array.from = Array.prototype.filter = Array.prototype.some = Array.prototype[Symbol.iterator] = replaced;
          Object.defineProperty(Array.prototype, '0', { configurable: true, set(value) {
            if (value && typeof value === 'object' && Object.hasOwn(value, 'source')) writes.index++;
            Object.defineProperty(this, '0', { value, configurable: true, enumerable: true, writable: true });
          } });
          Object.defineProperty(Object.prototype, 'probe', { configurable: true, set(value) {
            writes.probe++;
            Object.defineProperty(this, 'probe', { value, configurable: true, enumerable: true, writable: true });
          } });
        </script><iframe src="/frame-press" style="width:320px;height:180px"></iframe>`,
        '/large-frame': '<iframe src="/text" style="width:320px;height:180px"></iframe>',
        '/below-fold': `<div style="height:1400px"></div>${framePlay}`,
        '/covered-below-fold': `<div style="height:1400px"></div><div style="position:relative;width:200px">
          ${framePlay}<div style="position:absolute;inset:0;background:white"></div>
        </div>`,
        '/play-anchor': `<a class="controls-play-pause-big" href="javascript:void(0)"
          style="display:block;width:100px;height:60px"></a><script>
          window.__fixturePlayed = [];
          const anchor = document.querySelector('a');
          if (location.search) anchor.setAttribute('href', '#');
          anchor.onclick = event => {
            if (event.defaultPrevented) return;
            event.stopPropagation();
            window.__fixturePlayed.push('play');
          };
        </script>`,
        '/watch-link': `<a href="/frame-press" style="display:block;width:100px;height:60px">Watch video</a>`,
        '/consent-wall': `<dialog aria-label="Cookie consent" style="width:400px;height:250px">
          <button aria-label="Accept cookies and play" style="width:180px;height:60px">Accept</button>
          <button>Reject optional cookies</button></dialog><script>
          window.__fixturePlayed = [];
          document.querySelector('dialog').showModal();
          document.querySelector('button').onclick = () => window.__fixturePlayed.push('consent');
        </script>`,
        // A cookie banner along the bottom beside the player (as on Al Jazeera), or over the
        // middle of the view.
        '/cookie-banner': `<button id="play" aria-label="Play video" style="display:block;width:100px;height:60px"></button>
          <div role="dialog" aria-label="Cookie banner" style="position:fixed;left:0;right:0;bottom:0;height:94px;background:#eee">
            We use cookies <button>Accept all cookies</button> <button>Reject all</button></div><script>
          window.__fixturePlayed = [];
          if (location.search) document.querySelector('[role=dialog]').style.cssText = 'position:fixed;inset:0;background:#eee';
          for (const button of document.querySelectorAll('button')) {
            button.onclick = () => window.__fixturePlayed.push(button.id || 'consent');
          }
        </script>`,
        // The clip the page's play button starts loops (Al Jazeera's video pages).
        '/loop-press': `<div class="hero"><video id="hero" loop muted style="display:block;width:200px;height:100px"></video></div>
          <div class="player" style="position:relative"><video id="clip" loop style="display:block;width:200px;height:100px"></video>
          <button aria-label="Play video" style="position:absolute;left:50px;top:20px;width:100px;height:60px"></button></div><script>
          window.__fixturePlayed = [];
          // A looping hero video elsewhere on the page stays a background loop after the press.
          const hero = document.getElementById('hero');
          const heroSamples = Uint8Array.from(atob('${silentWave().toString('base64')}'), c => c.charCodeAt(0));
          const startHero = () => {
            hero.src = URL.createObjectURL(new Blob([heroSamples], { type: 'audio/wav' }));
            hero.play().catch(() => {});
          };
          document.querySelector('button').onclick = () => {
            window.__fixturePlayed.push('play');
            const clip = document.getElementById('clip');
            const samples = Uint8Array.from(atob('${silentWave().toString('base64')}'), c => c.charCodeAt(0));
            clip.src = URL.createObjectURL(new Blob([samples], { type: 'audio/wav' }));
            clip.play().catch(() => {});
            startHero();
          };
        </script>`,
        '/visibility-player': `<script>
          window.__fixturePlayed = [];
          const animation = requestAnimationFrame(() => window.__fixturePlayed.push('cancelled'));
          cancelAnimationFrame(animation);
          requestAnimationFrame(() => {
            window.__fixtureVisibility = document.visibilityState + ':' + document.hidden;
            if (document.visibilityState !== 'visible' || document.hidden) return;
            const button = document.createElement('button');
            button.textContent = 'Play video';
            button.style.cssText = 'width:100px;height:60px';
            button.onclick = () => window.__fixturePlayed.push('play');
            document.body.append(button);
          });
        </script>`,
        '/late-load': `<button id="play" aria-label="Play" style="display:block;width:80px;height:80px"></button>
          <img src="/stall.jpg" alt=""><script>
          window.__fixturePlayed = [];
          setTimeout(() => document.getElementById('play').addEventListener('click', () => window.__fixturePlayed.push('play')), 12000);
          window.__fixtureLoaded = false;
          addEventListener('load', () => { window.__fixtureLoaded = true; });
        </script>`,
        // A player with its file set that does not start by itself (play() fails here: the
        // fixture file does not decode); its own button starts it.
        '/paused-src': `<video id="main" src="/paused.mp4" preload="auto" style="display:block;width:400px;height:300px"></video>
          <button id="play" aria-label="Play" style="display:block;width:80px;height:80px"></button><script>
          window.__fixturePlayed = [];
          document.getElementById('play').addEventListener('click', () => window.__fixturePlayed.push('play'));
        </script>`,
        // Players already starting, each beside a play button: an MSE player before its blob is
        // attached, a page that fetched its playlist, a player playing inside another site's frame.
        '/starting': `<video id="main" style="display:block;width:400px;height:300px"></video>
          <button id="play" aria-label="Play" style="display:block;width:80px;height:80px"></button><script>
          window.__fixturePlayed = [];
          document.getElementById('main').play().catch(() => {});
          document.getElementById('play').addEventListener('click', () => window.__fixturePlayed.push('play'));
        </script>`,
        '/fetching': `<button id="play" aria-label="Play" style="display:block;width:80px;height:80px"></button><script>
          window.__fixturePlayed = [];
          fetch('/starting.m3u8');
          document.getElementById('play').addEventListener('click', () => window.__fixturePlayed.push('play'));
        </script>`,
        '/framed-player': `<button id="play" aria-label="Play" style="display:block;width:80px;height:80px"></button>
          <iframe src="${player.origin}/inner" style="width:400px;height:200px"></iframe><script>
          window.__fixturePlayed = [];
          document.getElementById('play').addEventListener('click', () => window.__fixturePlayed.push('play'));
        </script>`,
        '/framed-pending-player': `<button id="play" aria-label="Play" style="display:block;width:80px;height:80px"></button>
          <iframe src="${player.origin}/pending-source" style="width:400px;height:200px"></iframe><script>
          window.__fixturePlayed = [];
          document.getElementById('play').addEventListener('click', () => window.__fixturePlayed.push('play'));
        </script>`,
        '/frames': `<script>window.__fixtureWindowMessages = [];
          addEventListener('message', event => window.__fixtureWindowMessages.push(event.data));
          </script><iframe src="${player.origin}/outer"></iframe>
          <iframe sandbox="allow-scripts" src="${player.origin}/opaque"></iframe>`,
        '/player-positions': `<!doctype html>
          <iframe src="${player.origin}/position-outer" style="position:absolute;top:101px;height:220px;border:0"></iframe>
          <div></div><script>
            const shadow = document.querySelector('div').attachShadow({ mode: 'open' });
            shadow.innerHTML = '<iframe src="${player.origin}/position-player?shadow" style="position:absolute;top:350px;border:0"></iframe>';
          </script>`,
        '/relay-observer': `<script>window.__fixtureWindowMessages = [];
          addEventListener('message', event => window.__fixtureWindowMessages.push(event.data));
          </script><iframe src="${player.origin}/relay-check"></iframe>
          <iframe src="${player.origin}/relay-check?no-relay-key"></iframe>`,
        '/worker': workerFixture,
        '/worker-csp': workerFixture,
        '/worker-location': workerFixture,
        '/worker-revoke': workerFixture,
        '/shared': '<iframe src="/"></iframe>',
        '/popups': `<a id="link" href="https://films.invalid/film" style="display:block;height:40px">Film</a>
          <a id="adlink" data-ad href="https://films.invalid/other" style="display:block;height:40px">Ad on a link</a>
          <div id="empty" style="height:200px">Empty</div><div id="host"></div>
          <a id="redirect" href="https://films.invalid/watch" style="display:block;height:40px">Redirected</a>
          <a id="hijack" href="https://films.invalid/hijacked" style="display:block;height:40px">Hijacked</a>
          <div id="router"><a id="routed" href="https://notes.invalid/megathread" target="_blank" rel="noopener nofollow ugc" style="display:block;height:40px">Notes</a></div><script>
          // Reddit in WKWebView (no Navigation API, so its polyfill routes clicks): the user's click
          // is cancelled and the link is clicked again by script, which opens the new window.
          document.getElementById('router').addEventListener('click', event => {
            event.stopPropagation();
            if (!event.isTrusted) return;
            event.preventDefault();
            event.target.closest('a').click();
          });
          // A site's link inside a custom element (shadow DOM).
          document.getElementById('host').attachShadow({ mode: 'open' }).innerHTML =
            '<a id="inner" href="https://shadow.invalid/film" style="display:block;height:40px">Shadow film</a>';
          // The moment the button goes down: one link goes through the page's own redirect, the
          // other is swapped for an ad on another site.
          document.getElementById('redirect').addEventListener('mousedown', event => {
            event.currentTarget.href = '/out?u=films';
          });
          document.getElementById('hijack').addEventListener('mousedown', event => {
            event.currentTarget.href = 'https://hijack.invalid/ad';
          });
          // One handler on the whole document opens every link itself, as Reddit-like sites do; on
          // nothing (or a link marked as an ad) it opens an ad, as pop-under scripts do.
          document.addEventListener('click', event => {
            event.preventDefault();
            const link = event.composedPath()[0].closest?.('a');
            window.open(link && !link.hasAttribute('data-ad') ? link.href : 'https://ads.invalid/ad');
          });
        </script>`,
      });
      try {
        await use({ main: main.origin, player: player.origin });
      } finally {
        main.close();
      }
    } finally {
      player.close();
    }
  }, { scope: 'worker' }],
});

async function drain(page: Page): Promise<CaptureEntry[]> {
  return page.evaluate(() => JSON.parse(window.__nezzelCatch?.drain() ?? '[]') as CaptureEntry[]);
}

function expectEntry(page: Page, ...expected: Partial<CaptureEntry>[]) {
  const received: CaptureEntry[] = [];
  return expect.poll(async () => {
    received.push(...await drain(page));
    return received;
  }).toEqual(expect.arrayContaining(expected.map(entry => expect.objectContaining(entry))));
}

test.beforeEach(async ({ page, deep, auto, stubPlay, fakeClock, hiddenDocument }) => {
  if (fakeClock) await page.clock.install();
  const hooks = await readFile(hooksPath, 'utf8');
  const setup = `
    (function(relayKey, autoplayDocument) {
      'use strict';
      ${hooks}
    })(['?no-relay-key', '?page-catch-globals'].includes(location.search)
      ? undefined : { document, key: [1n, 2n] }, ${auto ? 'document' : 'undefined'});
  `;
  await page.addInitScript({ content: `
    globalThis.__nezzelCatchDeep = ${deep};
    ${stubPlay
      ? `window.__fixturePlayed = [];
      HTMLMediaElement.prototype.play = function () {
        window.__fixturePlayed.push(this.id + ':' + this.muted + ':' + this.volume);
        return Promise.resolve();
      };`
      : ''}
    ${hiddenDocument
      ? `
      Object.defineProperty(Document.prototype, 'visibilityState', { configurable: true, get: () => 'hidden' });
      Object.defineProperty(Document.prototype, 'hidden', { configurable: true, get: () => true });
      window.requestAnimationFrame = () => 1;
      window.cancelAnimationFrame = () => {};
    `
      : ''}
    window.__fixtureNativeWorker = Worker;
    window.__fixtureBlobBlocked = false;
    addEventListener('securitypolicyviolation', event => {
      if (event.effectiveDirective === 'worker-src' && event.blockedURI.startsWith('blob')) {
        window.__fixtureBlobBlocked = true;
      }
    });
    if (typeof navigator.requestMediaKeySystemAccess !== 'function') {
      navigator.requestMediaKeySystemAccess = () => Promise.reject(new Error('Unsupported key system'));
    }
    if (typeof HTMLMediaElement.prototype.setMediaKeys !== 'function') {
      HTMLMediaElement.prototype.setMediaKeys = () => Promise.resolve();
    }
    if (!('WebKitMediaKeys' in window)) {
      Object.defineProperty(window, 'WebKitMediaKeys', { value: {} });
    }
    if (location.pathname !== '/relay-observer') {
      if (location.search === '?page-catch-globals') {
        globalThis.__nezzelCatchRelayKey = { document, key: [3n, 4n] };
        globalThis.__nezzelCatchAuto = document;
      }
      ${setup}
      ${setup}
    }
  ` });
});

test('captures MSE manifest metadata without consuming the player response', async ({ page, origins }) => {
  await page.goto(origins.main + '/mse');
  await expect(page.locator('body')).toHaveAttribute('data-xhr-body', playlist);
  await expect(page.locator('body')).toHaveAttribute('data-fetch-body', playlist);
  const entries = await drain(page);
  expect(entries).toEqual(expect.arrayContaining([
    expect.objectContaining({
      method: 'xhr', url: origins.main + '/playlist.js?token=fixture',
      requestedUrl: origins.main + '/mse.m3u8', ct: 'application/javascript', loaded: true,
    }),
    expect.objectContaining({ method: 'fetch', url: origins.main + '/manifest', ct: 'application/vnd.apple.mpegurl', loaded: true }),
  ]));
  expect(entries.filter(entry => entry.method === 'fetch')).toHaveLength(1);
  expect(entries.some(entry => entry.url?.startsWith('blob:'))).toBe(false);
  expect(entries.some(entry => entry.drm)).toBe(false);
});

test('filters failed fetch responses even when their body is a manifest', async ({ page, origins }) => {
  await page.goto(origins.main);
  const failed = await page.evaluate(async () => {
    const response = await fetch('/unavailable.m3u8');
    await (await fetch('/manifest')).text();
    return { status: response.status, body: await response.text() };
  });
  expect(failed).toEqual({ status: 404, body: playlist });
  const entries = await drain(page);
  expect(entries.filter(entry => entry.method === 'fetch').map(entry => entry.url))
    .toEqual([origins.main + '/manifest']);
  expect(entries.some(entry => entry.url?.endsWith('/unavailable.m3u8') && entry.loaded)).toBe(false);
});

test('credits real playback without crediting seeks, source swaps or loops', async ({ page, origins }) => {
  await page.goto(origins.main);
  const playback = page.evaluate((encoded) => {
    const once = (audio: HTMLAudioElement, name: string) =>
      new Promise<void>(resolve => audio.addEventListener(name, () => resolve(), { once: true }));
    const read = () => JSON.parse(window.__nezzelCatch!.drain()) as CaptureEntry[];
    const progress = (entries: CaptureEntry[]) => Math.max(0, ...entries.map(e => e.playedMediaSec ?? 0));
    const audio = document.createElement('audio');
    audio.controls = true;
    Object.defineProperty(audio, 'played', { get: () => ({ length: 0 }) });
    audio.muted = true;
    const start = document.createElement('button');
    start.textContent = 'Start playback fixture';
    document.body.append(audio, start);
    const wave = new Blob([Uint8Array.from(atob(encoded), char => char.charCodeAt(0))], { type: 'audio/wav' });
    const load = () => {
      const previous = audio.src;
      audio.src = URL.createObjectURL(wave);
      if (previous) URL.revokeObjectURL(previous);
      return audio.play();
    };
    const play = async () => {
      const naturalEnd = once(audio, 'ended');
      await load();
      await naturalEnd;
      const natural = progress(read().filter(e => e.url === null));

      await load();
      const seekEnd = once(audio, 'ended');
      audio.currentTime = audio.duration;
      await seekEnd;
      const seek = progress(read());

      await load();
      while (audio.currentTime < 0.5) await once(audio, 'timeupdate');
      const beforeSwap = progress(read());
      await load();
      const swapEnd = once(audio, 'ended');
      audio.currentTime = audio.duration;
      await swapEnd;
      const swap = read();

      audio.loop = true;
      await load();
      let previous = 0;
      for (;;) {
        await once(audio, 'timeupdate');
        if (audio.currentTime < previous) break;
        previous = audio.currentTime;
      }
      audio.pause();
      const loopEntries = read();
      return {
        natural, seek, beforeSwap,
        reset: swap.some(e => e.method === 'player.reset'),
        replacement: progress(swap),
        loop: progress(loopEntries),
        loopTops: loopEntries.filter(e => e.method === 'playing').map(e => e.playerTop),
      };
    };
    return new Promise<Awaited<ReturnType<typeof play>>>((resolve, reject) => {
      start.addEventListener('click', () => {
        void play().then(resolve, reject);
      }, { once: true });
    });
  }, silentWave().toString('base64'));
  const start = page.getByRole('button', { name: 'Start playback fixture' });
  const [result] = await Promise.all([playback, start.click()]);
  expect(result.natural).toBeGreaterThanOrEqual(1.5);
  expect(result.seek).toBeLessThan(0.5);
  expect(result.beforeSwap).toBeGreaterThan(0.25);
  expect(result.reset).toBe(true);
  expect(result.replacement).toBeLessThan(0.5);
  expect(result.loop).toBe(0);
  expect(result.loopTops.length).toBeGreaterThan(0);
  // Looping players report where they show; Rust weighs them by length (state.rs FILM_SEC).
  expect(result.loopTops.every(top => top !== undefined && top < 2 ** 31 - 1)).toBe(true);
});

test('scans native video and dynamic source elements', async ({ page, origins }) => {
  await page.goto(origins.main + '/native');
  await page.evaluate(() => {
    const video = document.createElement('video');
    video.innerHTML = '<source src="/dynamic.mp4" type="video/mp4">';
    document.body.append(video);
  });
  await expectEntry(page,
    { method: 'video.src', url: origins.main + '/native.mp4' },
    { method: 'video.currentSrc', url: origins.main + '/native.mp4' },
    { method: 'source.src', url: origins.main + '/dynamic.mp4', ct: 'video/mp4' });
});

test('finds and follows media inside open and closed shadow roots', async ({ page, origins }) => {
  await page.goto(origins.main + '/shadow');
  await expectEntry(page,
    { method: 'video.src', url: origins.main + '/shadow.mp4' },
    { method: 'playing', url: origins.main + '/shadow.mp4', playing: true });
});

test('snapshot keeps its shape after the page replaces JSON.stringify, slice and title', async ({ page, origins }) => {
  await page.goto(origins.main + '/native');
  await expect.poll(() => page.evaluate(() => {
    const stringify = JSON.stringify;
    const slice = String.prototype.slice;
    JSON.stringify = (() => undefined) as unknown as typeof JSON.stringify;
    String.prototype.slice = () => 'poisoned';
    const title = Object.getOwnPropertyDescriptor(Document.prototype, 'title')!;
    document.title = 'Fixture title';
    Object.defineProperty(Document.prototype, 'title', { configurable: true, get: () => 'poisoned' });
    try {
      return window.__nezzelCatch?.snapshot() ?? '';
    } finally {
      JSON.stringify = stringify;
      String.prototype.slice = slice;
      Object.defineProperty(Document.prototype, 'title', title);
    }
  }).then((raw) => {
    const [location, title, agent, hits] = JSON.parse(raw) as [string, string, string, CaptureEntry[]];
    expect(location).toBe(origins.main + '/native');
    expect(title).toBe('Fixture title');
    expect(agent).not.toBe('poisoned');
    return hits.map(hit => hit.url);
  })).toContain(origins.main + '/native.mp4');
});

test('snapshot names the largest video poster and the page picture, http(s) only', async ({ page, origins }) => {
  await page.goto(origins.main + '/pictures');
  const raw = await page.evaluate(() => window.__nezzelCatch?.snapshot() ?? '');
  const [, , , , pictures] = JSON.parse(raw) as [string, string, string, CaptureEntry[], [string, string]];
  expect(pictures).toEqual([origins.main + '/poster.jpg?x=1', origins.main + '/og.jpg']);
});

test('after an in-page address change the snapshot follows it and drops the stale page picture', async ({ page, origins }) => {
  await page.goto(origins.main + '/pictures');
  const snapshot = () => page.evaluate(() => JSON.parse(window.__nezzelCatch?.snapshot() ?? '[]') as [string, string, string, CaptureEntry[], [string, string]]);
  await page.evaluate(() => history.replaceState(null, '', '#comments'));
  expect((await snapshot())[4][1]).toBe(origins.main + '/og.jpg');
  // An empty fragment (a "#" control link) leaves location.hash empty: still the same page.
  await page.evaluate(() => history.replaceState(null, '', '#'));
  expect((await snapshot())[4][1]).toBe(origins.main + '/og.jpg');
  // A page that replaces the String global changes nothing about what the snapshot reads.
  const replaced = await page.evaluate(() => {
    const original = window.String;
    window.String = (() => 'x') as unknown as StringConstructor;
    try {
      return JSON.parse(window.__nezzelCatch?.snapshot() ?? '[]') as [string, string, string, unknown[], [string, string]];
    } finally {
      window.String = original;
    }
  });
  expect(replaced[4][1]).toBe(origins.main + '/og.jpg');
  // A single-page site opens the next video by history.pushState, keeping the first page's og:image.
  await page.evaluate(() => {
    history.pushState(null, '', '/shorts/next');
    document.title = 'Next video';
  });
  const [location, title, , , pictures] = await snapshot();
  expect(location).toBe(origins.main + '/shorts/next');
  expect(title).toBe('Next video');
  // The poster's relative address follows the new page address too, as the browser's own does.
  expect(pictures).toEqual([origins.main + '/shorts/poster.jpg?x=1', '']);
  await page.goBack();
  expect((await snapshot())[4][1]).toBe(origins.main + '/og.jpg');
});

test('snapshot sends a short page its status and text, a long page no text', async ({ page, origins, browserName }) => {
  // WebKit does not tell a document's HTTP status (no `responseStatus`): 0.
  const told = (status: number) => browserName === 'chromium' ? status : 0;
  const geo = async (path: string) => {
    await page.goto(origins.main + path);
    const raw = await page.evaluate(() => window.__nezzelCatch?.snapshot() ?? '');
    return (JSON.parse(raw) as unknown[])[5] as [number, string];
  };
  expect(await geo('/sorry')).toEqual([told(200), 'We are sorry This site is not currently available in your area.']);
  expect((await geo('/long'))[1]).toBe('');
  // An ordinary article (2,000 characters) is not a short page.
  expect((await geo('/article'))[1]).toBe('');
  // The status is the one the document answered with, whatever the page says later.
  expect(await geo('/legal')).toEqual([told(451), 'Unavailable']);
  // Scripts and styles are not page text: a short page beside a large data script is still short.
  expect((await geo('/ssr'))[1]).toBe('Not available in your country');
  // A frame's text is not the page's.
  await page.goto(origins.main + '/framed');
  await page.frameLocator('iframe').getByText('We are sorry').waitFor();
  const framed = await page.evaluate(() => window.__nezzelCatch?.snapshot() ?? '');
  expect((JSON.parse(framed) as unknown[])[5]).toEqual([told(200), 'Welcome']);
});

test('snapshot says whether the top page shows a password field, never its value', async ({ page, origins }) => {
  await page.goto(origins.main + '/');
  const flag = () => page.evaluate(() => (JSON.parse(window.__nezzelCatch?.snapshot() ?? '[]') as unknown[])[6]);
  expect(await flag()).toBe(false);
  await page.evaluate(() => {
    const hidden = document.createElement('input');
    hidden.type = 'password';
    hidden.style.display = 'none';
    document.body.append(hidden);
  });
  expect(await flag()).toBe(false);
  await page.evaluate(() => {
    const shown = document.createElement('input');
    shown.type = 'PASSWORD';
    shown.value = 'fixture-secret';
    document.body.append(shown);
  });
  expect(await flag()).toBe(true);
  expect(await page.evaluate(() => window.__nezzelCatch?.snapshot() ?? '')).not.toContain('fixture-secret');
});

test('a password field nobody can see or type into is not a sign-in wall', async ({ page, origins }) => {
  await page.goto(origins.main + '/');
  const flag = () => page.evaluate(() => (JSON.parse(window.__nezzelCatch?.snapshot() ?? '[]') as unknown[])[6]);
  const cases: Record<string, string> = {
    'off-screen': '<input type="password" style="position: fixed; left: -9999px">',
    'off-canvas drawer': '<div style="position: fixed; top: 0; left: 0; width: 300px; transform: translateX(-100%)"><input type="password"></div>',
    'closed modal (ancestor opacity 0)': '<div style="opacity: 0"><form><input type="password"></form></div>',
    'ancestor visibility hidden': '<div style="visibility: hidden"><input type="password"></div>',
    'clipped by a zero-size wrapper': '<div style="width: 0; height: 0; overflow: hidden"><input type="password"></div>',
    'screen-reader-only wrapper': '<div style="position: absolute; width: 1px; height: 1px; overflow: hidden"><input type="password"></div>',
    'disabled fieldset': '<fieldset disabled><input type="password"></fieldset>',
    'inert ancestor': '<div inert><input type="password"></div>',
    'below the fold': '<div style="height: 300vh"></div><input type="password">',
    'same-origin frame': '<iframe srcdoc="<input type=password>"></iframe>',
  };
  for (const [name, html] of Object.entries(cases)) {
    await page.locator('main').evaluate((main, markup) => main.innerHTML = markup, html);
    if (name.includes('frame')) await page.frameLocator('iframe').locator('input').waitFor();
    expect(await flag(), name).toBe(false);
  }
  // Login modals as sites build them: a container that does not clip them must not hide them.
  const modals: Record<string, string> = {
    'fixed modal over a scroll-locked page': '<style>html, body { overflow: hidden; height: 0 }</style><div style="position: fixed; top: 200px; left: 100px"><input type="password"></div>',
    'fixed modal inside a clipped header': '<header style="height: 56px; overflow: hidden"><div style="position: fixed; top: 200px; left: 100px"><input type="password"></div></header>',
    'absolute popover out of a clipped static bar': '<nav style="height: 40px; overflow: hidden"><div style="position: absolute; top: 200px; left: 100px"><input type="password"></div></nav>',
    'one clipped axis only': '<div style="height: 40px; overflow-x: clip"><div style="height: 200px"></div><input type="password"></div>',
  };
  for (const [name, html] of Object.entries(modals)) {
    await page.locator('main').evaluate((main, markup) => main.innerHTML = markup, html);
    expect(await flag(), name).toBe(true);
  }
  // A real login form in view still counts, inside a scrolling panel too.
  await page.locator('main').evaluate(main => main.innerHTML = '<div style="overflow: auto; height: 200px"><form><input type="password"></form></div>');
  expect(await flag()).toBe(true);
});

test('the snapshot answer cannot be replaced on the hook object, and the password check uses the DOM calls it kept', async ({ page, origins }) => {
  await page.goto(origins.main + '/');
  const flag = await page.evaluate(() => {
    'use strict';
    const catcher = window.__nezzelCatch!;
    try {
      catcher.snapshot = () => JSON.stringify([location.href, '', '', [], ['', ''], true]);
    } catch { /* frozen */ }
    // A page that fakes the DOM answers instead of showing a field.
    const fake = document.createElement('input');
    fake.type = 'password';
    Document.prototype.querySelectorAll = (() => [fake]) as unknown as typeof Document.prototype.querySelectorAll;
    Element.prototype.getBoundingClientRect = () => new DOMRect(10, 10, 100, 20);
    window.getComputedStyle = (() => ({ visibility: 'visible', display: 'block', opacity: '1', overflow: 'visible' })) as unknown as typeof getComputedStyle;
    return (JSON.parse(window.__nezzelCatch!.snapshot()) as unknown[])[6];
  });
  expect(flag).toBe(false);
});

test('captures buffered resource timing from before installation', async ({ context, origins }) => {
  const page = await context.newPage();
  await page.goto(origins.main + '/');
  const manifest = origins.main + '/buffered.m3u8';
  await page.evaluate(async (url) => {
    const response = await fetch(url);
    await response.text();
  }, manifest);
  await page.waitForFunction(url => performance.getEntriesByName(url).length > 0, manifest);
  const hooks = await readFile(hooksPath, 'utf8');
  await page.addScriptTag({ content: `(function(relayKey, autoplayDocument) {\n'use strict';\n${hooks}\n})(undefined, undefined);` });
  await expectEntry(page, { method: 'perf:fetch', url: manifest });
});

test('relays nested playback and opaque sandbox media to the top frame', async ({ page, origins }) => {
  await page.goto(origins.main + '/frames');
  const frame = origins.player + '/inner';
  await expectEntry(page,
    { url: origins.player + signedPath, frame, msgOrigin: origins.player },
    { playing: true, frame, msgOrigin: origins.player },
    { url: origins.player + '/opaque.mp4', frame: origins.player + '/opaque', msgOrigin: 'null' });
  expect(await page.frame({ url: frame })?.evaluate(() => window.__nezzelCatch)).toBeUndefined();
  expect(await page.evaluate(() => JSON.stringify(window.__fixtureWindowMessages))).not.toContain('fixture-secret');
  const parent = page.frame({ url: origins.player + '/outer' });
  expect(parent).not.toBeNull();
  expect(await parent?.evaluate(() => JSON.stringify(window.__fixtureWindowMessages))).not.toContain('fixture-secret');
});

test('relays nested player positions and marks unlocated frames offscreen', async ({ page, origins }) => {
  await page.goto(origins.main + '/player-positions');
  const nested = page.frameLocator('iframe[src$="/position-outer"]').frameLocator('iframe');
  await nested.locator('audio').evaluate((audio) => {
    audio.getBoundingClientRect = () => new DOMRect();
    const native = window.getComputedStyle.bind(window);
    window.getComputedStyle = (element, pseudo) => element === audio
      ? { visibility: 'hidden' } as CSSStyleDeclaration
      : native(element, pseudo);
  });
  await nested.getByRole('button', { name: 'Start audio' }).click();
  await page.frameLocator('iframe[src$="?shadow"]').getByRole('button', { name: 'Start audio' }).click();
  await expectEntry(page,
    { playing: true, frame: origins.player + '/position-player', playerTop: 101 + 47 + 23 },
    { playing: true, frame: origins.player + '/position-player?shadow', playerTop: 2 ** 31 - 1 });
});

test('starts the visible frame relay only with a document-start key', async ({ page, origins }) => {
  await page.goto(origins.main + '/relay-observer');
  const messages = () => page.evaluate(() => window.__fixtureWindowMessages);
  // Each child posts its marker after its hooks have run.
  await expect.poll(messages).toEqual(expect.arrayContaining(['keyed:undefined', 'keyless:undefined']));
  const hellos = (await messages()).filter(message => message === '__nezzelCatchHello'
    || (message && typeof message === 'object' && 'type' in message && message.type === '__nezzelCatchHello'));
  expect(hellos).toEqual([expect.objectContaining({ type: '__nezzelCatchHello', challenge: expect.any(BigInt) })]);
});

test('page globals set before the hooks enable neither relay nor autoplay', async ({ page, origins }) => {
  await page.goto(origins.main + '/relay-observer');
  const messages = () => page.evaluate(() => window.__fixtureWindowMessages);
  await expect.poll(messages).toContain('keyed:undefined');
  await page.evaluate((url) => {
    const frame = document.createElement('iframe');
    frame.src = url;
    document.body.append(frame);
  }, origins.player + '/relay-check?page-catch-globals');
  await expect.poll(messages).toContain('page-globals:object');
  const hellos = (await messages()).filter(message => message && typeof message === 'object'
    && 'type' in message && message.type === '__nezzelCatchHello');
  expect(hellos).toHaveLength(1);
  expect(await page.frame({ url: /\?page-catch-globals$/ })?.evaluate(() => {
    const audio = document.createElement('audio');
    audio.muted = false;
    return audio.muted;
  })).toBe(false);
});

test('ignores entries forged on the embedding window message bus', async ({ page, origins }) => {
  await page.goto(origins.main + '/frames');
  await expectEntry(page, { url: origins.player + signedPath });
  await page.evaluate(async (url) => {
    const leaked = window.__fixtureWindowMessages.find((message): message is { nonce: string } =>
      Boolean(message && typeof message === 'object' && 'nonce' in message));
    const data = {
      __nezzelCatchRelay: 'entry', nonce: leaked?.nonce,
      entry: { method: 'fetch', url, frame: location.href },
    };
    const source = document.querySelector('iframe')?.contentWindow;
    window.dispatchEvent(new MessageEvent('message', { data, origin: location.origin, source }));
    await new Promise<void>((resolve) => {
      window.addEventListener('message', function barrier(event) {
        if (event.data === 'fixture-barrier') {
          window.removeEventListener('message', barrier);
          resolve();
        }
      });
      window.postMessage(data, '*');
      window.postMessage('fixture-barrier', '*');
    });
  }, origins.main + '/forged.m3u8');
  expect((await drain(page)).map(entry => entry.url)).not.toContain(origins.main + '/forged.m3u8');
});

test('reports an EME capability probe without a DRM signal', async ({ page, origins }) => {
  await page.goto(origins.main + '/');
  expect(await drain(page)).toEqual([]);
  await page.evaluate(async () => {
    await navigator.requestMediaKeySystemAccess('invalid.nezzel.fixture', []).catch(() => {});
  });
  const entries = await drain(page);
  expect(entries).toContainEqual(expect.objectContaining({
    method: 'requestMediaKeySystemAccess', emeProbe: true, emeActive: false, drm: false,
  }));
  expect(entries.some(entry => entry.emeActive || entry.drm)).toBe(false);
});

test('reports the encrypted event per element, and attached keys only as a probe', async ({ page, origins }) => {
  await page.goto(origins.main + '/');
  // Players clear keys on detach: that is not encrypted playback.
  await page.evaluate(async () => {
    const video = document.createElement('video');
    document.body.append(video);
    await video.setMediaKeys(null).catch(() => {});
  });
  expect((await drain(page)).some(entry => entry.emeActive || entry.drm)).toBe(false);
  await page.evaluate(async () => {
    const video = document.createElement('video');
    document.body.append(video);
    await video.setMediaKeys({} as MediaKeys).catch(() => {});
    video.dispatchEvent(new Event('encrypted'));
  });
  const entries = await drain(page);
  // Keys attached to a video are no evidence (players attach them to clear video too).
  expect(entries).toContainEqual(expect.objectContaining({
    method: 'setMediaKeys', emeActive: false, emeProbe: true, drm: false,
  }));
  // The encrypted event names its element, so the verdict belongs to that video only.
  expect(entries).toContainEqual(expect.objectContaining({
    method: 'encrypted', emeActive: true, emeProbe: false, drm: true, mediaId: expect.any(Number),
  }));
});

async function startWorkerOnPage(page: Page, expected: string) {
  await page.getByRole('button', { name: 'Start worker' }).click();
  await expect(page.locator('body')).toHaveAttribute('data-worker-body', expected);
}

for (const deep of [false, true]) {
  test.describe(deep ? 'deep worker capture' : 'default worker behavior', () => {
    test.use({ deep });

    test('keeps workers native when CSP blocks blob startup', async ({ page, origins }) => {
      await page.goto(origins.main + '/worker-csp');
      if (deep) await page.waitForFunction(() => window.__fixtureBlobBlocked);
      expect(await page.evaluate(() => Worker === window.__fixtureNativeWorker)).toBe(true);
      await startWorkerOnPage(page, origins.main + '/scripts/data.bin');
    });

    test('supports revoking a page-created worker blob immediately', async ({ page, origins }) => {
      await page.goto(origins.main + '/worker-revoke');
      if (deep) await page.waitForFunction(() => Worker !== window.__fixtureNativeWorker);
      await startWorkerOnPage(page, 'blob-ok');
    });

    test('preserves the original worker URL through self.location', async ({ page, origins }) => {
      await page.goto(origins.main + '/worker-location');
      if (deep) await page.waitForFunction(() => Worker !== window.__fixtureNativeWorker);
      await startWorkerOnPage(page, origins.main + '/scripts/data.bin');
    });
  });
}

test.describe('deep worker capture', () => {
  test.use({ deep: true });

  test('captures classic worker manifests and preserves relative imports and requests', async ({ page, origins }) => {
    await page.goto(origins.main + '/worker');
    await page.waitForFunction(() => Worker !== window.__fixtureNativeWorker);
    await startWorkerOnPage(page, playlist);
    await expectEntry(page,
      { method: 'worker-fetch', url: origins.main + '/scripts/relative.m3u8', frame: origins.main + '/worker' });
  });

  test('keeps a named SharedWorker connected across frames', async ({ page, origins }) => {
    await page.goto(origins.main + '/shared');
    test.skip(!await page.evaluate(() => 'SharedWorker' in window), 'SharedWorker is unavailable');
    const frame = page.frame({ url: origins.main + '/' });
    if (!frame) throw new Error('Missing SharedWorker fixture frame');
    await Promise.all([
      page.waitForFunction(() => Worker !== window.__fixtureNativeWorker),
      frame.waitForFunction(() => Worker !== window.__fixtureNativeWorker),
    ]);
    const connect = () => {
      const worker = new SharedWorker('/scripts/worker.js', 'fixture');
      return new Promise<{ connection: number; manifest: string }>((resolve) => {
        let connection = 0;
        worker.port.onmessage = ({ data }) => {
          if (typeof data === 'number') connection = data;
          else if (typeof data === 'string') resolve({ connection, manifest: data });
        };
      });
    };
    const connections = await Promise.all([page.evaluate(connect), frame.evaluate(connect)]);
    expect(connections).toEqual(expect.arrayContaining([
      { connection: 1, manifest: playlist },
      { connection: 2, manifest: playlist },
    ]));
  });
});

test('rebinds mutation capture when a window receives a different document', async ({ page, origins }) => {
  await page.goto(origins.main + '/');
  const hooks = await readFile(hooksPath, 'utf8');
  const entries = await page.evaluate(async (source) => {
    const sameWindow = Object.assign(new EventTarget(), { top: null as EventTarget | null });
    sameWindow.top = sameWindow;
    const first = document.implementation.createHTMLDocument('initial');
    const second = document.implementation.createHTMLDocument('replacement');
    const inject = new Function('window', 'document', 'globalThis', 'relayKey', 'autoplayDocument', source);
    inject(sameWindow, first, sameWindow, undefined, undefined);
    inject(sameWindow, second, sameWindow, undefined, undefined);
    for (const [doc, file] of [[first, 'stale.mp4'], [second, 'replacement.mp4']] as const) {
      const video = doc.createElement('video');
      video.src = new URL(file, location.href).href;
      doc.body.append(video);
    }
    await new Promise(resolve => setTimeout(resolve, 0));
    const drain = (sameWindow as unknown as Window).__nezzelCatch?.drain;
    return JSON.parse(drain?.() ?? '[]') as CaptureEntry[];
  }, hooks);
  expect(entries.map(entry => entry.url)).toEqual([origins.main + '/replacement.mp4']);
});

test('drains the newest 500 entries and empties the queue', async ({ page, origins }) => {
  await page.goto(origins.main + '/');
  const { batch, empty } = await page.evaluate(() => {
    const video = document.createElement('video');
    document.body.append(video);
    for (let index = 0; index < 510; index++) {
      video.setAttribute('src', `/signal-${index}.mp4`);
      video.dispatchEvent(new Event('encrypted'));
    }
    const hooks = window.__nezzelCatch;
    return { batch: JSON.parse(hooks?.drain() ?? '[]') as CaptureEntry[], empty: hooks?.drain() };
  });
  expect(batch).toHaveLength(500);
  expect(batch.at(0)).toMatchObject({ url: origins.main + '/signal-10.mp4', drm: true });
  expect(batch.at(-1)).toMatchObject({ url: origins.main + '/signal-509.mp4' });
  expect(empty).toBe('[]');
});

test.describe('automatic catch', () => {
  test.use({ auto: true });

  test('keeps peer registration and probing off page prototype setters and Array methods', async ({ page, origins }) => {
    await page.goto(origins.main + '/relay-properties');
    const player = page.frame({ url: origins.main + '/frame-press' })!;
    // The player frame keeps its native Array methods, including for Playwright evaluation.
    await expect.poll(() => player.evaluate(() => window.__fixturePlayed), { timeout: 10_000 }).toEqual(['play']);
    await expect.poll(() => player.evaluate(() => JSON.parse(parent.__nezzelCatch?.drain() ?? '[]') as CaptureEntry[]))
      .toEqual(expect.arrayContaining([expect.objectContaining({ method: 'video.src', url: origins.main + '/frame-press.mp4' })]));
    expect(await player.evaluate(() => parent.__fixtureRelayWrites)).toEqual({ index: 0, probe: 0 });
  });

  test.describe('with play() recorded', () => {
    test.use({ stubPlay: true });

    test('plays the largest visible player with a source, muted', async ({ page, origins }) => {
      await page.goto(origins.main + '/autoplay');
      const played = () => page.evaluate(() => window.__fixturePlayed.map(play => play.split(':')[0]));
      const muted = () => page.evaluate(() => Array.from(document.querySelectorAll('video, audio'),
        element => (element as HTMLMediaElement).muted));
      await expect.poll(played).toContain('main');
      expect(new Set(await played())).toEqual(new Set(['main']));
      expect(await muted()).not.toContain(false);

      // Mute lock: unmuting does not last.
      await page.evaluate(() => {
        (document.getElementById('main') as HTMLMediaElement).muted = false;
      });
      await expect.poll(muted).not.toContain(false);

      // A player added later and larger is the one to start; an empty shell given its source
      // later is started then.
      await page.evaluate(() => {
        const late = document.createElement('video');
        late.id = 'late';
        late.src = '/late.mp4';
        late.style.cssText = 'display:block;width:600px;height:400px';
        document.body.append(late);
      });
      await expect.poll(played).toContain('late');
      await page.evaluate(() => {
        document.getElementById('late')?.remove();
        document.getElementById('empty')?.setAttribute('src', '/empty.mp4');
      });
      await expect.poll(played).toContain('empty');
      for (const never of ['small', 'unseen', 'invisible', 'sound']) {
        expect(await played()).not.toContain(never);
      }
    });

    test('mutes detached playback and starts a player inside closed shadow DOM', async ({ page, origins }) => {
      await page.goto(origins.main + '/autoplay');
      await page.evaluate(() => {
        document.querySelectorAll('video, audio').forEach(element => element.remove());
        window.__fixturePlayed = [];

        const detached = new Audio();
        detached.id = 'detached';
        void detached.play();
        detached.muted = false;
        detached.volume = 1;
        void detached.play();

        const host = document.createElement('div');
        const shadow = host.attachShadow({ mode: 'closed' });
        shadow.innerHTML = '<video id="closed" src="/closed.mp4" style="display:block;width:400px;height:200px"></video>';
        document.body.append(host);
      });

      const played = () => page.evaluate(() => window.__fixturePlayed);
      expect((await played()).filter(play => play.startsWith('detached'))).toEqual([
        'detached:true:0', 'detached:true:0',
      ]);
      await expect.poll(played).toContain('closed:true:0');
    });
  });

  test.describe('with fake timers', () => {
    test.use({ fakeClock: true });

    test('presses the page\'s own play button once when nothing plays, then plays what it loads', async ({ page, origins }) => {
      await page.goto(origins.main + '/click-to-play');
      await page.clock.runFor(30_000);
      // The button itself, not its player's wrapper; not the ad, a link or a card holding one,
      // the button under the notice, a store badge or a playlist. Once; the video it loads plays muted.
      expect(await page.evaluate(() => window.__fixturePlayed)).toEqual(['facade', 'video:true']);
      await expectEntry(page, { method: 'video.src', url: origins.main + '/stall.mp4' });
    });

    test('presses only after the page has loaded, or once its load is long overdue', async ({ page, origins }) => {
      await page.goto(origins.main + '/late-load', { waitUntil: 'domcontentloaded' });
      await page.clock.runFor(15_000);
      // Still loading: no press yet (the button's handler comes later on such pages).
      expect(await page.evaluate(() => window.__fixtureLoaded)).toBe(false);
      expect(await page.evaluate(() => window.__fixturePlayed)).toEqual([]);
      await page.clock.runFor(10_000);
      expect(await page.evaluate(() => window.__fixturePlayed)).toEqual(['play']);
      // A play button is a player even when no video element ever appears.
      await expectEntry(page, { method: 'player.present' });
    });

    test('another site’s embed and a large same-origin frame count as player hints', async ({ page, origins }) => {
      for (const path of ['/other-embed', '/large-frame']) {
        await page.goto(origins.main + path);
        await page.clock.runFor(30_000);
        await expectEntry(page, { method: 'player.present' });
      }
      await page.goto(origins.main + '/no-player');
      await page.clock.runFor(30_000);
      expect((await drain(page)).filter(entry => entry.method === 'player.present')).toEqual([]);
    });

    test('presses a visible same-origin frame once across competing frames', async ({ page, origins }) => {
      await page.goto(origins.main + '/same-origin-press');
      await page.clock.runFor(30_000);
      await expectEntry(page, { method: 'video.src', url: origins.main + '/frame-press.mp4' });
      const players = page.frames().filter(frame => frame.url().startsWith(origins.main + '/frame-press'));
      expect(players).toHaveLength(2);
      const presses = await Promise.all(players.map(frame => frame.evaluate(() => window.__fixturePlayed)));
      expect(presses.flat()).toEqual(['play']);
    });

    test('presses a visible player nested across origins once', async ({ page, origins }) => {
      await page.goto(origins.main + '/cross-origin-press');
      await page.clock.runFor(30_000);
      await expectEntry(page, { method: 'video.src', url: origins.player + '/frame-press.mp4' });
      expect(await page.frame({ url: origins.player + '/frame-press' })?.evaluate(() => window.__fixturePlayed)).toEqual(['play']);
    });

    test('rejects a page-authored press on the window bus and a forged relay port', async ({ page, origins }) => {
      await page.goto(origins.main + '/covered-frame-press');
      await page.evaluate(() => {
        const target = document.querySelector('iframe')!.contentWindow!;
        const port = new MessageChannel();
        target.postMessage({ type: '__nezzelCatchPort', challenge: 1n, proof: 0n }, '*', [port.port2]);
        target.postMessage({ type: 'press', round: 1 }, '*');
        port.port1.postMessage({ type: 'probe', round: 1 });
        port.port1.postMessage({ type: 'press', round: 1 });
      });
      await page.clock.runFor(30_000);
      expect(await page.frame({ url: origins.main + '/frame-press' })?.evaluate(() => window.__fixturePlayed)).toEqual([]);
    });

    test('scrolls to a play control below the fold and presses it once', async ({ page, origins }) => {
      await page.goto(origins.main + '/below-fold');
      await page.clock.runFor(30_000);
      expect(await page.evaluate(() => window.__fixturePlayed)).toEqual(['play']);
      expect(await page.evaluate(() => scrollY)).toBeGreaterThan(0);

      await page.goto(origins.main + '/covered-below-fold');
      await page.clock.runFor(30_000);
      expect(await page.evaluate(() => scrollY)).toBeGreaterThan(0);
      expect(await page.evaluate(() => window.__fixturePlayed)).toEqual([]);
    });

    test('presses play anchors without canceling their click handlers or navigating', async ({ page, origins }) => {
      for (const [path, href] of [['/play-anchor?hash', '#'], ['/play-anchor', 'javascript:void(0)']]) {
        const url = origins.main + path;
        await page.goto(url);
        await page.clock.runFor(30_000);
        expect(await page.evaluate(() => window.__fixturePlayed)).toEqual(['play']);
        await expect(page).toHaveURL(url);
        await expect(page.locator('a')).toHaveAttribute('href', href);
      }
    });

    test('reports a watch link and a consent wall without following the link or accepting', async ({ page, origins }) => {
      await page.goto(origins.main + '/watch-link');
      await page.clock.runFor(30_000);
      await expectEntry(page, { method: 'player.present' });
      await expect(page).toHaveURL(origins.main + '/watch-link');

      await page.goto(origins.main + '/consent-wall');
      await page.clock.runFor(30_000);
      await expectEntry(page, { method: 'player.present' });
      expect(await page.evaluate(() => window.__fixturePlayed)).toEqual([]);
    });

    test('presses play beside a cookie banner, not under one over the middle of the view', async ({ page, origins }) => {
      await page.goto(origins.main + '/cookie-banner');
      await page.clock.runFor(30_000);
      expect(await page.evaluate(() => window.__fixturePlayed)).toEqual(['play']);

      await page.goto(origins.main + '/cookie-banner?wall');
      await page.clock.runFor(30_000);
      expect(await page.evaluate(() => window.__fixturePlayed)).toEqual([]);
    });

    test('a looping clip started by the pressed play button is reported where it shows', async ({ page, origins }) => {
      await page.goto(origins.main + '/loop-press');
      await page.clock.runFor(30_000);
      expect(await page.evaluate(() => window.__fixturePlayed)).toEqual(['play']);
      const tops: (number | undefined)[] = [];
      await expect.poll(async () => {
        tops.push(...(await drain(page)).filter(entry => entry.method === 'playing').map(entry => entry.playerTop));
        return tops.length;
      }).toBeGreaterThan(0);
      await expect.poll(async () => {
        tops.push(...(await drain(page)).filter(entry => entry.method === 'playing').map(entry => entry.playerTop));
        return tops.length >= 2 && tops.every(top => top !== undefined && top < 2 ** 31 - 1);
      }).toBe(true);
    });

    test('presses the play button of a player whose file is set but that stays paused', async ({ page, origins }) => {
      await page.goto(origins.main + '/paused-src');
      await page.clock.runFor(30_000);
      expect(await page.evaluate(() => window.__fixturePlayed)).toEqual(['play']);
    });

    test('presses nothing while a player starts by itself, here or in a frame', async ({ page, origins }) => {
      for (const path of ['/playing', '/starting', '/fetching', '/framed-player', '/framed-pending-player']) {
        await page.goto(origins.main + path);
        await page.clock.runFor(30_000);
        expect(await page.evaluate(() => window.__fixturePlayed), path).toEqual([]);
      }
    });
  });

  test.describe('in a hidden document', () => {
    test.use({ fakeClock: true, hiddenDocument: true });

    test('mounts and presses a player that needs visibility and animation frames', async ({ page, origins }) => {
      await page.goto(origins.main + '/visibility-player');
      await page.clock.runFor(30_000);
      expect(await page.evaluate(() => window.__fixtureVisibility)).toBe('visible:false');
      expect(await page.evaluate(() => window.__fixturePlayed)).toEqual(['play']);
    });

    test('batches hidden animation callbacks with one timestamp and honors cancellation', async ({ page, origins }) => {
      await page.goto(origins.main + '/');
      await page.evaluate(() => {
        window.__fixturePlayed = [];
        let firstTime: number;
        let cancelled: number;
        requestAnimationFrame((time) => {
          firstTime = time;
          window.__fixturePlayed.push('first');
          cancelAnimationFrame(cancelled);
          requestAnimationFrame(next => window.__fixturePlayed.push(next > time ? 'next tick' : 'same tick'));
        });
        setTimeout(() => {
          requestAnimationFrame(time => window.__fixturePlayed.push(time === firstTime ? 'shared time' : 'different time'));
          cancelled = requestAnimationFrame(() => window.__fixturePlayed.push('cancelled'));
        }, 8);
      });
      await page.clock.runFor(48);
      expect(await page.evaluate(() => window.__fixturePlayed)).toEqual(['first', 'shared time', 'next tick']);
    });
  });
});

test.describe('the window the user sees', () => {
  test.use({ fakeClock: true });

  test('no page can switch automatic catch on: it never presses or mutes', async ({ page, origins }) => {
    await page.goto(origins.main + '/click-to-play');
    // What a page could try: the old event, the global with any value, and a value left on a
    // frame's window before that frame loads a page of the same site.
    await page.evaluate(async () => {
      const scope = globalThis as { __nezzelCatchAuto?: unknown };
      scope.__nezzelCatchAuto = document;
      scope.__nezzelCatchAuto = true;
      dispatchEvent(new Event('__nezzelCatchAuto'));
      const frame = document.createElement('iframe');
      document.body.append(frame);
      const early = frame.contentWindow as (Window & { __nezzelCatchAuto?: unknown }) | null;
      if (early) early.__nezzelCatchAuto = frame.contentDocument;
      frame.src = '/click-to-play';
      await new Promise(resolve => frame.addEventListener('load', resolve, { once: true }));
    });
    await page.clock.runFor(30_000);
    expect(await page.evaluate(() => window.__fixturePlayed)).toEqual([]);
    const frame = page.frames().find(child => child !== page.mainFrame());
    expect(frame).toBeDefined();
    expect(await frame!.evaluate(() => window.__fixturePlayed)).toEqual([]);
    for (const target of [page.mainFrame(), frame!]) {
      expect(await target.evaluate(() => {
        const audio = document.getElementById('silent') as HTMLAudioElement;
        audio.muted = false;
        return audio.muted;
      })).toBe(false);
    }
  });
});

test('a click opens a pop-up only to the link it was on', async ({ page, origins }) => {
  const loaded: string[] = [];
  await page.context().route(/^https:\/\/[a-z]+\.invalid\//, (route) => {
    loaded.push(route.request().url());
    return route.fulfill({ body: '' });
  });
  await page.goto(origins.main + '/popups');
  const opened = async (selector: string) => {
    const popup = page.context().waitForEvent('page', { timeout: 1500 }).catch(() => null);
    await page.click(selector);
    const tab = await popup;
    await tab?.waitForLoadState('domcontentloaded');
    const url = tab?.url() ?? null;
    await tab?.close();
    return url;
  };
  // A pop-under script listens on the whole document: a click on nothing, or one on a link
  // while the script opens an ad instead, opens no ad.
  expect(await opened('#empty')).toBeNull();
  expect(await opened('#adlink')).toBeNull();
  // The same script opens the link's own address: a plain link and one inside a shadow root.
  expect(await opened('#link')).toBe('https://films.invalid/film');
  expect(await opened('#inner')).toBe('https://shadow.invalid/film');
  // A link sent through the page's own redirect on mousedown opens; one swapped for an ad on
  // another site does not.
  expect(await opened('#redirect')).toBe(origins.main + '/out?u=films');
  expect(await opened('#hijack')).toBeNull();
  // A router that cancels the click and clicks the link again by script opens that link.
  expect(await opened('#routed')).toBe('https://notes.invalid/megathread');
  expect(loaded).not.toContain('https://ads.invalid/ad');
  expect(loaded).not.toContain('https://hijack.invalid/ad');
  // A script's click on a new-window link to another site, without a user click, opens nothing.
  const scripted = page.context().waitForEvent('page', { timeout: 1000 }).catch(() => null);
  await page.evaluate(() => {
    Object.assign(document.createElement('a'), { href: 'https://other.invalid/x', target: '_blank' }).click();
  });
  expect(await scripted).toBeNull();
  expect(loaded).not.toContain('https://other.invalid/x');
  // A script's window.open on its own site needs no click.
  const sameSite = await page.evaluate(() => {
    const tab = window.open('/');
    tab?.close();
    return tab !== null;
  });
  expect(sameSite).toBe(true);
});
