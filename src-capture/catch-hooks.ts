import { installPopupGuard } from './popup-guard';

type MediaKind = 'hls' | 'dash' | 'direct' | 'segment';

export interface CaptureEntry {
  method: string;
  url: string | null;
  requestedUrl?: string;
  kind: MediaKind | null;
  ct: string | null;
  frame: string;
  isTop: boolean;
  drm: boolean;
  emeProbe: boolean;
  emeActive: boolean;
  playing: boolean;
  playDeltaSec?: number;
  playedMediaSec?: number;
  playerTop?: number;
  loaded?: boolean;
  durationSec?: number;
  /** Which media element of its frame this is about, so EME evidence stays with its video. */
  mediaId?: number;
  msgOrigin?: string;
}

declare global {
  interface Window {
    __nezzelCatch?: { drain: () => string; snapshot: () => string };
  }
  var __nezzelCatchDeep: boolean | undefined;
}

// Parameters of the native wrapper around the bundled hooks.
declare const relayKey: { document: Document; key: [bigint, bigint] } | undefined;
declare const autoplayDocument: Document | undefined;

type ReportNetwork = (method: string, url: string, ct: string | null, requestedUrl?: string) => void;

// Serialized into the worker bootstrap as well. Keep all runtime dependencies inside this function.
function installNetworkHooks(report: ReportNetwork, workerUrl?: string, lifetime?: AbortSignal) {
  const inWorker = workerUrl !== undefined;
  const absolute = (input: string) => {
    try {
      return new URL(input, workerUrl ?? document.baseURI).href;
    } catch {
      return input;
    }
  };

  try {
    const native = globalThis.fetch;
    const wrapped = new Proxy(native, {
      apply(fetch, receiver, args: Parameters<typeof globalThis.fetch>) {
        const result: Promise<Response> = Reflect.apply(fetch, receiver,
          typeof args[0] === 'string' && inWorker ? [absolute(args[0]), ...args.slice(1)] : args);
        try {
          const input = args[0];
          const requested = absolute(typeof input === 'string' ? input : input instanceof URL ? input.href : input.url);
          void result.then((response) => {
            try {
              if (!response.ok) return;
              report(inWorker ? 'worker-fetch' : 'fetch', response.url || requested,
                response.headers.get('content-type'), response.redirected ? requested : undefined);
            } catch { /* Observation must not change the caller's result. */ }
          }, () => {});
        } catch { /* Preserve the original result if observation cannot be attached. */ }
        return result;
      },
    });
    globalThis.fetch = wrapped;
    lifetime?.addEventListener('abort', () => {
      try {
        if (globalThis.fetch === wrapped) globalThis.fetch = native;
      } catch { /* A page may replace or freeze its APIs. */ }
    }, { once: true });
  } catch { /* The API may be unavailable or read-only. */ }

  try {
    const prototype = XMLHttpRequest.prototype;
    const requests = new WeakMap<XMLHttpRequest, string>();
    const subscribed = new WeakSet<XMLHttpRequest>();
    const native = prototype.open;
    const wrapped = new Proxy(native, {
      apply(open, xhr: XMLHttpRequest, args: unknown[]) {
        const input = args[1];
        const url = typeof input === 'string' && inWorker ? absolute(input) : input;
        const forwarded = typeof input === 'string' && inWorker ? [args[0], url, ...args.slice(2)] : args;
        const result: void = Reflect.apply(open, xhr, forwarded);
        try {
          requests.set(xhr, typeof url === 'string' ? absolute(url) : '');
          if (!subscribed.has(xhr)) {
            xhr.addEventListener('load', () => {
              try {
                if (xhr.status < 200 || xhr.status >= 300) return;
                const requested = requests.get(xhr);
                const finalUrl = xhr.responseURL;
                report(inWorker ? 'worker-xhr' : 'xhr', finalUrl || requested || '',
                  xhr.getResponseHeader('content-type'),
                  finalUrl && finalUrl !== requested ? requested : undefined);
              } catch { /* A page listener can reset the request. */ }
            }, true);
            subscribed.add(xhr);
          }
        } catch { /* Preserve the result of open(). */ }
        return result;
      },
    });
    prototype.open = wrapped;
    lifetime?.addEventListener('abort', () => {
      try {
        if (prototype.open === wrapped) prototype.open = native;
      } catch { /* A page may replace or freeze its APIs. */ }
    }, { once: true });
  } catch { /* Some worker environments do not expose XHR. */ }
}

// Both the probe and the bootstrap run this in the worker's own realm.
function setWorkerLocation(workerUrl: string) {
  const workerLocation = globalThis.location;
  const prototype = Object.getPrototypeOf(workerLocation);
  const original = new URL(workerUrl);
  const properties = ['href', 'origin', 'protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash'] as const;
  for (const property of properties) {
    const descriptor = Object.getOwnPropertyDescriptor(prototype, property);
    if (!descriptor?.configurable || !descriptor.get) return false;
    const native = descriptor.get;
    Object.defineProperty(prototype, property, {
      ...descriptor,
      get() {
        return this === workerLocation ? original[property] : native.call(this);
      },
    });
  }
  const descriptor = Object.getOwnPropertyDescriptor(prototype, 'toString');
  if (!descriptor?.configurable || typeof descriptor.value !== 'function') return false;
  const native = descriptor.value as () => string;
  Object.defineProperty(prototype, 'toString', {
    ...descriptor,
    value() {
      return this === workerLocation ? original.href : native.call(this);
    },
  });
  return String(workerLocation) === original.href && workerLocation.href === original.href;
}

// Serialized with its helpers. importScripts() at the end starts the actual worker.
function startWorker(
  channelName: string,
  workerUrl: string,
  networkHooks: typeof installNetworkHooks,
  restoreLocation: typeof setWorkerLocation,
) {
  const scope = globalThis as typeof globalThis & { importScripts: (...urls: string[]) => void };
  const bootstrapUrl = scope.location.href;
  try {
    restoreLocation(workerUrl);
  } catch { /* A successful probe is required before a bootstrap is used. */ }
  try {
    const channel = new BroadcastChannel(channelName);
    channel.postMessage({ started: bootstrapUrl });
    networkHooks((method, url, ct, requestedUrl) => channel.postMessage({ method, url, ct, requestedUrl }), workerUrl);
  } catch { /* The actual worker can still run without observation. */ }

  const resolve = (input: unknown) => {
    if (typeof input !== 'string') return input;
    try {
      return new URL(input, workerUrl).href;
    } catch {
      return input;
    }
  };

  try {
    scope.Request = new Proxy(scope.Request, {
      construct(Request, args: unknown[], newTarget) {
        const forwarded = typeof args[0] === 'string' ? [resolve(args[0]), ...args.slice(1)] : args;
        return Reflect.construct(Request, forwarded, newTarget);
      },
    });
    scope.importScripts = new Proxy(scope.importScripts, {
      apply(importScripts, receiver, args: unknown[]) {
        return Reflect.apply(importScripts, receiver, args.map(resolve));
      },
    });
  } catch { /* Preserve the worker's native APIs when wrapping is unavailable. */ }
  scope.importScripts(workerUrl);
}

function safely(action: () => void) {
  try {
    action();
  } catch { /* Capture is best-effort. */ }
}

function onAbort(lifetime: AbortSignal, action: () => void) {
  lifetime.addEventListener('abort', () => safely(action), { once: true });
}

function nonce() {
  return Array.from(crypto.getRandomValues(new Uint32Array(4)), value => value.toString(36)).join('-');
}

const maxEntries = 500;
const maxUrlLength = 8192;

function enqueue(queue: CaptureEntry[], entry: CaptureEntry) {
  if (queue.length === maxEntries) queue.shift();
  queue.push(entry);
}

function httpUrl(input: string | null): URL | null {
  if (!input || input.length > maxUrlLength) return null;
  try {
    const url = new URL(input, document.baseURI);
    return (url.protocol === 'http:' || url.protocol === 'https:') && url.href.length <= maxUrlLength ? url : null;
  } catch {
    return null;
  }
}

function mediaKind(url: URL, ct: string | null): MediaKind | null {
  const path = url.pathname.toLowerCase();
  const type = ct?.toLowerCase() ?? '';
  if (/[.](ts|m4s)$/.test(path) || type.startsWith('video/mp2t')) return 'segment';
  if (path.endsWith('.m3u8') || type.includes('mpegurl')) return 'hls';
  if (path.endsWith('.mpd') || type.includes('dash+xml')) return 'dash';
  if (/[.](mp4|m4v|webm|mov|m4a|mp3|ogv|oga|ogg|aac|flac|wav|mkv)$/.test(path)
    || type.startsWith('video/') || type.startsWith('audio/')) return 'direct';
  return null;
}

function redirectedPlaylistRequest(url: URL | null, input?: string): string | undefined {
  if (!url || typeof input !== 'string') return;
  const requested = httpUrl(input);
  if (requested && requested.href !== url.href && /\.m3u8?$/i.test(requested.pathname)) {
    return requested.href;
  }
}

function isMediaElementMethod(method: string) {
  return method.startsWith('video.') || method === 'source.src' || method === 'perf:video' || method === 'perf:audio';
}

