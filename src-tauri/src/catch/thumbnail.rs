//! A small picture for a caught video: the player's poster, else the picture the page names for
//! itself (og:image), else one frame of the video. Every byte comes through an admitted request
//! (public addresses only, pinned, no proxy); the shell only ever gets the finished `data:` URL,
//! never a remote address. Picture addresses are never logged.

use super::candidate::normalize_url;
use crate::net;
use base64::Engine;
use image::{codecs::jpeg::JpegEncoder, ImageFormat, ImageReader};
use reqwest::{header, Url};
use std::{future::Future, io::Cursor, path::PathBuf, process::Stdio, time::Duration};
use tauri::webview::Cookie;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The most a picture may weigh before it is decoded.
pub(super) const MAX_PICTURE_BYTES: usize = 2 * 1024 * 1024;
/// One HLS segment (with its init section) or the head of a direct file.
const MAX_PIECE_BYTES: usize = 8 * 1024 * 1024;
const DIRECT_HEAD_BYTES: usize = 4 * 1024 * 1024;
const MAX_PLAYLIST_BYTES: usize = 1024 * 1024;
const WIDTH: u32 = 320;
/// The tallest a thumbnail gets (a narrow, tall picture keeps its shape inside 320×1280).
const HEIGHT: u32 = 4 * WIDTH;
/// One request, every redirect hop and the body included.
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
/// Fetching a piece of the video and taking its frame, together.
pub(super) const FRAME_CAP: Duration = Duration::from_secs(8);
/// Every source together: a stalled picture never holds the thumbnail past this.
pub(super) const TOTAL_CAP: Duration = Duration::from_secs(9);
const MAX_HOPS: usize = 3;
/// ffmpeg demuxers for an HLS segment (TS or fMP4) or a direct mp4/mov/webm head.
const VIDEO_CONTAINERS: &str = "mpegts,mov,mp4,m4a,3gp,3g2,mj2,matroska,webm";

/// What a request for a picture or a piece of the video carries.
#[derive(Clone, Debug, Default)]
pub(super) struct Headers {
  pub(super) referer: Option<String>,
  pub(super) user_agent: String,
  /// The cookies read for the media address. Each request gets only those that match it
  /// (`cookie_for`): never another host, a `Secure` cookie never over http, a path cookie only
  /// on its path.
  pub(super) cookies: Option<MediaCookies>,
}

/// Cookies the page holds for the media address, bound to its host.
#[derive(Clone, Debug)]
pub(super) struct MediaCookies {
  host: String,
  jar: Vec<Cookie<'static>>,
}

/// The caught media a frame can come from.
#[derive(Clone, Debug)]
pub(super) struct Media {
  pub(super) url: Url,
  pub(super) hls: bool,
  pub(super) headers: Headers,
}

/// Picture addresses in the order they are tried: the poster, then the page's picture.
pub(super) fn pictures(poster: Option<Url>, page: Option<Url>) -> Vec<Url> {
  let mut ordered: Vec<Url> = Vec::with_capacity(2);
  for url in [poster, page].into_iter().flatten() {
    if super::state::is_http(&url) && !ordered.contains(&url) {
      ordered.push(url);
    }
  }
  ordered
}

/// The first source that gives a picture, as a small JPEG `data:` URL: each picture in order,
/// then one frame of the video.
/// All of it within `TOTAL_CAP`.
pub(super) async fn resolve<I, IF, F, FF>(
  pictures: Vec<Url>,
  mut picture: I,
  frame: F,
) -> Option<String>
where
  I: FnMut(Url) -> IF,
  IF: Future<Output = Option<Vec<u8>>>,
  F: FnOnce() -> FF,
  FF: Future<Output = Option<Vec<u8>>>,
{
  tokio::time::timeout(TOTAL_CAP, async move {
    for url in pictures {
      if let Some(bytes) = picture(url).await {
        if let Some(data) = data_url(bytes).await {
          return Some(data);
        }
      }
    }
    data_url(frame().await?).await
  })
  .await
  .ok()?
}

