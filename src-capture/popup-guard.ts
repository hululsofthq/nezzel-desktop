// Pop-up guard: a page's pop-up opens only when the click that caused it was on a link or form
// leading there, on a button whose own handler opened it, or when it stays on the page's site.
// Pop-under scripts open their ads from a click handler on the whole document, so a click alone
// proves nothing. The engine's new-window request can't wait for the page, so the page decides
// here, before the request exists.

/** The last real click (or form submit) of the user in this frame. */
export interface ClickRecord {
  /** The address of the link or form clicked, or null when it was neither. */
  target: string | null;
  /** The click was on a button (or an element acting as one). */
  button: boolean;
  /** `performance.now()` when it happened. */
  at: number;
}

/** A pop-up more than this long after the click was not caused by it. */
export const clickWindowMs = 1000;

/** The handler that is running: a document-wide one is how pop-under scripts listen. */
export type Handler = 'element' | 'document' | 'none';

const twoPartSuffixes = new Set(['co', 'com', 'net', 'org', 'gov', 'edu', 'ac', 'or', 'ne', 'go']);

/** Registrable domain, close enough without a suffix list: `a.b.example.co.uk` → `example.co.uk`. */
export function siteOf(host: string): string {
  const name = host.toLowerCase().replace(/\.$/, '');
  if (/^[\d.]+$/.test(name) || name.includes(':')) return name;
  const labels = name.split('.');
  const two = labels.length >= 3 && labels[labels.length - 1].length === 2
    && twoPartSuffixes.has(labels[labels.length - 2]);
  return labels.slice(two ? -3 : -2).join('.');
}

function parse(input: string, base: string): URL | null {
  try {
    const url = new URL(input, base);
    return url.protocol === 'http:' || url.protocol === 'https:' ? url : null;
  } catch {
    return null;
  }
}

function sameAddress(a: URL, b: URL) {
  return a.href.split('#')[0] === b.href.split('#')[0];
}

/**
 * Whether a pop-up for `requested` (as the page passed it) may open from a page at `page`.
 * `allowOnce`: the user allowed the next pop-up.
 */
export function popupAllowed(
  requested: string, page: string, click: ClickRecord | null, now: number,
  handler: Handler, allowOnce: boolean,
): boolean {
  if (allowOnce) return true;
  const fresh = click !== null && now - click.at >= 0 && now - click.at <= clickWindowMs;
  // A button's own handler (not one on the whole document) opened it: a sign-in button.
  if (fresh && click.button && handler === 'element') return true;
  const to = parse(requested, page);
  // A blank pop-up is written or sent elsewhere by the script: it proves nothing by itself.
  if (!to) return false;
  const from = parse(page, page);
  if (from && siteOf(from.hostname) === siteOf(to.hostname)) return true;
  if (!fresh || click.target === null) return false;
  const link = parse(click.target, page);
  return link !== null && (sameAddress(link, to) || siteOf(link.hostname) === siteOf(to.hostname));
}

/**
 * The click as the guard keeps it. A script may rewrite a link between the button going down
 * (`pressed`) and the click (`clicked`): a redirect on the page's own site or the link's own site
 * is kept, an address on another site is a hijack and the link's address from the press stays.
 */
export function settleClick(pressed: ClickRecord | null, clicked: ClickRecord, page: string): ClickRecord {
  if (pressed === null || pressed.target === null || clicked.target === null) return clicked;
  if (clicked.at - pressed.at < 0 || clicked.at - pressed.at > clickWindowMs) return clicked;
  const before = parse(pressed.target, page);
  const after = parse(clicked.target, page);
  if (!before || !after) return clicked;
  const from = parse(page, page);
  const site = siteOf(after.hostname);
  if (site === siteOf(before.hostname) || (from && site === siteOf(from.hostname))) return clicked;
  return { ...clicked, target: pressed.target };
}

const guardKey = Symbol.for('nezzel.popup.guard');

/** What native and the main-frame pop-up script reach: the allow-once gate and the count. */
export interface PopupGuard {
  gate(open: boolean): void;
  /** Pop-ups this guard stopped, ever. */
  count(): number;
}

const buttonRoles = new Set(['button', 'link', 'menuitem']);