function probeBlobWorker(Native: typeof Worker, lifetime: AbortSignal) {
  return new Promise<boolean>((resolve) => {
    let probe: Worker | undefined;
    let probeUrl: string | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    function cancel() {
      finish(false);
    }
    function finish(allowed: boolean) {
      clearTimeout(timer);
      lifetime.removeEventListener('abort', cancel);
      safely(() => probe?.terminate());
      const url = probeUrl;
      if (url) safely(() => URL.revokeObjectURL(url));
      resolve(allowed);
    }
    try {
      const script = `postMessage((${setWorkerLocation.toString()})("https://nezzel.invalid/path/worker.js"));`;
      probeUrl = URL.createObjectURL(new Blob([script], { type: 'text/javascript' }));
      probe = new Native(probeUrl);
      probe.onmessage = event => finish(event.data === true);
      probe.onerror = (event) => {
        event.preventDefault();
        finish(false);
      };
      lifetime.addEventListener('abort', cancel, { once: true });
      timer = setTimeout(cancel, 5000);
    } catch {
      finish(false);
    }
  });
}

async function installWorkerHooks(report: ReportNetwork, lifetime: AbortSignal) {
  const Native = window.Worker;
  const installedDocument = document;
  if (typeof Native !== 'function' || typeof BroadcastChannel !== 'function'
    || !await probeBlobWorker(Native, lifetime) || lifetime.aborted || document !== installedDocument
    || window.Worker !== Native) return;

  const pendingUrls = new Set<string>();
  const channel = new BroadcastChannel('nezzel-catch-worker-' + nonce());
  channel.onmessage = ({ data }: MessageEvent<unknown>) => safely(() => {
    if (!data || typeof data !== 'object') return;
    if ('started' in data && typeof data.started === 'string' && pendingUrls.delete(data.started)) {
      URL.revokeObjectURL(data.started);
    }
    if ('method' in data && (data.method === 'worker-fetch' || data.method === 'worker-xhr')
      && 'url' in data && typeof data.url === 'string'
      && 'ct' in data && (typeof data.ct === 'string' || data.ct === null)) {
      report(data.method, data.url, data.ct,
        'requestedUrl' in data && typeof data.requestedUrl === 'string' ? data.requestedUrl : undefined);
    }
  });

  const wrapped = new Proxy(Native, {
    construct(target, args: unknown[], newTarget) {
      let wrapperUrl: string | undefined;
      try {
        const input = args[0];
        const options = args[1] as WorkerOptions | undefined;
        const type = options?.type;
        if (pendingUrls.size < maxEntries && (typeof input === 'string' || input instanceof URL)
          && (type === undefined || type === 'classic')) {
          const original = httpUrl(String(input));
          if (original?.origin === location.origin) {
            const script = `(${startWorker.toString()})(${JSON.stringify(channel.name)},`
              + `${JSON.stringify(original.href)},(${installNetworkHooks.toString()}),(${setWorkerLocation.toString()}));`;
            const url = URL.createObjectURL(new Blob([script], { type: 'text/javascript' }));
            wrapperUrl = url;
            const worker: Worker = Reflect.construct(target, [url, ...args.slice(1)], newTarget);
            safely(() => {
              pendingUrls.add(url);
              worker.addEventListener('error', () => safely(() => {
                if (pendingUrls.delete(url)) URL.revokeObjectURL(url);
              }), { once: true });
            });
            return worker;
          }
        }
      } catch {
        const url = wrapperUrl;
        if (url) safely(() => URL.revokeObjectURL(url));
      }
      return Reflect.construct(target, args, newTarget);
    },
  });
  onAbort(lifetime, () => {
    if (window.Worker === wrapped) window.Worker = Native;
  });
  onAbort(lifetime, () => {
    channel.close();
    for (const url of pendingUrls) safely(() => URL.revokeObjectURL(url));
    pendingUrls.clear();
  });
  window.Worker = wrapped;
}

const relayHello = '__nezzelCatchHello';
const relayReady = '__nezzelCatchPort';
const captureState = Symbol.for('nezzel.catch.document');

type CaptureSignal = 'emeProbe' | 'emeActive' | 'playing';
type PlaybackEvidence = Pick<CaptureEntry, 'playedMediaSec' | 'playerTop' | 'loaded'>;
type ReportCapture = (method: string, url: string | null, ct: string | null,
  signal?: CaptureSignal, durationSec?: number, playDeltaSec?: number, mediaId?: number, evidence?: PlaybackEvidence) => void;

function isPlayerStatus(method: string) {
  return method === 'player.present' || method === 'player.inactive' || method === 'player.reset' || method === 'player.progress';
}

const offscreenPlayer = 2 ** 31 - 1;
const boxOf = Function.prototype.call.bind(Element.prototype.getBoundingClientRect) as
  (element: Element) => DOMRect;
// Captured at install so a page cannot replace it later; an injected test window may lack it.
const styleOf: (element: Element) => CSSStyleDeclaration = typeof window.getComputedStyle === 'function'
  ? window.getComputedStyle.bind(window)
  : element => getComputedStyle(element);
const viewportSize = (key: 'innerHeight' | 'innerWidth') =>
  Object.getOwnPropertyDescriptor(window, key)?.get?.bind(window) as (() => number) | undefined;
const viewportHeight = viewportSize('innerHeight');
const viewportWidth = viewportSize('innerWidth');
const frameWindowOf = Function.prototype.call.bind(Object.getOwnPropertyDescriptor(HTMLIFrameElement.prototype, 'contentWindow')!.get!) as
  (frame: HTMLIFrameElement) => Window | null;

function visibleTop(element: Element): number {
  const box = boxOf(element);
  if (box.width <= 0 || box.height <= 0 || box.bottom <= 0 || box.right <= 0
    || box.top >= (viewportHeight?.() ?? innerHeight)
    || box.left >= (viewportWidth?.() ?? innerWidth)) return offscreenPlayer;
  const style = styleOf(element);
  return style.visibility === 'visible' && Number(style.opacity) > 0 ? Math.max(0, Math.round(box.top)) : offscreenPlayer;
}
type PressOffer = 'busy' | 'pending' | 'ready' | 'none';
type RunPress = () => void;

interface AutoPress {
  busy: () => boolean;
  pick: () => RunPress | undefined;
  visibleFrames: () => HTMLIFrameElement[];
}

interface FramePeer {
  source: Window;
  port: MessagePort;
  probe: { round: number; answer: (offer: PressOffer) => void } | undefined;
}

/**
 * SipHash-2-4 over three 64-bit words. The per-view key never leaves the hooks.
 * The short, fixed-size input lets the relay authenticate on HTTP too.
 */
function relayProof([key0, key1]: [bigint, bigint]) {
  const mask = 0xffffffffffffffffn;
  const add = (a: bigint, b: bigint) => (a + b) & mask;
  const rol = (a: bigint, b: bigint) => ((a << b) | (a >> (64n - b))) & mask;
  return (direction: bigint, child: bigint, parent: bigint) => {
    let v0 = key0 ^ 0x736f6d6570736575n;
    let v1 = key1 ^ 0x646f72616e646f6dn;
    let v2 = key0 ^ 0x6c7967656e657261n;
    let v3 = key1 ^ 0x7465646279746573n;
    const round = () => {
      v0 = add(v0, v1);
      v1 = rol(v1, 13n) ^ v0;
      v0 = rol(v0, 32n);
      v2 = add(v2, v3);
      v3 = rol(v3, 16n) ^ v2;
      v0 = add(v0, v3);
      v3 = rol(v3, 21n) ^ v0;
      v2 = add(v2, v1);
      v1 = rol(v1, 17n) ^ v2;
      v2 = rol(v2, 32n);
    };
    const word = (value: bigint) => {
      v3 ^= value;
      round();
      round();
      v0 ^= value;
    };
    word(direction);
    word(child);
    word(parent);
    word(24n << 56n);
    v2 ^= 255n;
    round();
    round();
    round();
    round();
    return v0 ^ v1 ^ v2 ^ v3;
  };
}

