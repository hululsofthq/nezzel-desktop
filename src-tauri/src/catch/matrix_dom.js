// Developer sweep only (`--features catch-matrix`): what the hidden page's document holds.
// Hosts and short labels only; never an address. Returns one JSON string.
(() => {
  try {
    const visible = (el) => {
      const box = el.getBoundingClientRect();
      const style = getComputedStyle(el);
      return box.width > 0 && box.height > 0 && style.visibility !== 'hidden' && style.display !== 'none'
        && Number(style.opacity) > 0.05 ? box : null;
    };
    const label = (el) => [el.getAttribute('aria-label'), el.getAttribute('title'), el.getAttribute('data-testid'),
      typeof el.className === 'string' ? el.className : '', el.id, (el.textContent || '').trim().slice(0, 40)]
      .filter(Boolean).join(' | ').slice(0, 160);
    const media = { top: 0, sameOriginFrames: 0, shadow: 0, playing: 0, withSrc: 0 };
    const frames = [];
    const plays = [];
    const walls = [];
    const playWord = /(^|[^a-z])(play|vjs-big-play|ytp-large-play|jw-icon-display|plyr__control--overlaid)|تشغيل|شاهد/i;
    const wallWord = /consent|cookie|gdpr|onetrust|didomi|sp_message|qc-cmp|fc-consent|truste|cmp-|privacy-?manager|usercentrics/i;
    const shadowHosts = new Set();
    const custom = new Set();
    const walk = (root, where, depth) => {
      if (depth > 4) return;
      for (const el of root.querySelectorAll('*')) {
        if (el.shadowRoot) walk(el.shadowRoot, 'shadow', depth + 1);
        if (el.shadowRoot) shadowHosts.add(el.tagName.toLowerCase());
        if (el instanceof HTMLMediaElement) {
          media[where] += 1;
          if (!el.paused) media.playing += 1;
          if (el.currentSrc || el.src) media.withSrc += 1;
        }
        if (el instanceof HTMLIFrameElement) {
          let host = '';
          try { host = new URL(el.src, location.href).host; } catch { host = ''; }
          let same = false;
          try { same = !!el.contentDocument; } catch { same = false; }
          const box = visible(el);
          if (frames.length < 12) frames.push({ host, same, w: Math.round(box?.width || 0), h: Math.round(box?.height || 0) });
          if (same) try { walk(el.contentDocument, 'sameOriginFrames', depth + 1); } catch {}
        }
        const tag = el.tagName.toLowerCase();
        if (tag.includes('-') && custom.size < 12) custom.add(tag);
        const clickable = tag === 'button' || el.getAttribute('role') === 'button' || tag === 'a'
          || (el.hasAttribute && el.hasAttribute('tabindex')) || /play/i.test(String(el.className));
        if (clickable && plays.length < 8) {
          const text = label(el);
          if (playWord.test(text)) {
            const box = visible(el);
            if (box) {
              const hitEl = root.elementFromPoint?.(box.left + box.width / 2, box.top + box.height / 2);
              plays.push({ tag, text: text.slice(0, 80), w: Math.round(box.width), h: Math.round(box.height),
                href: tag === 'a' ? (el.getAttribute('href') || '').slice(0, 1) : null, where,
                role: el.getAttribute('role'), x: Math.round(box.left), y: Math.round(box.top),
                hit: hitEl ? (hitEl === el || el.contains(hitEl) ? 'self' : `${hitEl.tagName}.${String(hitEl.className).slice(0, 40)}`) : 'none',
                inner: Array.from(el.querySelectorAll('button, [role="button"], input[type="button" i]')).map(c => c.tagName).slice(0, 3) });
            }
          }
        }
        const id = `${el.id} ${typeof el.className === 'string' ? el.className : ''}`;
        if (walls.length < 4 && wallWord.test(id)) {
          const box = visible(el);
          if (box && box.width * box.height > 40000) walls.push({ tag, text: id.slice(0, 80), w: Math.round(box.width), h: Math.round(box.height) });
        }
      }
    };
    walk(document, 'top', 0);
    // Whether this page renders at all: frames painted and intersection callbacks since the first look.
    if (!window.__matrixTicks) {
      const ticks = { raf: 0, io: 0 };
      const frame = () => { ticks.raf += 1; requestAnimationFrame(frame); };
      requestAnimationFrame(frame);
      try { new IntersectionObserver(() => { ticks.io += 1; }).observe(document.body || document.documentElement); } catch {}
      window.__matrixTicks = ticks;
      ticks.clicks = [];
      document.addEventListener('click', ({ target }) => ticks.clicks.push(`${Math.round(performance.now())} ${target?.tagName}`), true);
    }
    window.__matrixTicks.load = Math.round(performance.getEntriesByType('navigation')[0]?.loadEventEnd || 0);
    const body = (document.body?.innerText || '').slice(0, 4000);
    const bot = /just a moment|attention required|verify you are human|are you a robot|checking your browser|captcha/i.test(document.title + ' ' + body)
      || frames.some(f => /challenges\.cloudflare|hcaptcha|recaptcha|captcha/i.test(f.host));
    return JSON.stringify({
      title: document.title.slice(0, 60), ready: document.readyState, media, frames, plays, walls,
      shadowHosts: [...shadowHosts].slice(0, 8), custom: [...custom], bot,
      visibility: document.visibilityState, ticks: window.__matrixTicks,
      consentText: /cookie|consent|privacy choices|we use cookies/i.test(body.slice(0, 1500)),
    });
  } catch (error) {
    return JSON.stringify({ error: String(error).slice(0, 120) });
  }
})()
