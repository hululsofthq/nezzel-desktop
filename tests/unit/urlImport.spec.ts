import { describe, expect, it } from 'vitest';
import { mergeParsedUrlImports, parseUrlFileText, parseUrlInputText } from '../../src/helpers/urlImport.ts';
import fixtures from '../../src-tauri/src/links/fixtures.json';
import { routingUrl } from '../../src/helpers/linkNormalization';
import { normalizePastedUrl, pendingUrlKey } from '../../src/helpers/url.ts';

describe('urlImport', () => {
  it.each(fixtures.filter(row => 'url' in row))('matches Rust normalization: $input', (row) => {
    expect(routingUrl(row.input).href).toBe(row.url);
    if ('key' in row) expect(pendingUrlKey(row.input)).toBe(row.key);
  });
  it.each([
    ['https://www.instagram.com/p/DEg2j6USJgo/?igsh=one&utm_source=copy', 'https://instagram.com/p/DEg2j6USJgo?igsh=two'],
    ['https://youtu.be/video123?si=one', 'https://www.youtube.com/watch?v=video123&si=two&utm_medium=share'],
    ['https://youtube.com/shorts/video123', 'https://m.youtube.com/watch?v=video123'],
    ['https://example.com/video?id=1&utm_campaign=a', 'https://example.com/video?utm_campaign=b&id=1'],
  ])('matches the same pending item despite tracking: %s', (first, second) => {
    expect(pendingUrlKey(first)).toBe(pendingUrlKey(second));
  });

  it.each([
    ['https://instagram.com/p/ABC', 'https://instagram.com/p/abc'],
    ['https://youtube.com/watch?v=one', 'https://youtube.com/watch?v=two'],
    ['https://youtube.com/watch?v=one&list=playlist1', 'https://youtube.com/watch?v=one&list=playlist2'],
    ['https://dropbox.com/scl/fi/id/a?rlkey=one', 'https://dropbox.com/scl/fi/id/a?rlkey=two'],
    ['https://example.com/video?token=one', 'https://example.com/video?token=two'],
    ['https://instagram.com.evil.net/p/ABC', 'https://instagram.com/p/ABC'],
  ])('keeps distinct identities and access keys separate: %s', (first, second) => {
    expect(pendingUrlKey(first)).not.toBe(pendingUrlKey(second));
  });
  it('preserves unchanged URLs byte for byte, including signed query values', () => {
    const url = 'https://cdn.example.com/file?signature=a\'b%2fc+%20&next=%2Fpath';
    expect(normalizePastedUrl(url)).toBe(url);
  });

  it('normalizes pasted hosts and boundary direction marks without losing share keys', () => {
    expect(normalizePastedUrl('\u200b\u2060\ufeffhttps://youtu.be/abc?si=x\ufeff\u2060\u200b')).toBe('https://youtu.be/abc?si=x');
    expect(parseUrlInputText(
      '\u200f\u2067youtube.com/watch?v=YsKxkP_kGbE&t=10s&si=abc\u2069\u200f '
      + 'HTTPS://M.YOUTUBE.COM/watch?v=YsKxkP_kGbE '
      + 'https://fxtwitter.com/ANI/status/1823253560127041999/photo/1 '
      + 'dropbox.com/scl/fi/abc/report.pdf?rlkey=secret&dl=0',
    )).toEqual({ urls: [
      'https://youtube.com/watch?v=YsKxkP_kGbE&t=10s&si=abc',
      'HTTPS://M.YOUTUBE.COM/watch?v=YsKxkP_kGbE',
      'https://fxtwitter.com/ANI/status/1823253560127041999/photo/1',
      'https://dropbox.com/scl/fi/abc/report.pdf?rlkey=secret&dl=0',
    ], skipped: 0, skipReasons: [] });
    expect(parseUrlInputText('hello youtube.com.evil.test/watch?v=abc')).toEqual({ urls: [], skipped: 2, skipReasons: ['invalid', 'invalid'] });
  });

  it.each([
    ['this is not a link', [], ['invalid', 'invalid', 'invalid', 'invalid', 'invalid']],
    ['example', [], ['invalid']],
    ['http://', [], ['invalid']],
    ['ftp://example.com/file.mp4', [], ['unsupportedScheme']],
    ['/Users/x/video.mp4', [], ['localFile']],
    ['C:\\Users\\x\\video.mp4', [], ['localFile']],
    ['file:///Users/x/video.mp4', [], ['localFile']],
    ['https://youtube.com\nexample.com', ['https://youtube.com'], ['invalid']],
    ['https://example.com/\nfoo.com', ['https://example.com/'], ['invalid']],
    ['https://www.you\ntube.com/watch?v=abc', ['https://www.youtube.com/watch?v=abc'], []],
    ['https://example.com/watch?v=\r\nabc', ['https://example.com/watch?v=abc'], []],
    ['https://example.com\n/watch?v=abc', ['https://example.com/watch?v=abc'], []],
    ['https://example.com/video\n%20name', ['https://example.com/video%20name'], []],
    ['https://example.com/a\nplain/word', ['https://example.com/a'], ['invalid']],
    ['https://example.com/a\ninvalid', ['https://example.com/a'], ['invalid']],
    ['https://example.com/a\nexample.invalid', ['https://example.com/a'], ['invalid']],
    ['https://example.com/a\n/Users/x/video.mp4', ['https://example.com/a'], ['localFile']],
    ['https://example.com/a\nwww.youtube.com', ['https://example.com/a', 'https://www.youtube.com/'], []],
    ['https://example.com/a https://example.com/a', ['https://example.com/a'], []],
  ])('parses field input %j with rejection reasons', (input, urls, skipReasons) => {
    expect(parseUrlInputText(input)).toEqual({ urls, skipped: skipReasons.length, skipReasons });
  });

  it('parses multiple input urls separated by commas, whitespace, and newlines', () => {
    const result = parseUrlInputText(
      'https://example.com/a, https://example.com/b\nhttps://example.com/c\tinvalid',
    );

    expect(result.urls).toEqual([
      'https://example.com/a',
      'https://example.com/b',
      'https://example.com/c',
    ]);
    expect(result.skipped).toBe(1);
  });

  it('recognizes a scheme only at the start, preserving nested URL query values', () => {
    expect(parseUrlInputText('youtu.be/YsKxkP_kGbE?next=https://example.com/path')).toEqual({ urls: ['https://youtu.be/YsKxkP_kGbE?next=https://example.com/path'], skipped: 0, skipReasons: [] });
  });

  it('requires public bare hosts and recognized file-share paths', () => {
    expect(parseUrlInputText('evil.test@youtube.com/x youtube.com:8080/x github.com/o/r pixeldrain.com/')).toEqual({ urls: [], skipped: 4, skipReasons: ['invalid', 'invalid', 'invalid', 'invalid'] });
    expect(parseUrlInputText('www.github.com/octocat/Hello-World/blob/master/README')).toEqual({ urls: ['https://www.github.com/octocat/Hello-World/blob/master/README'], skipped: 0, skipReasons: [] });
  });

  it('extracts urls from csv-like file content', () => {
    const result = parseUrlFileText(
      'title,url\nExample,https://example.com/watch?v=1\nTwo,https://example.com/watch?v=2',
    );

    expect(result.urls).toEqual([
      'https://example.com/watch?v=1',
      'https://example.com/watch?v=2',
    ]);
  });

  it('deduplicates merged imports', () => {
    const result = mergeParsedUrlImports([
      { urls: ['https://example.com/a', 'https://example.com/b'], skipped: 1 },
      { urls: ['https://example.com/b', 'https://example.com/c'], skipped: 2 },
    ]);

    expect(result.urls).toEqual([
      'https://example.com/a',
      'https://example.com/b',
      'https://example.com/c',
    ]);
    expect(result.skipped).toBe(3);
  });
});