function installFrameRelay(lifetime: AbortSignal, queue: CaptureEntry[], seen: (entry: CaptureEntry) => void,
  key: [bigint, bigint], autoplay: () => AutoPress | undefined) {
  const parent = window.parent;
  const isTop = parent === window;
  const Channel = MessageChannel;
  const Words = BigUint64Array;
  const random = crypto.getRandomValues.bind(crypto);
  const prove = relayProof(key);
  const challenge = () => random(new Words(1))[0];
  const iterator: typeof Symbol.iterator = Symbol.iterator;
  const send = Function.prototype.call.bind(MessagePort.prototype.postMessage) as
    (port: MessagePort, message: unknown) => void;
  const start = Function.prototype.call.bind(MessagePort.prototype.start) as (port: MessagePort) => void;
  const close = Function.prototype.call.bind(MessagePort.prototype.close) as (port: MessagePort) => void;
  const post = Function.prototype.call.bind(window.postMessage) as
    (target: Window, message: unknown, origin: string, ports?: Iterable<MessagePort>) => void;
  const listen = Function.prototype.call.bind(EventTarget.prototype.addEventListener) as
    (target: EventTarget, type: string, listener: EventListener, options: AddEventListenerOptions) => void;
  const stop = Function.prototype.call.bind(Event.prototype.stopImmediatePropagation) as (event: Event) => void;
  const messageGetter = (key: 'data' | 'source' | 'origin' | 'ports') =>
    Function.prototype.call.bind(Object.getOwnPropertyDescriptor(MessageEvent.prototype, key)!.get!);
  const channelPort = (key: 'port1' | 'port2') =>
    Function.prototype.call.bind(Object.getOwnPropertyDescriptor(Channel.prototype, key)!.get!) as
      (channel: MessageChannel) => MessagePort;
  const dataOf = messageGetter('data') as (event: MessageEvent) => unknown;
  const sourceOf = messageGetter('source') as (event: MessageEvent) => Window | null;
  const originOf = messageGetter('origin') as (event: MessageEvent) => string;
  const portsOf = messageGetter('ports') as (event: MessageEvent) => readonly MessagePort[];
  const firstPort = channelPort('port1');
  const secondPort = channelPort('port2');
  const later = setTimeout.bind(window);
  const cancel = clearTimeout.bind(window);
  const options = { capture: true, signal: lifetime };
  const inputs: FramePeer[] = [];
  const define = Reflect.defineProperty;
  const writeInput = (index: number, value: FramePeer) => {
    const descriptor = { __proto__: null, value, configurable: true, writable: true };
    define(inputs, index, descriptor);
  };
  let upstream: MessagePort | undefined;
  let offeredRound = 0;
  let offeredPress: RunPress | undefined;
  const childChallenge = isTop ? undefined : challenge();

  function deliver(entry: CaptureEntry) {
    if (lifetime.aborted) return;
    safely(() => seen(entry));
    if (upstream) safely(() => send(upstream!, entry));
    else enqueue(queue, entry);
  }

  // A probe walks the same relay tree as playback. Only the top can initiate it, and each
  // frame keeps its chosen route private. Any frame reporting playback vetoes the press.
  function choose(round: number, answer: (offer: PressOffer, run?: RunPress) => void) {
    const local = autoplay();
    if (!local || lifetime.aborted) return answer('none');
    if (local.busy()) return answer('busy');
    let waiting = 0;
    let busy = false;
    let pending = false;
    let picked: FramePeer | undefined;
    let timeout: ReturnType<typeof setTimeout> | undefined;
    const finish = () => {
      cancel(timeout);
      for (let index = 0; index < inputs.length; index += 1) {
        const peer = inputs[index];
        if (peer.probe?.round === round) peer.probe = undefined;
      }
      if (lifetime.aborted) return answer('none');
      if (busy || local.busy()) return answer('busy');
      if (waiting > 0 || pending) return answer('pending');
      const run = local.pick();
      if (run) return answer('ready', run);
      const child = picked;
      if (!child) return answer('none');
      answer('ready', () => {
        if (lifetime.aborted || local.busy()) return;
        const frames = local.visibleFrames();
        for (let index = 0; index < frames.length; index += 1) {
          if (frameWindowOf(frames[index]) === child.source) {
            send(child.port, { type: 'press', round });
            return;
          }
        }
      });
    };
    const frames = local.visibleFrames();
    for (let index = 0; index < frames.length; index += 1) {
      const frame = frames[index];
      const source = frameWindowOf(frame);
      if (!source) continue;
      let peer: FramePeer | undefined;
      for (let index = 0; index < inputs.length; index += 1) {
        if (inputs[index].source === source) {
          peer = inputs[index];
          break;
        }
      }
      if (!peer) {
        // Initial playback reports can still be queued while a frame joins the relay.
        // A blank frame or one that disallows scripts may never join.
        const src = frame.getAttribute('src')?.trim();
        if ((frame.hasAttribute('srcdoc') || (src && src !== 'about:blank'))
          && (!frame.hasAttribute('sandbox') || frame.sandbox.contains('allow-scripts'))) pending = true;
        continue;
      }
      waiting += 1;
      peer.probe = { round, answer: (offer) => {
        if (offer === 'busy') busy = true;
        if (offer === 'pending') pending = true;
        if (offer === 'ready' && !picked) picked = peer;
        if (--waiting === 0) finish();
      } };
      safely(() => send(peer.port, { type: 'probe', round }));
    }
    if (waiting > 0) timeout = later(finish, 500);
    else finish();
  }

  function accept(source: Window, port: MessagePort, origin: string, expected: bigint) {
    const peer: FramePeer = { source, port, probe: undefined };
    function connect() {
      // A frame navigation replaces its previous channel once the new hooks authenticate.
      for (let index = 0; index < inputs.length; index += 1) {
        if (inputs[index].source === source) {
          close(inputs[index].port);
          writeInput(index, peer);
          return;
        }
      }
      if (inputs.length === maxEntries) {
        close(inputs[0].port);
        for (let index = 1; index < inputs.length; index += 1) writeInput(index - 1, inputs[index]);
        inputs.length -= 1;
      }
      writeInput(inputs.length, peer);
    }
    let connected = false;
    const pending = later(() => close(port), 2000);
    listen(port, 'message', event => safely(() => {
      const data = dataOf(event as MessageEvent);
      if (!connected) {
        if (data !== expected) return;
        cancel(pending);
        connected = true;
        connect();
        return;
      }
      const offer = data as { type?: unknown; round?: unknown; offer?: unknown } | null;
      const probe = peer.probe;
      if (probe && offer?.type === 'offer' && offer.round === probe.round
        && (offer.offer === 'busy' || offer.offer === 'pending' || offer.offer === 'ready' || offer.offer === 'none')) {
        peer.probe = undefined;
        probe.answer(offer.offer);
        return;
      }
      const entry = data as Partial<CaptureEntry> | null;
      if (!entry || typeof entry.frame !== 'string' || entry.frame.length > maxUrlLength
        || typeof entry.method !== 'string' || entry.method.length > 64) return;
      const url = typeof entry.url === 'string' ? httpUrl(entry.url) : null;
      const ct = typeof entry.ct === 'string' ? entry.ct.slice(0, 512) : null;
      const requestedUrl = redirectedPlaylistRequest(url, entry.requestedUrl);
      const kind = url && (mediaKind(url, ct) ?? (requestedUrl ? 'hls' : null));
      const emeProbe = entry.emeProbe === true;
      const emeActive = entry.emeActive === true;
      const playing = entry.playing === true;
      if (!kind && !emeProbe && !emeActive && !playing && !isPlayerStatus(entry.method)
        && !(url && isMediaElementMethod(entry.method))) return;
      const msgOrigin = typeof entry.msgOrigin === 'string' && entry.msgOrigin.length <= maxUrlLength
        ? entry.msgOrigin
        : origin;
      const top = entry.playerTop;
      let playerTop = typeof top === 'number' && Number.isInteger(top) && top >= 0 && top <= offscreenPlayer
        ? top
        : undefined;
      if (playerTop !== undefined && playerTop < offscreenPlayer) {
        // Each relay level adds its own frame's offset, so nested players end up in top coordinates.
        const frames = document.getElementsByTagName('iframe');
        let frame: HTMLIFrameElement | undefined;
        for (let index = 0; index < frames.length; index += 1) {
          if (frames[index].contentWindow === source) {
            frame = frames[index];
            break;
          }
        }
        const frameTop = frame ? visibleTop(frame) : offscreenPlayer;
        playerTop = Math.min(offscreenPlayer, frameTop + playerTop);
      }
      deliver({
        method: entry.method, url: url?.href ?? null, requestedUrl, kind, ct, frame: entry.frame, isTop: false,
        drm: emeActive, emeProbe, emeActive, playing, msgOrigin,
        playDeltaSec: typeof entry.playDeltaSec === 'number' ? entry.playDeltaSec : undefined,
        playedMediaSec: typeof entry.playedMediaSec === 'number' ? entry.playedMediaSec : undefined,
        playerTop,
        loaded: entry.loaded === true || undefined,
        durationSec: typeof entry.durationSec === 'number' ? entry.durationSec : undefined,
        mediaId: typeof entry.mediaId === 'number' && Number.isSafeInteger(entry.mediaId) ? entry.mediaId : undefined,
      });
    }), options);
    start(port);
  }

  // Public messages only establish channels. The two proofs are bound to fresh challenges
  // in both documents, so a page's window messages or substitute ports cannot grant a press.
  listen(window, 'message', event => safely(() => {
    if (!event.isTrusted) return;
    const message = event as MessageEvent;
    const source = sourceOf(message);
    const ports = portsOf(message);
    const data = dataOf(message);
    const handshake = data as { type?: unknown; challenge?: unknown; proof?: unknown } | null;
    if (source && source !== window && source.parent === window && !ports.length
      && handshake?.type === relayHello && typeof handshake.challenge === 'bigint') {
      const child = handshake.challenge;
      if (child < 0n || child > 0xffffffffffffffffn) return;
      stop(event);
      const origin = originOf(message);
      const channel = new Channel();
      const own = challenge();
      const ready = { type: relayReady, challenge: own, proof: prove(1n, child, own) };
      accept(source, firstPort(channel), origin, prove(2n, child, own));
      const port = secondPort(channel);
      // A page can replace Array's iterator. Give postMessage its own transfer iterator so
      // the port is never passed to page code during Web IDL sequence conversion.
      const transfer: Iterable<MessagePort> = {
        [iterator](): Iterator<MessagePort> {
          let pending = true;
          return { next() {
            if (!pending) return { done: true, value: undefined };
            pending = false;
            return { done: false, value: port };
          } };
        },
      };
      post(source, ready, origin === 'null' ? '*' : origin, transfer);
    } else if (childChallenge !== undefined && source === parent && ports.length === 1 && !upstream) {
      if (handshake?.type !== relayReady || typeof handshake.challenge !== 'bigint'
        || handshake.challenge < 0n || handshake.challenge > 0xffffffffffffffffn
        || handshake.proof !== prove(1n, childChallenge, handshake.challenge)) return;
      stop(event);
      const port = ports[0];
      send(port, prove(2n, childChallenge, handshake.challenge));
      listen(port, 'message', event => safely(() => {
        const command = dataOf(event as MessageEvent) as { type?: unknown; round?: unknown } | null;
        if (typeof command?.round !== 'number') return;
        const round = command.round;
        if (command.type === 'probe' && round > offeredRound) {
          offeredRound = round;
          offeredPress = undefined;
          choose(round, (offer, run) => {
            if (offeredRound !== round || lifetime.aborted) return;
            offeredPress = run;
            send(port, { type: 'offer', round, offer });
          });
        } else if (command.type === 'press' && round === offeredRound) {
          const run = offeredPress;
          offeredPress = undefined;
          run?.();
        }
      }), options);
      start(port);
      upstream = port;
      for (const entry of queue.splice(0)) deliver(entry);
    }
  }), options);

  onAbort(lifetime, () => {
    if (upstream) safely(() => close(upstream!));
    for (let index = 0; index < inputs.length; index += 1) safely(() => close(inputs[index].port));
  });
  if (childChallenge !== undefined) safely(() => post(parent, { type: relayHello, challenge: childChallenge }, '*'));
  return { deliver, choose };
}