async fn data_url(bytes: Vec<u8>) -> Option<String> {
  let jpeg = tokio::task::spawn_blocking(move || small_jpeg(&bytes))
    .await
    .ok()??;
  Some(format!(
    "data:image/jpeg;base64,{}",
    base64::engine::general_purpose::STANDARD.encode(jpeg)
  ))
}

/// A JPEG that fits in 320×1280 from a JPEG, PNG or WebP; anything else (SVG, HTML, a huge bitmap)
/// gives nothing.
pub(super) fn small_jpeg(bytes: &[u8]) -> Option<Vec<u8>> {
  if bytes.len() > MAX_PICTURE_BYTES {
    return None;
  }
  let mut reader = ImageReader::new(Cursor::new(bytes))
    .with_guessed_format()
    .ok()?;
  if !matches!(
    reader.format(),
    Some(ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP)
  ) {
    return None;
  }
  // A small file can still decode to a huge bitmap.
  let mut limits = image::Limits::default();
  limits.max_image_width = Some(8_000);
  limits.max_image_height = Some(8_000);
  limits.max_alloc = Some(96 * 1024 * 1024);
  reader.limits(limits);
  let picture = reader.decode().ok()?;
  let picture = if picture.width() > WIDTH || picture.height() > HEIGHT {
    picture.resize(WIDTH, HEIGHT, image::imageops::FilterType::Triangle)
  } else {
    picture
  };
  let mut jpeg = Vec::new();
  picture
    .to_rgb8()
    .write_with_encoder(JpegEncoder::new_with_quality(&mut jpeg, 78))
    .ok()?;
  Some(jpeg)
}

/// Whether a picture's answer may be read: an image type, not over the size cap.
pub(super) fn picture_response(content_type: Option<&str>, length: Option<u64>) -> bool {
  content_type.is_some_and(|ct| {
    let ct = ct.trim_start().to_ascii_lowercase();
    ct.starts_with("image/") && !ct.starts_with("image/svg")
  }) && length.is_none_or(|n| n <= MAX_PICTURE_BYTES as u64)
}

/// A picture's bytes through an admitted request, or nothing.
pub(super) async fn fetch_picture(url: Url, headers: &Headers) -> Option<Vec<u8>> {
  get(
    url,
    headers,
    "image/webp,image/png,image/jpeg;q=0.9,*/*;q=0.1",
    Read::Whole(MAX_PICTURE_BYTES),
    picture_response,
  )
  .await
}

enum Read {
  /// The whole body, refused past this many bytes.
  Whole(usize),
  /// The first bytes only (a Range request; a server that ignores it is cut off).
  Head(usize),
}

/// One GET through admitted, pinned connections; every redirect hop is admitted again. The whole
/// of it (lookups, hops, body) within `FETCH_TIMEOUT`.
async fn get(
  url: Url,
  headers: &Headers,
  accept: &str,
  read: Read,
  allows: impl Fn(Option<&str>, Option<u64>) -> bool,
) -> Option<Vec<u8>> {
  tokio::time::timeout(FETCH_TIMEOUT, get_hops(url, headers, accept, read, allows))
    .await
    .ok()?
}

fn referer_for<'a>(headers: &'a Headers, original_url: &Url, url: &Url) -> Option<&'a str> {
  headers
    .referer
    .as_deref()
    .filter(|_| original_url.origin() == url.origin())
}

