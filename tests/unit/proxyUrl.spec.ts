import { describe, expect, it } from 'vitest';
import { isValidProxyUrl } from '../../src/helpers/network';

describe('proxy address', () => {
  it.each([
    ['socks5://127.0.0.1:1080', true],
    ['socks5://127.0.0.1:1080/', true],
    ['http://user:pass@proxy.example.com:8080', true],
    ['https://proxy.example.com', true],
    ['socks5h://[::1]:9050', true],
    ['socks4a://10.0.0.1:1080', true],
    ['127.0.0.1:1080', false],
    ['ftp://proxy.example.com:21', false],
    ['http://proxy.example.com:99999', false],
    ['http://proxy example.com', false],
    ['http://proxy.example.com:0', false],
    // A host is an address or letters, digits, dots and hyphens: nothing an engine's proxy argument could read as a second rule.
    ['socks5://a;https=direct:1080', false],
    ['socks5://a,direct:1080', false],
    ['socks5://a"b:1080', false],
    ['socks5://a\'b:1080', false],
    ['socks5://a=b:1080', false],
    ['socks5://a%20b:1080', false],
    ['http://-proxy.example.com:8080', false],
    ['http://my-proxy.example.com:8080', true],
  ])('validates proxy address %s', (value, valid) => {
    expect(isValidProxyUrl(value)).toBe(valid);
  });
});