function install() {
  const installedDocument = document;
  const auto = autoplayDocument === installedDocument;
  const captureWindow: Window & {
    [captureState]?: { document: Document; controller: AbortController };
  } = window;
  const previous = captureWindow[captureState];
  if (previous?.document === installedDocument) return;
  previous?.controller.abort();
  const controller = new AbortController();
  captureWindow[captureState] = { document: installedDocument, controller };
  const lifetime = controller.signal;
  if (auto) safely(() => keepHiddenDocumentActive(installedDocument, lifetime));

  const isTop = window.top === window;
  const queue: CaptureEntry[] = [];
  // A player started in this frame or a frame below: playback, encrypted media, or a script
  // fetching a stream (an MSE player filling). Automatic catch presses nothing then. A media
  // element's own file is no evidence: a paused player with its file set still needs its button.
  let started = false;
  const seen = (entry: CaptureEntry) => {
    if (entry.playing || entry.emeActive
      || (entry.kind && entry.kind !== 'direct' && !isMediaElementMethod(entry.method))) started = true;
  };
  let deliver = (entry: CaptureEntry) => {
    seen(entry);
    enqueue(queue, entry);
  };
  let autoPress: AutoPress | undefined;
  let choose: ReturnType<typeof installFrameRelay>['choose'] = (_round, answer) => {
    const local = autoPress;
    if (!local) return answer('none');
    if (local.busy()) return answer('busy');
    const run = local.pick();
    if (run) answer('ready', run);
    else answer('none');
  };
  if (relayKey?.document === installedDocument) safely(() => {
    ({ deliver, choose } = installFrameRelay(lifetime, queue, seen, relayKey.key, () => autoPress));
  });

  if (isTop) {
    // Captured before page scripts run: a page that later replaces JSON.stringify,
    // String.prototype.slice or the title getter can't change what the snapshot returns.
    const stringify = JSON.stringify;
    const slice = Function.prototype.call.bind(String.prototype.slice) as (text: string, start: number, end: number) => string;
    const indexOf = Function.prototype.call.bind(String.prototype.indexOf) as (text: string, search: string) => number;
    const titleOf = Object.getOwnPropertyDescriptor(Document.prototype, 'title')?.get;
    const title = titleOf ? Function.prototype.call.bind(titleOf) as (document: Document) => unknown : () => '';
    const agent = String(navigator.userAgent);
    const text = (value: unknown, max: number) => typeof value === 'string' ? slice(value, 0, max) : '';
    // A single-page site changes the address without a new document (history.pushState) and
    // often leaves the first page's og:image in place: that picture is not the current page's.
    // The fragment alone (#comments) is still the same page.
    const address = () => {
      const href = text(location.href, 8193);
      const hash = indexOf(href, '#');
      return hash < 0 ? href : slice(href, 0, hash);
    };
    const loadedUrl = address();
    // Read once, before page scripts run: the page cannot change what its own document answered.
    const status = safelyRead(() => Number((performance.getEntriesByType('navigation')[0] as PerformanceNavigationTiming | undefined)?.responseStatus) || 0, 0);
    const geo = geoReader(installedDocument);
    const passwordVisible = safelyRead(passwordProbe, () => false);
    // Frozen: a page script cannot swap `snapshot` for its own answer.
    const catcher = Object.freeze({
      drain() {
        try {
          return stringify(queue.splice(0));
        } catch {
          return '[]';
        }
      },
      /** `[location, title, userAgent, hits, [poster, pageImage], [status, text], passwordVisible]` as one JSON string: the only value native reads. */
      snapshot() {
        try {
          const pictures = safelyRead(() => pagePictures(installedDocument, address() === loadedUrl), ['', '']);
          const password = safelyRead(() => passwordVisible(installedDocument), false);
          return stringify([text(location.href, 8193), text(title(installedDocument), 512), text(agent, 1025), queue.splice(0), [text(pictures[0], 2049), text(pictures[1], 2049)], [status | 0, text(safelyRead(geo, ''), 2001)], password === true]);
        } catch {
          return '';
        }
      },
    });
    try {
      Object.defineProperty(window, '__nezzelCatch', { value: catcher, configurable: true, enumerable: false, writable: false });
    } catch {
      window.__nezzelCatch = catcher;
    }
  }

  function report(method: string, input: string | null, ct: string | null,
    signal?: CaptureSignal, durationSec?: number, playDeltaSec?: number, mediaId?: number,
    evidence?: PlaybackEvidence, requested?: string) {
    if (lifetime.aborted || document !== installedDocument) return;
    const url = httpUrl(input);
    const contentType = ct?.slice(0, 512) ?? null;
    const requestedUrl = redirectedPlaylistRequest(url, requested);
    const kind = url && (mediaKind(url, contentType) ?? (requestedUrl ? 'hls' : null));
    if (!kind && !signal && !isPlayerStatus(method) && !(url && isMediaElementMethod(method))) return;
    const emeActive = signal === 'emeActive';
    deliver({
      method, url: url?.href ?? null, requestedUrl, kind, ct: contentType, frame: location.href, isTop,
      drm: emeActive, emeActive, emeProbe: signal === 'emeProbe', playing: signal === 'playing',
      durationSec, playDeltaSec, mediaId, ...evidence,
    });
  }

  safely(() => {
    const observer = new PerformanceObserver(list => safely(() => {
      for (const entry of list.getEntries() as PerformanceResourceTiming[]) {
        const status = entry.responseStatus;
        report('perf:' + entry.initiatorType, entry.name, null, undefined, undefined, undefined, undefined,
          status >= 200 && status < 300 ? { loaded: true } : undefined);
      }
    }));
    onAbort(lifetime, () => observer.disconnect());
    observer.observe({ type: 'resource', buffered: true });
  });

  // Every frame: a player inside a frame opens pop-unders from there.
  safely(installPopupGuard);
  const reportNetwork: ReportNetwork = (method, url, ct, requested) =>
    report(method, url, ct, undefined, undefined, undefined, undefined, { loaded: true }, requested);
  installNetworkHooks(reportNetwork, undefined, lifetime);
  let observeAutoplayRoot: ((root: ShadowRoot) => void) | undefined;
  safely(() => installMediaHooks(report, installedDocument, lifetime, root => observeAutoplayRoot?.(root)));
  const enableDeep = () => {
    if (window.__nezzelCatchDeep === true) void installWorkerHooks(reportNetwork, lifetime).catch(() => {});
  };
  // The native webview can append the opt-in script to its existing document-start scripts.
  if (window.__nezzelCatchDeep === true) enableDeep();
  else window.addEventListener('__nezzelCatchDeep', enableDeep, { once: true, signal: lifetime });
  // Automatic catch (a page nobody sees): start the player, muted. No event or later value
  // turns it on.
  if (auto) safely(() => {
    const autoplay = installAutoplay(installedDocument, lifetime, () => started,
      () => report('player.present', null, null), choose);
    autoPress = autoplay;
    observeAutoplayRoot = autoplay.observeRoot;
  });
}