async fn get_hops(
  mut url: Url,
  headers: &Headers,
  accept: &str,
  read: Read,
  allows: impl Fn(Option<&str>, Option<u64>) -> bool,
) -> Option<Vec<u8>> {
  if net::proxied() {
    return None;
  }
  let original_url = url.clone();
  for hop in 0..=MAX_HOPS {
    if !super::state::is_http(&url) {
      return None;
    }
    let host = net::host(&url)?;
    let port = url.port_or_known_default()?;
    let addresses = net::admit(host, port, net::public_address).await.ok()?;
    let client = net::pinned_builder(host, &addresses)
      .timeout(FETCH_TIMEOUT)
      .build()
      .ok()?;
    let mut request = client
      .get(url.clone())
      .header(header::USER_AGENT, headers.user_agent.as_str())
      .header(header::ACCEPT, accept);
    if let Some(referer) = referer_for(headers, &original_url, &url) {
      request = request.header(header::REFERER, referer);
    }
    if let Some(cookie) = cookie_for(headers, &url) {
      request = request.header(header::COOKIE, cookie);
    }
    if let Read::Head(bytes) = read {
      request = request.header(header::RANGE, format!("bytes=0-{}", bytes - 1));
    }
    let mut response = request.send().await.ok()?;
    let status = response.status().as_u16();
    if matches!(status, 301 | 302 | 303 | 307 | 308) {
      if hop == MAX_HOPS {
        return None;
      }
      let location = response.headers().get(header::LOCATION)?.to_str().ok()?;
      url = normalize_url(location, Some(&url))?;
      continue;
    }
    if !response.status().is_success() {
      return None;
    }
    let content_type = response
      .headers()
      .get(header::CONTENT_TYPE)
      .and_then(|value| value.to_str().ok());
    let (max, cut) = match read {
      Read::Whole(max) => (max, false),
      Read::Head(max) => (max, true),
    };
    let length = response.content_length().filter(|_| !cut);
    if !allows(content_type, length) {
      return None;
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
      if chunk.len() > max - body.len() {
        if !cut {
          return None;
        }
        body.extend_from_slice(&chunk[..max - body.len()]);
        break;
      }
      body.extend_from_slice(&chunk);
    }
    return Some(body);
  }
  None
}

async fn get_text(url: Url, headers: &Headers) -> Option<String> {
  let body = get(
    url,
    headers,
    "*/*",
    Read::Whole(MAX_PLAYLIST_BYTES),
    |_, _| true,
  )
  .await?;
  String::from_utf8(body).ok()
}

/// Video bytes: anything but text or a picture (ffmpeg's demuxer list decides the rest).
fn video_response(content_type: Option<&str>, _length: Option<u64>) -> bool {
  !content_type.is_some_and(|ct| {
    let ct = ct.trim_start().to_ascii_lowercase();
    ct.starts_with("text/") || ct.starts_with("image/")
  })
}

/// One segment of a finished HLS media playlist and the init section it needs.
#[derive(Debug, PartialEq)]
pub(super) struct Piece {
  pub(super) init: Option<Url>,
  pub(super) url: Url,
}

/// The variant with the least bandwidth of a master playlist (a frame needs no more).
pub(super) fn lowest_variant(text: &str, base: &Url) -> Option<Url> {
  let info = super::manifest::inspect(text)?;
  let variant = info
    .variants
    .iter()
    .min_by_key(|variant| variant.bandwidth.unwrap_or(u64::MAX))?;
  normalize_url(&variant.uri, Some(base))
}

