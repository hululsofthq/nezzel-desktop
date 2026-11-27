import { describe, expect, it, vi } from 'vitest';
import { installExternalLinks } from '../../src/externalLinks';

function click(html: string, init: MouseEventInit = {}) {
  const scope = document.createElement('div');
  scope.innerHTML = html;
  const open = vi.fn();
  installExternalLinks(scope, open);
  const event = new MouseEvent('click', { bubbles: true, cancelable: true, button: 0, ...init });
  scope.querySelector('a')!.dispatchEvent(event);
  return { open, prevented: event.defaultPrevented };
}

describe('installExternalLinks', () => {
  it.each([
    ['a _blank web link', '<a href="https://example.com/x" target="_blank">x</a>', {}, 'https://example.com/x'],
    ['a Ctrl-clicked link', '<a href="https://example.com/y">y</a>', { ctrlKey: true }, 'https://example.com/y'],
    ['a _blank mail link', '<a href="mailto:a@example.com" target="_blank">m</a>', {}, 'mailto:a@example.com'],
  ])('opens %s in the default browser', (_name, html, init, url) => {
    const { open, prevented } = click(html, init);
    expect(open).toHaveBeenCalledWith(url);
    expect(prevented).toBe(true);
  });

  it.each([
    ['a same-tab link', '<a href="https://example.com/z">z</a>', {}],
    ['a Cmd-click', '<a href="https://example.com/z" target="_blank">z</a>', { metaKey: true }],
    ['a middle press', '<a href="https://example.com/z" target="_blank">z</a>', { button: 1 }],
    ['a non-web scheme', '<a href="file:///etc/hosts" target="_blank">f</a>', {}],
  ])('leaves %s to the webview', (_name, html, init) => {
    const { open, prevented } = click(html, init);
    expect(open).not.toHaveBeenCalled();
    expect(prevented).toBe(false);
  });
});