function installMediaHooks(
  report: ReportCapture,
  installedDocument: Document,
  lifetime: AbortSignal,
  onShadowRoot: (root: ShadowRoot) => void,
) {
  const mediaSelector = 'video, audio, video source, audio source';
  const mediaTags = ['video', 'audio', 'source'] as const;
  const seen = new WeakMap<Element, [string | null, string | null]>();

  function scan(element: Element) {
    if (element instanceof HTMLMediaElement) {
      const src = element.hasAttribute('src') ? element.src : null;
      const currentSrc = element.currentSrc || null;
      const previous = seen.get(element);
      if (previous?.[0] !== src) report('video.src', src, null);
      if (previous?.[1] !== currentSrc) report('video.currentSrc', currentSrc, null);
      seen.set(element, [src, currentSrc]);
    } else if (element instanceof HTMLSourceElement && element.matches(mediaSelector)) {
      const src = element.hasAttribute('src') ? element.src : null;
      const ct = element.type || null;
      const previous = seen.get(element);
      if (previous?.[0] !== src || previous?.[1] !== ct) report('source.src', src, ct);
      seen.set(element, [src, ct]);
    }
  }

  const mediaUrl = (element: HTMLMediaElement) => element.currentSrc || element.src;
  const mediaIds = new WeakMap<HTMLMediaElement, number>();
  let nextMediaId = 0;
  const mediaId = (element: HTMLMediaElement) => {
    let id = mediaIds.get(element);
    if (id === undefined) {
      id = ++nextMediaId;
      mediaIds.set(element, id);
    }
    return id;
  };
  const playback = new WeakMap<HTMLMediaElement, { source: string; time: number; position: number }>();
  const loopOf = Function.prototype.call.bind(Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, 'loop')!.get!) as
    (element: HTMLMediaElement) => boolean;
  const rangesOf = Function.prototype.call.bind(Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, 'played')!.get!) as
    (element: HTMLMediaElement) => TimeRanges;
  // Looping clips report where they show: Al Jazeera's video pages autoplay their clip on a
  // loop. Rust keeps a short player from displacing an unplayed film (state.rs FILM_SEC).
  const playerTop = (element: HTMLMediaElement) => visibleTop(element);
  // The engine's played ranges exclude gaps made by seeking and reset on every new source.
  // Keep looping players on the existing timeupdate accounting.
  const playedMediaSec = (element: HTMLMediaElement) => {
    if (loopOf(element)) return;
    const played = rangesOf(element);
    let seconds = 0;
    for (let i = 0; i < played.length; i++) seconds += played.end(i) - played.start(i);
    return seconds;
  };

  // Media events do not leave a shadow root, and document queries do not enter one:
  // each root (the document and every shadow root) gets its own scan and listeners.
  function observeRoot(root: Document | ShadowRoot) {
    root.querySelectorAll(mediaSelector).forEach(scan);
    safely(() => {
      const observer = new MutationObserver(records => safely(() => {
        for (const record of records) {
          if (record.type === 'attributes' && record.target instanceof Element) scan(record.target);
          for (const node of record.addedNodes) {
            if (node instanceof Element) {
              scan(node);
              // Most added nodes are leaves or hold no media: skip the subtree query for them.
              if (node.firstElementChild) {
                for (const tag of mediaTags) {
                  for (const element of node.getElementsByTagName(tag)) scan(element);
                }
              }
            }
          }
        }
      }));
      onAbort(lifetime, () => observer.disconnect());
      observer.observe(root, { subtree: true, childList: true, attributes: true, attributeFilter: ['src', 'type'] });
    });

    for (const event of ['loadstart', 'emptied', 'loadedmetadata', 'durationchange', 'playing', 'timeupdate',
      'pause', 'waiting', 'seeking', 'ended', 'encrypted', 'webkitneedkey']) {
      root.addEventListener(event, ({ target, isTrusted }) => safely(() => {
        if (!(target instanceof HTMLMediaElement)) return;
        if (event === 'encrypted' || event === 'webkitneedkey') {
          report('encrypted', mediaUrl(target), null, 'emeActive', target.duration, undefined, mediaId(target));
        } else if (event === 'playing') {
          const source = mediaUrl(target);
          if (isTrusted) playback.set(target, { source, time: performance.now(), position: target.currentTime });
          report('playing', source, null, 'playing', target.duration, undefined, mediaId(target),
            isTrusted ? { playerTop: playerTop(target) } : undefined);
        } else if (event === 'timeupdate') {
          if (!isTrusted) return;
          const previous = playback.get(target);
          const source = mediaUrl(target);
          if (!previous || previous.source !== source || target.seeking || target.paused) return;
          const time = performance.now();
          if (time - previous.time < 225) return;
          playback.set(target, { source, time, position: target.currentTime });
          if (target.currentTime > previous.position) {
            report('playing', source, null, 'playing', target.duration,
              Math.min((time - previous.time) / 1000, 1), mediaId(target),
              { playerTop: playerTop(target), playedMediaSec: playedMediaSec(target) });
          }
        } else if (isTrusted && (event === 'loadstart' || event === 'emptied')) {
          playback.delete(target);
          report('player.reset', null, null, undefined, undefined, undefined, mediaId(target));
          if (event === 'loadstart') scan(target);
        } else if (isTrusted && ['pause', 'waiting', 'seeking', 'ended'].includes(event)) {
          const previous = playback.get(target);
          if (previous) previous.time = Infinity;
          const source = mediaUrl(target);
          if (event === 'ended' && target.ended && !loopOf(target) && previous?.source === source) {
            report('player.progress', source, null, undefined, target.duration, undefined, mediaId(target),
              { playedMediaSec: playedMediaSec(target) });
          }
          if (event === 'pause' || event === 'ended') {
            report('player.inactive', null, null, undefined, undefined, undefined, mediaId(target));
          }
        } else if (event === 'loadedmetadata' || event === 'durationchange') {
          scan(target);
          report('video.currentSrc', mediaUrl(target), null, undefined, target.duration, undefined, mediaId(target));
        }
      }), { capture: true, signal: lifetime });
    }
  }

  observeRoot(installedDocument);
  safely(() => {
    const prototype = Element.prototype;
    const native = prototype.attachShadow;
    if (typeof native !== 'function') return;
    const wrapped = new Proxy(native, {
      apply(method, receiver: Element, args: [ShadowRootInit]) {
        const root: ShadowRoot = Reflect.apply(method, receiver, args);
        if (!lifetime.aborted && receiver.ownerDocument === installedDocument) safely(() => {
          observeRoot(root);
          onShadowRoot(root);
        });
        return root;
      },
    });
    prototype.attachShadow = wrapped;
    onAbort(lifetime, () => {
      if (prototype.attachShadow === wrapped) prototype.attachShadow = native;
    });
  });

  safely(() => {
    const native = navigator.requestMediaKeySystemAccess;
    if (typeof native !== 'function') return;
    const wrapped = new Proxy(native, {
      apply(method, receiver, args: unknown[]) {
        safely(() => report('requestMediaKeySystemAccess', null, null, 'emeProbe'));
        return Reflect.apply(method, receiver, args);
      },
    });
    navigator.requestMediaKeySystemAccess = wrapped;
    onAbort(lifetime, () => {
      if (navigator.requestMediaKeySystemAccess === wrapped) navigator.requestMediaKeySystemAccess = native;
    });
  });

  const mediaPrototype = HTMLMediaElement.prototype as HTMLMediaElement & {
    webkitSetMediaKeys?: HTMLMediaElement['setMediaKeys'];
  };
  for (const key of ['setMediaKeys', 'webkitSetMediaKeys'] as const) {
    safely(() => {
      const native = mediaPrototype[key];
      if (typeof native !== 'function') return;
      const wrapped = new Proxy(native, {
        apply(method, receiver: HTMLMediaElement, args: unknown[]) {
          // Players attach keys to clear video too: only an `encrypted` event is evidence, so
          // attaching keys is reported as a probe. setMediaKeys(null) is a teardown: not reported.
          if (args[0] != null) safely(() => report(key, mediaUrl(receiver), null, 'emeProbe'));
          return Reflect.apply(method, receiver, args);
        },
      });
      mediaPrototype[key] = wrapped;
      onAbort(lifetime, () => {
        if (mediaPrototype[key] === wrapped) mediaPrototype[key] = native;
      });
    });
  }
}

/**
 * The player a visitor would start: the largest visible video, else an audio element.
 * Elements that take no room (hidden, zero-sized, preloaded off-page) are not a player.
 */
function safelyRead<T>(read: () => T, fallback: T): T {
  try {
    return read();
  } catch {
    return fallback;
  }
}

/** An absolute http(s) address, else ''. */
function pictureUrl(value: string | null | undefined, base: string): string {
  if (!value) return '';
  try {
    const url = new URL(value, base);
    return url.protocol === 'http:' || url.protocol === 'https:' ? url.href : '';
  } catch {
    return '';
  }
}

/**
 * Where a thumbnail for the page's video can come from: the poster of the playing (else the
 * largest visible) video, and the picture the page names for itself (og:image, twitter:image,
 * link rel=image_src). Native fetches them; the page never learns.
 */