/// The segment playing at `fraction` of a finished, unencrypted media playlist. A live playlist
/// (no ENDLIST), an encrypted one or byte ranges give nothing.
pub(super) fn segment_at(text: &str, base: &Url, fraction: f64) -> Option<Piece> {
  let mut init = None;
  let mut ended = false;
  let mut segments: Vec<(f64, Option<Url>, String)> = Vec::new();
  let mut pending: Option<f64> = None;
  for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
    if let Some(attributes) = line.strip_prefix("#EXT-X-KEY:") {
      if !attributes.contains("METHOD=NONE") {
        return None;
      }
    } else if line.starts_with("#EXT-X-BYTERANGE") || line.starts_with("#EXT-X-STREAM-INF") {
      return None;
    } else if let Some(attributes) = line.strip_prefix("#EXT-X-MAP:") {
      if attributes.contains("BYTERANGE") {
        return None;
      }
      let uri = attributes.split("URI=\"").nth(1)?.split('"').next()?;
      init = Some(normalize_url(uri, Some(base))?);
    } else if line == "#EXT-X-ENDLIST" {
      ended = true;
    } else if let Some(value) = line.strip_prefix("#EXTINF:") {
      let seconds = value.split(',').next()?.trim().parse::<f64>().ok()?;
      pending = Some(seconds).filter(|s| s.is_finite() && *s >= 0.0);
    } else if !line.starts_with('#') {
      segments.push((pending.take()?, init.clone(), line.to_owned()));
    }
  }
  if !ended || segments.is_empty() {
    return None;
  }
  let total: f64 = segments.iter().map(|(seconds, ..)| seconds).sum();
  let target = total * fraction.clamp(0.0, 1.0);
  let mut start = 0.0;
  let (_, init, uri) = segments
    .iter()
    .find(|(seconds, ..)| {
      start += seconds;
      start > target
    })
    .or(segments.last())?;
  Some(Piece {
    init: init.clone(),
    url: normalize_url(uri, Some(base))?,
  })
}

/// One frame of the caught video, at about a tenth of its length for HLS, through the same
/// admitted request (Referer, user agent, cookies) as its download. ffmpeg reads only stdin.
pub(super) async fn frame(ffmpeg: PathBuf, media: Media) -> Option<Vec<u8>> {
  tokio::time::timeout(FRAME_CAP, async move {
    let (bytes, seek) = if media.hls {
      (hls_piece(&media).await?, None)
    } else {
      let head = get(
        media.url.clone(),
        &media.headers,
        "*/*",
        Read::Head(DIRECT_HEAD_BYTES),
        video_response,
      )
      .await?;
      (head, Some("1"))
    };
    grab(&ffmpeg, bytes, seek).await
  })
  .await
  .ok()?
}

async fn hls_piece(media: &Media) -> Option<Vec<u8>> {
  let text = get_text(media.url.clone(), &media.headers).await?;
  let (playlist, text) = match lowest_variant(&text, &media.url) {
    Some(variant) => (variant.clone(), get_text(variant, &media.headers).await?),
    None => (media.url.clone(), text),
  };
  let piece = segment_at(&text, &playlist, 0.1)?;
  let mut bytes = match piece.init {
    Some(init) => {
      get(
        init,
        &media.headers,
        "*/*",
        Read::Whole(MAX_PIECE_BYTES),
        video_response,
      )
      .await?
    }
    None => Vec::new(),
  };
  let rest = MAX_PIECE_BYTES.checked_sub(bytes.len())?;
  bytes.extend(
    get(
      piece.url,
      &media.headers,
      "*/*",
      Read::Whole(rest),
      video_response,
    )
    .await?,
  );
  Some(bytes)
}

/// A JPEG of the first frame (after `seek`, else from the start) of `bytes`, fed on stdin.
async fn grab(ffmpeg: &std::path::Path, bytes: Vec<u8>, seek: Option<&str>) -> Option<Vec<u8>> {
  for seek in [seek, None]
    .into_iter()
    .take(if seek.is_some() { 2 } else { 1 })
  {
    let mut command = crate::runners::tool_process::async_command(ffmpeg).ok()?;
    command.args(["-hide_banner", "-loglevel", "error"]);
    if let Some(seek) = seek {
      command.args(["-ss", seek]);
    }
    // Site bytes: only real containers and no protocol but the pipe, so ffmpeg opens nothing else.
    // Both sides bounded: a crafted 2×16384 frame never scales to a huge bitmap.
    command
      .args([
        "-protocol_whitelist",
        "pipe",
        "-format_whitelist",
        VIDEO_CONTAINERS,
        "-i",
        "pipe:0",
        "-frames:v",
        "1",
        "-vf",
        "scale=w=320:h=1280:force_original_aspect_ratio=decrease:force_divisible_by=2",
        "-f",
        "image2pipe",
        "-c:v",
        "mjpeg",
        "pipe:1",
      ])
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::null())
      .kill_on_drop(true);
    let mut child = command.spawn().ok()?;
    let mut stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;
    // Fed and read in this one future: dropping it (a cap ran out) stops both and kills ffmpeg.
    let feed = async {
      // ffmpeg stops reading after its frame: a broken pipe then is expected.
      let _ = stdin.write_all(&bytes).await;
      drop(stdin);
    };
    let read = async {
      let mut jpeg = Vec::new();
      stdout
        .take(MAX_PICTURE_BYTES as u64 + 1)
        .read_to_end(&mut jpeg)
        .await
        .map(|_| jpeg)
    };
    let ((), jpeg) = tokio::join!(feed, read);
    let jpeg = jpeg.ok().filter(|jpeg| jpeg.len() <= MAX_PICTURE_BYTES);
    let Some(jpeg) = jpeg else {
      let _ = child.kill().await;
      return None;
    };
    let status = child.wait().await.ok()?;
    if status.success() && !jpeg.is_empty() {
      return Some(jpeg);
    }
  }
  None
}