function record(event: Event): ClickRecord {
  let target: string | null = null;
  let button = false;
  for (const node of event.composedPath()) {
    if (!(node instanceof Element)) continue;
    if ((node instanceof HTMLAnchorElement || node instanceof HTMLAreaElement) && node.hasAttribute('href')) {
      target = node.href;
      break;
    }
    if (node instanceof HTMLFormElement) {
      target = node.action;
      break;
    }
    if (node instanceof HTMLButtonElement || (node instanceof HTMLInputElement && /^(submit|button|image)$/i.test(node.type))) {
      button = true;
      const form = node.form;
      if (form && node.type.toLowerCase() !== 'button') {
        target = node.formAction || form.action;
        break;
      }
      continue;
    }
    if (buttonRoles.has((node.getAttribute('role') ?? '').toLowerCase())) button = true;
  }
  return { target, button, at: performance.now() };
}

/** The listener running now, for a call made inside an event handler. */
function handlerOf(): Handler {
  const event = (window as { event?: Event }).event;
  const current = event?.currentTarget;
  if (!event || !current) return 'none';
  if (current === window || current === document || current === document.documentElement || current === document.body) {
    return 'document';
  }
  return 'element';
}

function opensNewWindow(target: string) {
  const name = target.trim().toLowerCase();
  return name !== '' && name !== '_self' && name !== '_parent' && name !== '_top';
}

export function installPopupGuard() {
  const scope = window as Window & { [guardKey]?: PopupGuard };
  if (scope[guardKey]) return;
  let click: ClickRecord | null = null;
  let pressed: ClickRecord | null = null;
  let allowOnce = false;
  let blocked = 0;
  let pressedLink: Element | null = null;
  const linkOf = (event: Event) => event.composedPath().find(node =>
    (node instanceof HTMLAnchorElement || node instanceof HTMLAreaElement) && node.hasAttribute('href')) as Element | undefined;
  const remember = (event: Event) => {
    if (!event.isTrusted) return;
    const seen = record(event);
    if (event.type === 'pointerdown') {
      pressed = seen;
      pressedLink = linkOf(event) ?? null;
      click = seen;
      return;
    }
    // Only the link that was pressed can have been rewritten since.
    const same = event.type !== 'submit' && pressedLink !== null && linkOf(event) === pressedLink;
    click = same ? settleClick(pressed, seen, location.href) : seen;
  };
  // Capture phase on the window: before any handler of the page.
  for (const type of ['pointerdown', 'click', 'auxclick', 'submit']) {
    addEventListener(type, remember, { capture: true });
  }
  const allowed = (requested: string) => {
    const yes = popupAllowed(requested, location.href, click, performance.now(), handlerOf(), allowOnce);
    if (!yes) blocked += 1;
    return yes;
  };
  const guard: PopupGuard = {
    gate(open) {
      allowOnce = open === true;
    },
    count: () => blocked,
  };
  Object.defineProperty(scope, guardKey, { value: guard });

  const open = window.open;
  window.open = function (...args: Parameters<typeof window.open>) {
    const requested = args[0] === undefined || args[0] === null ? '' : String(args[0]);
    if (!allowed(requested)) return null;
    return Reflect.apply(open, this ?? window, args) as Window | null;
  };
  // A script's own click on a link or submit of a form that opens a new window.
  const anchorClick = HTMLElement.prototype.click;
  HTMLElement.prototype.click = function (this: HTMLElement) {
    if (this instanceof HTMLAnchorElement && opensNewWindow(this.target) && !allowed(this.href)) return;
    Reflect.apply(anchorClick, this, []);
  };
  const dispatch = EventTarget.prototype.dispatchEvent;
  EventTarget.prototype.dispatchEvent = function (this: EventTarget, event: Event) {
    if (this instanceof HTMLAnchorElement && event.type === 'click' && opensNewWindow(this.target)
      && !allowed(this.href)) return true;
    return Reflect.apply(dispatch, this, [event]) as boolean;
  };
  for (const name of ['submit', 'requestSubmit'] as const) {
    const native = HTMLFormElement.prototype[name] as (this: HTMLFormElement, ...args: unknown[]) => void;
    if (typeof native !== 'function') continue;
    (HTMLFormElement.prototype[name] as unknown) = function (this: HTMLFormElement, ...args: unknown[]) {
      if (opensNewWindow(this.target) && !allowed(this.action)) return;
      Reflect.apply(native, this, args);
    };
  }
}