function pagePictures(installedDocument: Document, sameAddress: boolean): [string, string] {
  const base = installedDocument.baseURI;
  let poster = '';
  let posterRank = -1;
  for (const video of installedDocument.getElementsByTagName('video')) {
    const url = pictureUrl(video.getAttribute('poster'), base);
    if (!url) continue;
    const box = video.getBoundingClientRect();
    const rank = (video.paused ? 0 : 1e12) + Math.max(0, box.width * box.height);
    if (rank > posterRank) {
      poster = url;
      posterRank = rank;
    }
  }
  let image = '';
  if (!sameAddress) return [poster, image];
  for (const selector of ['meta[property="og:image:secure_url"]', 'meta[property="og:image"]', 'meta[name="og:image"]', 'meta[name="twitter:image"]', 'meta[property="twitter:image"]', 'meta[name="twitter:image:src"]', 'link[rel~="image_src"]']) {
    const element = installedDocument.querySelector(selector);
    image = pictureUrl(element?.getAttribute(element instanceof HTMLLinkElement ? 'href' : 'content'), base);
    if (image) break;
  }
  return [poster, image];
}

/**
 * What native's geo-block check reads (`geo::blocked`) beside the top document's HTTP status:
 * the text a short page shows, '' for any other page. Scripts, styles and frames are not text
 * the visitor sees. Read at most every two seconds: `innerText` lays the page out.
 */
function geoReader(installedDocument: Document): () => string {
  let read = 0;
  let last = '';
  return () => {
    const now = Date.now();
    if (now - read < 2e3) return last;
    read = now;
    last = '';
    const body = installedDocument.body;
    if (!body) return last;
    let unseen = 0;
    for (const element of body.querySelectorAll('script, style, noscript')) unseen += element.textContent?.length ?? 0;
    // A cheap bound first, so a long page is never laid out for this.
    if ((body.textContent?.length ?? 0) - unseen >= 2e4) return last;
    const shown = String(body.innerText ?? '').replace(/\s+/g, ' ').trim();
    if (shown.length < 1500) last = shown;
    return last;
  };
}

/**
 * Whether the top page shows a password field the visitor can see and type into: a sign-in wall on
 * the page itself (a landing page with its login form). Only that it exists: never what is typed.
 * A field off-screen, inside a hidden, clipped or inert container (a closed login modal or drawer)
 * or disabled is no wall. The DOM calls are kept here, before page scripts: a page that replaces
 * them later cannot answer for the hook.
 */
function passwordProbe(): (installedDocument: Document) => boolean {
  const bind = <T extends (...args: never[]) => unknown>(method: T) =>
    Function.prototype.call.bind(method) as (self: ThisParameterType<T>, ...args: Parameters<T>) => ReturnType<T>;
  const all = bind(Document.prototype.querySelectorAll as (this: Document, selectors: string) => NodeListOf<Element>);
  const boxOf = bind(Element.prototype.getBoundingClientRect);
  const matches = bind(Element.prototype.matches);
  const parentGetter = Object.getOwnPropertyDescriptor(Node.prototype, 'parentElement')?.get as ((this: Node) => Element | null) | undefined;
  const parentOf = parentGetter ? bind(parentGetter) : () => null;
  const styleOf = getComputedStyle.bind(window);
  const Input = HTMLInputElement;
  return (installedDocument) => {
    const inputs = all(installedDocument, 'input[type="password" i]');
    for (let index = 0; index < inputs.length; index++) {
      const input = inputs[index];
      if (!(input instanceof Input) || matches(input, ':disabled, [inert], [inert] *')) continue;
      const box = boxOf(input);
      // What is left of the field inside the window and every container that clips it.
      let [left, top, right, bottom] = [Math.max(box.left, 0), Math.max(box.top, 0), Math.min(box.right, innerWidth), Math.min(box.bottom, innerHeight)];
      let shown = styleOf(input).visibility === 'visible';
      // What the walk is leaving: a fixed box is clipped by the window only (a login modal over a
      // scroll-locked page, or built inside a clipped header); an absolute one by its positioned
      // ancestors, not by the static ones between.
      let escaped: 'no' | 'absolute' | 'fixed' = 'no';
      for (let element: Element | null = input; shown && element; element = parentOf(element)) {
        const style = styleOf(element);
        if (style.display === 'none' || !(parseFloat(style.opacity) > 0)) shown = false;
        const clips = escaped === 'no' || (escaped === 'absolute' && style.position !== 'static');
        // The page's own boxes clip nothing here: the window already did.
        if (clips && element !== input && element !== installedDocument.body && element !== installedDocument.documentElement) {
          const clip = boxOf(element);
          if (style.overflowX !== 'visible') [left, right] = [Math.max(left, clip.left), Math.min(right, clip.right)];
          if (style.overflowY !== 'visible') [top, bottom] = [Math.max(top, clip.top), Math.min(bottom, clip.bottom)];
        }
        if (escaped !== 'fixed' && style.position === 'fixed') escaped = 'fixed';
        else if (escaped !== 'fixed' && style.position === 'absolute') escaped = 'absolute';
        else if (clips) escaped = 'no';
      }
      // More than a sliver: a 1px "screen reader only" wrapper shows nothing.
      if (shown && right - left > 1 && bottom - top > 1) return true;
    }
    return false;
  };
}

function pickAutoplayTarget(elements: HTMLMediaElement[]): HTMLMediaElement | null {
  let best: HTMLMediaElement | null = null;
  let bestArea = -1;
  for (let index = 0; index < elements.length; index += 1) {
    const element = elements[index];
    if (!element.isConnected || element.ended || !hasSource(element)) continue;
    const style = getComputedStyle(element);
    if (style.visibility === 'hidden') continue;
    let area = 0;
    if (element instanceof HTMLVideoElement) {
      const box = element.getBoundingClientRect();
      area = box.width * box.height;
      if (style.display === 'none' || area <= 0) continue;
    }
    if (area > bestArea) {
      best = element;
      bestArea = area;
    }
  }
  return best;
}

/** A source to play: an element play() is called on without one plays nothing. */
function hasSource(element: HTMLMediaElement) {
  return Boolean(element.currentSrc || element.getAttribute('src') || element.srcObject || element.querySelector('source[src]'));
}

/**
 * A player that is starting or plays: unpaused with a source, or an unpaused video that takes
 * room on the page (an MSE player before its blob is attached).
 */
function running(element: HTMLMediaElement) {
  if (!element.isConnected || element.paused || element.ended) return false;
  if (hasSource(element)) return true;
  const box = element.getBoundingClientRect();
  return element instanceof HTMLVideoElement && box.width * box.height > 0;
}

/** Engines without a native visibility override still let a hidden player render its controls. */
function keepHiddenDocumentActive(doc: Document, lifetime: AbortSignal) {
  const hidden = Object.getOwnPropertyDescriptor(Document.prototype, 'hidden')?.get?.bind(doc);
  const visibility = Object.getOwnPropertyDescriptor(Document.prototype, 'visibilityState')?.get?.bind(doc);
  if (!hidden || !visibility) return;
  const frame = window.requestAnimationFrame;
  const cancelFrame = window.cancelAnimationFrame;
  const after = window.setTimeout.bind(window);
  const cancelAfter = window.clearTimeout.bind(window);
  const now = performance.now.bind(performance);
  const reportError = window.reportError?.bind(window);
  let nextId = -1;
  let timer: number | undefined;
  let pending = new Map<number, FrameRequestCallback>();
  let running: typeof pending | undefined;
  Object.defineProperties(doc, {
    visibilityState: { configurable: true, get: () => hidden() ? 'visible' : visibility() },
    hidden: { configurable: true, get: () => false },
  });
  const request = (callback: FrameRequestCallback) => {
    if (!hidden()) return frame.call(window, callback);
    // Timer-backed frame IDs occupy a separate range from real animation frames.
    const id = nextId--;
    pending.set(id, callback);
    timer ??= after(() => {
      timer = undefined;
      const batch = pending;
      pending = new Map();
      running = batch;
      const timestamp = now();
      for (const run of batch.values()) {
        if (lifetime.aborted) break;
        try {
          run(timestamp);
        } catch (error) {
          // Like native rAF, report a callback error and keep processing this frame.
          if (reportError) reportError(error);
          else after(() => {
            throw error;
          }, 0);
        }
      }
      running = undefined;
    }, 16);
    return id;
  };
  const cancel = (id: number) => {
    if (id >= 0) cancelFrame.call(window, id);
    else {
      pending.delete(id);
      running?.delete(id);
    }
  };
  window.requestAnimationFrame = request;
  window.cancelAnimationFrame = cancel;
  onAbort(lifetime, () => {
    cancelAfter(timer);
    pending.clear();
    if (window.requestAnimationFrame === request) window.requestAnimationFrame = frame;
    if (window.cancelAnimationFrame === cancel) window.cancelAnimationFrame = cancelFrame;
  });
}