/// The Referer a picture request carries: the page's origin only, as a browser sends it across
/// origins by default, so a path or query with tokens never leaves for another host.
pub(super) fn picture_referer(page: &Url) -> Option<String> {
  let origin = page.origin();
  origin
    .is_tuple()
    .then(|| format!("{}/", origin.ascii_serialization()))
}

/// The user agent a request carries: the page's own when it is a valid header, else Chrome's.
pub(super) fn user_agent(agent: Option<&str>) -> String {
  agent
    .filter(|agent| !agent.is_empty() && header::HeaderValue::from_str(agent).is_ok())
    .unwrap_or(net::CHROME_UA)
    .to_owned()
}

/// A thumbnail for a caught video: `pictures` in order (the page's origin as Referer, no
/// cookies), then one frame of `media`. `media` (it may read cookies) and the ffmpeg lookup
/// (disk reads, a checksum) run only when no picture worked.
pub(super) async fn find<M>(
  app: &tauri::AppHandle,
  pictures: Vec<Url>,
  page: &Url,
  user_agent: Option<&str>,
  media: M,
) -> Option<String>
where
  M: Future<Output = Option<Media>>,
{
  let headers = Headers {
    referer: picture_referer(page),
    user_agent: self::user_agent(user_agent),
    cookies: None,
  };
  let app = app.clone();
  resolve(
    pictures,
    |url| fetch_picture(url, &headers),
    || async move {
      let media = media.await?;
      let ffmpeg = tokio::task::spawn_blocking(move || {
        crate::binaries::binaries_manager::BinariesManager::for_app(&app).tool_path("ffmpeg")
      })
      .await
      .ok()?
      .ok()?;
      frame(ffmpeg, media).await
    },
  )
  .await
}

/// Cookies already matched to the media address, bound to its host; none gives nothing.
pub(super) fn media_cookies(media: &Url, jar: Vec<Cookie<'static>>) -> Option<MediaCookies> {
  let host = net::host(media)?.to_ascii_lowercase();
  (!jar.is_empty()).then_some(MediaCookies { host, jar })
}

/// The `Cookie` header one request may carry: only the media host's cookies that match `url`
/// itself (scheme for `Secure`, path), so a redirect or an HLS piece on http or another path
/// never gets them.
pub(super) fn cookie_for(headers: &Headers, url: &Url) -> Option<String> {
  let cookies = headers.cookies.as_ref()?;
  if !net::host(url)?.eq_ignore_ascii_case(&cookies.host) {
    return None;
  }
  let header = cookies
    .jar
    .iter()
    .filter(|cookie| super::handoff::cookie_matches_url(cookie, url))
    .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
    .collect::<Vec<_>>()
    .join("; ");
  (!header.is_empty() && header::HeaderValue::from_str(&header).is_ok()).then_some(header)
}

#[cfg(test)]
#[path = "thumbnail_tests.rs"]
mod tests;