// After load: before it, the page may not have wired its button yet and the one press is lost.
const pressAfterLoad = 3000;
// A page whose load never ends (a stalled request) is pressed then.
const pressFallback = 20000;
const pressRetry = 2000;
const pressRetries = 6;
const playWords = /(^|[^a-z])play([^a-z]|$)|play-?button|big-?play|(^|\s)ل?تشغيل(\s|$)|(^|[^a-z])lecture([^a-z]|$)|reproducir|abspielen/i;
/** Labels with "play" that start no video here. */
const notPlay = /continuer la lecture|mode lecture|liste de lecture|قائمة التشغيل|تلقائي|google[- ]?play|play[- ]?store|auto[- ]?play|air[- ]?play|playlist|replay|\b(?:accept|agree|consent)\b|accepter|أوافق|قبول/i;
const pauseWords = /(^|[^a-z])pause([^a-z]|$)|إيقاف/i;
/** Elements a press goes to: real controls only, never a container around one. */
const controlSelector = 'button, [role="button"], input[type="button" i], a';
const watchWords = /\bwatch\b|regarder|voir la vidéo|شاهد|مشاهدة/i;
const consentWords = /cookie|consent|privacy choices|confidentialité|ملفات تعريف الارتباط/i;
const consentAction = /\b(?:accept|agree|allow|reject|decline|manage)\b|accepter|refuser|أوافق|قبول|رفض/i;
/** One id or class name that marks an ad slot: `ad-banner`, `video-ads`, `sponsored`, `taboola`. */
const adName = /^(ads?|advert\w*|sponsored|promo\w*|taboola\w*|outbrain\w*)([-_]|$)|[-_](ads?|advert\w*|sponsored)([-_]|$)/i;
const arrayFrom = Array.from;
const arrayFilter = Function.prototype.call.bind(Array.prototype.filter) as
  <T>(items: T[], test: (item: T) => unknown) => T[];
const arraySome = Function.prototype.call.bind(Array.prototype.some) as
  <T>(items: T[], test: (item: T) => unknown) => boolean;

/** What a person reads on the control: its accessible name, title, or short text. */
function humanLabel(element: Element) {
  const text = (element.textContent ?? '').trim();
  return arrayFilter([element.getAttribute('aria-label'), element.getAttribute('title'),
    element instanceof HTMLInputElement ? element.value : '', text.length <= 40 ? text : ''], Boolean).join(' ');
}

function namesOf(element: Element) {
  return `${element.id} ${typeof element.className === 'string' ? element.className : ''}`;
}

/** "Play" by its label, or, with no label, by its id or class (`vjs-big-play-button`). */
function saysPlay(element: Element) {
  const label = humanLabel(element);
  if (label) return playWords.test(label) && !notPlay.test(label) && !pauseWords.test(label);
  const names = namesOf(element);
  return playWords.test(names) && !notPlay.test(names);
}

/** The element and every container it sits in, shadow roots crossed. */
function* ancestors(element: Element) {
  let node: Element | null = element;
  while (node) {
    yield node;
    const parent: Node | null = node.parentNode;
    node = parent instanceof ShadowRoot ? parent.host : parent instanceof Element ? parent : null;
  }
}

function isAdSlot(node: Element) {
  // Utility classes with values (`[--footer-ad-height:0]`, `md:p-4`) name no slot.
  return node.hasAttribute('data-ad')
    || arraySome(namesOf(node).split(/\s+/), name => name && !/[[:(]/.test(name) && adName.test(name));
}

/** A link or submit control that would take the page somewhere else instead of playing. */
function navigates(node: Element) {
  if (node instanceof HTMLAnchorElement || node instanceof HTMLAreaElement) {
    const href = node.getAttribute('href')?.trim();
    return href !== undefined && href !== '' && href !== '#' && !/^javascript:/i.test(href);
  }
  return (node instanceof HTMLButtonElement || node instanceof HTMLInputElement)
    && node.type === 'submit' && node.form !== null;
}

/** The control, what it sits in or what it holds is an ad or leads away from the page. */
function unsafe(control: Element, hit?: Element) {
  for (const node of ancestors(control)) {
    if (isAdSlot(node) || navigates(node)) return true;
  }
  if (hit && hit !== control) {
    for (const node of ancestors(hit)) {
      if (isAdSlot(node) || navigates(node)) return true;
    }
  }
  for (const node of control.querySelectorAll('*')) {
    if (isAdSlot(node) || navigates(node)) return true;
  }
  return false;
}

function hitAtCenter(root: Document | ShadowRoot, box: DOMRect) {
  const x = box.left + box.width / 2;
  const y = box.top + box.height / 2;
  if (x < 0 || y < 0 || x >= innerWidth || y >= innerHeight) return null;
  return root.elementFromPoint(x, y);
}

function hitPlayControl(root: Document | ShadowRoot, control: Element) {
  const hit = hitAtCenter(root, control.getBoundingClientRect());
  return hit && (hit === control || control.contains(hit)) && !unsafe(control, hit);
}

function gone(element: Element) {
  if (styleOf(element).visibility !== 'visible') return true;
  for (const ancestor of ancestors(element)) {
    if (styleOf(ancestor).opacity === '0') return true;
  }
  return false;
}

function playControl(element: Element) {
  return saysPlay(element) && !element.matches(':disabled') && element.getAttribute('aria-disabled') !== 'true';
}

/**
 * Prefer the largest play control the pointer can reach. If there is no such control, keep
 * the largest one below the fold for a scroll and a fresh hit-test.
 */
function pickPlayControl(roots: Set<Document | ShadowRoot>) {
  const found: { element: Element; root: Document | ShadowRoot; area: number; belowFold: boolean }[] = [];
  for (const root of roots) {
    for (const element of root.querySelectorAll(controlSelector)) {
      if (!playControl(element)) continue;
      const box = element.getBoundingClientRect();
      if (box.width < 24 || box.height < 24 || gone(element)) continue;
      const x = box.left + box.width / 2;
      const belowFold = x >= 0 && x < innerWidth && box.top + box.height / 2 >= innerHeight;
      if (belowFold ? unsafe(element) : !hitPlayControl(root, element)) continue;
      found.push({ element, root, area: box.width * box.height, belowFold });
    }
  }
  let best: typeof found[number] | undefined;
  for (let index = 0; index < found.length; index += 1) {
    const candidate = found[index];
    if (arraySome(found, other => other !== candidate && candidate.element.contains(other.element))) continue;
    if (!best || (best.belowFold && !candidate.belowFold)
      || (best.belowFold === candidate.belowFold && candidate.area > best.area)) best = candidate;
  }
  return best;
}

function modal(dialog: Element) {
  if (dialog.getAttribute('aria-modal') === 'true') return true;
  try {
    return dialog.matches(':modal');
  } catch {
    return false;
  }
}

function consentWall(roots: Set<Document | ShadowRoot>) {
  for (const root of roots) {
    for (const dialog of root.querySelectorAll('dialog, [role="dialog"], [role="alertdialog"], [aria-modal="true"]')) {
      const box = dialog.getBoundingClientRect();
      if (box.width < 150 || box.height < 80 || gone(dialog)) continue;
      const hit = hitAtCenter(root, box);
      if (!hit || !dialog.contains(hit)) continue;
      // A banner along an edge leaves the player usable: only a modal one, or one over the
      // middle of the view, stands between the visitor and the video.
      if (!modal(dialog) && !dialog.contains(hitAtCenter(root, new DOMRect(0, 0, innerWidth, innerHeight)))) continue;
      if (!consentWords.test(`${humanLabel(dialog)} ${dialog.textContent}`)) continue;
      if (arraySome(arrayFrom(dialog.querySelectorAll(controlSelector)), control => consentAction.test(humanLabel(control)))) return true;
    }
  }
  return false;
}

/** One press as a pointer makes it, at the control's centre. */
function press(element: Element) {
  const box = element.getBoundingClientRect();
  const at = { bubbles: true, cancelable: true, composed: true, view: window, button: 0,
    clientX: box.left + box.width / 2, clientY: box.top + box.height / 2 };
  const pointer = { ...at, pointerId: 1, isPrimary: true, pointerType: 'mouse' };
  const Pointer = typeof PointerEvent === 'function' ? PointerEvent : MouseEvent;
  element.dispatchEvent(new Pointer('pointerdown', pointer));
  element.dispatchEvent(new MouseEvent('mousedown', { ...at, buttons: 1 }));
  element.dispatchEvent(new Pointer('pointerup', pointer));
  element.dispatchEvent(new MouseEvent('mouseup', at));
  const click = new MouseEvent('click', at);
  const link = Array.from(ancestors(element)).find(node => node instanceof HTMLAnchorElement);
  const href = link?.getAttribute('href');
  if (!link || href == null) {
    element.dispatchEvent(click);
    return;
  }
  // Suppress hyperlink activation while handlers receive an uncanceled click, even if they
  // stop propagation. Restore the placeholder unless a handler supplies a new href.
  link.removeAttribute('href');
  try {
    element.dispatchEvent(click);
  } finally {
    if (!link.hasAttribute('href')) link.setAttribute('href', href);
  }
}

/**
 * Automatic catch only. Plays the page's main player, muted, with the element's own play();
 * when nothing plays, presses the page's own play button once (`pickPlayControl`). Every media
 * element and audio context of the page stays silent.
 */
function installAutoplay(installedDocument: Document, lifetime: AbortSignal, mediaStarted: () => boolean,
  reportPlayer: () => void, choose: ReturnType<typeof installFrameRelay>['choose']) {
  const prototype = HTMLMediaElement.prototype;
  const play = prototype.play;
  const roots = new Set<Document | ShadowRoot>();
  const maxAttempts = 20;
  let attempts = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let playerPage = '';
  const fallbackAt = Date.now() + pressFallback;
  const playerEmbed = /^https?:\/\/(?:(?:www\.)?youtube(?:-nocookie)?\.com\/embed\/|player\.vimeo\.com\/video\/|(?:www\.)?dailymotion\.com\/embed\/video\/|geo\.dailymotion\.com\/player[./]|player\.twitch\.tv\/|(?:www\.)?streamable\.com\/e\/|cdn\.jwplayer\.com\/players\/)/i;

  const properties = [['muted', true], ['volume', 0]] as const;
  const setters = properties.map(([name, quietValue]) => {
    const descriptor = Object.getOwnPropertyDescriptor(prototype, name);
    const set = descriptor?.set;
    if (!descriptor || !set) return undefined;
    const lock = function (this: HTMLMediaElement) {
      Reflect.apply(set, this, [quietValue]);
    };
    safely(() => {
      Object.defineProperty(prototype, name, { ...descriptor, set: lock });
      onAbort(lifetime, () => {
        if (Object.getOwnPropertyDescriptor(prototype, name)?.set === lock) {
          Object.defineProperty(prototype, name, descriptor);
        }
      });
    });
    return (element: HTMLMediaElement) => {
      if (element[name] !== quietValue) lock.call(element);
    };
  });
  const silence = (element: HTMLMediaElement) => safely(() => {
    for (let index = 0; index < setters.length; index += 1) setters[index]?.(element);
  });
  const quietPlay = new Proxy(play, {
    apply(method, receiver: HTMLMediaElement, args: unknown[]) {
      silence(receiver);
      return Reflect.apply(method, receiver, args);
    },
  });
  prototype.play = quietPlay;
  onAbort(lifetime, () => {
    if (prototype.play === quietPlay) prototype.play = play;
  });

  // Signs that the user may still need a step, even if this page never exposes a video.
  const reportOnce = () => {
    if (playerPage === location.href) return;
    playerPage = location.href;
    reportPlayer();
  };
  const foreignFrame = (src: string) => {
    try {
      const url = new URL(src, location.href);
      return /^https?:$/.test(url.protocol) && url.origin !== location.origin;
    } catch {
      return false;
    }
  };
  const media = () => {
    const elements: HTMLMediaElement[] = [];
    for (const root of roots) {
      for (const element of root.querySelectorAll<HTMLMediaElement>('video, audio')) elements.push(element);
    }
    return elements;
  };
  const busy = () => lifetime.aborted || document !== installedDocument || mediaStarted() || arraySome(media(), running);
  const documentFrames = installedDocument.getElementsByTagName.bind(installedDocument) as
    (tag: 'iframe') => HTMLCollectionOf<HTMLIFrameElement>;
  const shadowFrames = Function.prototype.call.bind(ShadowRoot.prototype.querySelectorAll) as
    (root: ShadowRoot, selector: 'iframe') => NodeListOf<HTMLIFrameElement>;
  const frameElements = (root: Document | ShadowRoot) => root instanceof ShadowRoot
    ? shadowFrames(root, 'iframe')
    : documentFrames('iframe');
  const forEachRoot = roots.forEach.bind(roots);
  const define = Reflect.defineProperty;
  const visibleFrames = () => {
    const visible: HTMLIFrameElement[] = [];
    forEachRoot((root) => {
      const frames = frameElements(root);
      for (let index = 0; index < frames.length; index += 1) {
        const frame = frames[index];
        const box = boxOf(frame);
        if (box.width >= 24 && box.height >= 24 && !gone(frame) && !unsafe(frame)
          && hitAtCenter(root, box) === frame) {
          const descriptor = { __proto__: null, value: frame };
          define(visible, visible.length, descriptor);
        }
      }
    });
    return visible;
  };
  const playerHint = () => consentWall(roots) || arraySome(arrayFrom(roots), root =>
    arraySome(arrayFrom(frameElements(root)), frame => playerEmbed.test(frame.src) || foreignFrame(frame.src)
      || (frame.clientWidth >= 300 && frame.clientHeight >= 150 && !gone(frame)))
    || arraySome(arrayFrom(root.querySelectorAll(controlSelector)), (element) => {
      const box = element.getBoundingClientRect();
      if (box.width < 24 || box.height < 24 || gone(element)) return false;
      if (element instanceof HTMLAnchorElement && navigates(element)) {
        const label = humanLabel(element) || namesOf(element);
        return !notPlay.test(label) && !/\bwatch[- ]?list\b/i.test(label)
          && (saysPlay(element) || watchWords.test(label)) && !arraySome(arrayFrom(ancestors(element)), isAdSlot);
      }
      return playControl(element) && !unsafe(element);
    }));
  const pick = (): RunPress | undefined => {
    if ((installedDocument.readyState !== 'complete' && Date.now() < fallbackAt) || consentWall(roots)) return;
    const control = pickPlayControl(roots);
    if (!control) return;
    reportOnce();
    return () => {
      if (busy() || !control.element.isConnected || !playControl(control.element) || consentWall(roots)) return;
      if (control.belowFold) {
        if (gone(control.element) || unsafe(control.element)) return;
        control.element.scrollIntoView({ block: 'center', inline: 'nearest', behavior: 'instant' });
      }
      if (!busy() && hitPlayControl(control.root, control.element)) press(control.element);
    };
  };

  function attempt() {
    timer = undefined;
    if (lifetime.aborted || document !== installedDocument) return;
    const elements = media();
    if (playerPage !== location.href && (elements.length > 0 || playerHint())) reportOnce();
    elements.forEach(silence);
    // A player that already runs is the page's own choice: leave it.
    if (arraySome(elements, running)) return;
    const target = pickAutoplayTarget(elements);
    if (!target || attempts >= maxAttempts) return;
    attempts += 1;
    safely(() => {
      const started: unknown = Reflect.apply(quietPlay, target, []);
      if (started instanceof Promise) started.catch(() => {});
    });
  }

  const schedule = () => {
    if (timer === undefined && !lifetime.aborted) timer = setTimeout(attempt, 250);
  };
  onAbort(lifetime, () => clearTimeout(timer));

  // Click-to-play pages load their player only when its own play button is pressed. Once per
  // page (top document), and only when nothing started by itself.
  let pressTimer: ReturnType<typeof setTimeout> | undefined;
  let pressChecks = 0;
  const checkPress = () => {
    pressTimer = undefined;
    if (lifetime.aborted || document !== installedDocument || pressChecks >= pressRetries) return;
    pressChecks += 1;
    if (playerPage !== location.href && playerHint()) reportOnce();
    choose(pressChecks, (offer, run) => {
      if (busy() || offer === 'busy') return;
      if (run) {
        pressChecks = pressRetries;
        safely(run);
      } else if (pressChecks < pressRetries) pressTimer = setTimeout(checkPress, pressRetry);
    });
  };
  const schedulePress = (delay: number) => {
    if (window.top !== window || pressChecks > 0 || lifetime.aborted) return;
    clearTimeout(pressTimer);
    pressTimer = setTimeout(checkPress, delay);
  };
  onAbort(lifetime, () => clearTimeout(pressTimer));
  schedulePress(pressFallback);

  function observeRoot(root: Document | ShadowRoot) {
    roots.add(root);
    for (const event of ['play', 'playing', 'volumechange']) {
      root.addEventListener(event, ({ target }) => {
        if (target instanceof HTMLMediaElement) silence(target);
      }, { capture: true, signal: lifetime });
    }
    for (const event of ['loadedmetadata', 'canplay', 'emptied', 'pause']) {
      root.addEventListener(event, ({ target }) => {
        if (target instanceof HTMLMediaElement) schedule();
      }, { capture: true, signal: lifetime });
    }
    safely(() => {
      const observer = new MutationObserver(records => safely(() => {
        for (const record of records) {
          // A source given late (an MSE blob, a facade's video) makes an element playable.
          if (record.type === 'attributes') {
            schedule();
            return;
          }
          for (const node of record.addedNodes) {
            // Inserting a host can make a previously detached closed root playable.
            if (node instanceof Element) {
              schedule();
              return;
            }
          }
        }
      }));
      onAbort(lifetime, () => observer.disconnect());
      observer.observe(root, { subtree: true, childList: true, attributes: true,
        attributeFilter: ['src', 'open', 'hidden', 'aria-hidden'] });
    });
    schedule();
  }
  observeRoot(installedDocument);
  installedDocument.addEventListener('DOMContentLoaded', schedule, { signal: lifetime });
  window.addEventListener('load', () => {
    schedule();
    schedulePress(pressAfterLoad);
  }, { signal: lifetime });

  // Web Audio has no element to mute: its contexts never leave the suspended state.
  for (const name of ['AudioContext', 'webkitAudioContext'] as const) {
    safely(() => {
      const scope = window as unknown as Record<string, typeof AudioContext | undefined>;
      const Native = scope[name];
      if (typeof Native !== 'function') return;
      const resume = Native.prototype.resume;
      const quiet = new Proxy(resume, { apply: () => Promise.resolve() });
      Native.prototype.resume = quiet;
      const Wrapped = new Proxy(Native, {
        construct(target, args: unknown[], newTarget) {
          const context: AudioContext = Reflect.construct(target, args, newTarget);
          safely(() => void context.suspend().catch(() => {}));
          return context;
        },
      });
      scope[name] = Wrapped;
      onAbort(lifetime, () => {
        if (Native.prototype.resume === quiet) Native.prototype.resume = resume;
        if (scope[name] === Wrapped) scope[name] = Native;
      });
    });
  }

  return { observeRoot, busy, pick, visibleFrames };
}

safely(install);
