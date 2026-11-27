//! Private browser request context, format discovery and queue admission for caught media.

use super::{candidate::Candidate, route::Route, thumbnail};
use crate::{
  capture::preview::{self, FailReason},
  models::{
    download::{DownloadItem, FormatOptions},
    parsed::{FetchEngine, MediaKind},
    TrackType,
  },
  net,
  runners::{
    browser_cookies::{CookieAccess, CookieContext},
    template_context::TemplateContext,
    ytdlp_runner::YtdlpRunner,
  },
  scheduling::{
    dispatcher::DispatchRequest,
    download_pipeline::{DownloadEntry, DownloadRequest, DownloadSender},
    group_state::{remove_group, start_group_run},
  },
};
use reqwest::{
  header::{self, HeaderMap, HeaderValue},
  Url,
};
use serde::{Deserialize, Serialize};
use std::{
  collections::{HashMap, HashSet},
  future::Future,
  net::{IpAddr, SocketAddr},
  sync::Arc,
  time::{Duration, Instant},
};
use tauri::{webview::Cookie, AppHandle, Emitter, EventTarget, Manager};
use tokio::sync::OnceCell;

#[derive(Clone)]
pub struct RequestContext {
  pub frame_url: String,
  pub frame_origin: String,
  pub user_agent: String,
  /// Captured from the browser's cookie store for the media URL.
  pub cookies: Vec<Cookie<'static>>,
}

#[derive(Clone)]
pub struct CapturedRequest {
  url: Url,
  context: RequestContext,
  first_seen: Instant,
  protected: bool,
  /// The extractors of a page address Nezzel vetted itself; a caught address has none.
  site: Option<&'static str>,
}

impl CapturedRequest {
  pub fn new(candidate: &Candidate, context: RequestContext) -> Self {
    Self {
      url: candidate.download_url().clone(),
      context,
      first_seen: candidate.first_seen,
      protected: candidate.has_drm(),
      site: candidate.site(),
    }
  }

  fn args(&self) -> Result<Vec<String>, HandoffError> {
    request_args(&self.url, &self.context, self.site.unwrap_or("generic"))
  }

  async fn runner<'a>(&self, runner: YtdlpRunner<'a>) -> Result<YtdlpRunner<'a>, HandoffError> {
    if self.protected {
      return Err(HandoffError::Protected);
    }
    // A configured upstream proxy could resolve and connect to an unchecked IP.
    if net::proxied() {
      return Err(HandoffError::Refused);
    }
    let addresses = admit_with(&self.url, |host, port| {
      let host = host.to_owned();
      async move { net::admit(&host, port, net::public_address).await }
    })
    .await?;
    let (host, port) = destination(&self.url)?;
    let args = self.args()?;
    let route = Route::start(host.to_owned(), port, addresses)
      .await
      .map_err(|_| HandoffError::Runner)?;
    let context = CookieContext::for_app(runner.app_handle()).map_err(|_| HandoffError::Runner)?;
    let cookies = cookie_args(&context, host, &self.context)?;
    Ok(runner.with_capture_route(route, cookies).with_args(args))
  }
}

/// Supplies the newest available observation and its own browser context on a 403.
/// The caller owns frame identity. Requests from a different frame URL or origin are refused.
pub type Refresh = Arc<dyn Fn() -> Option<CapturedRequest> + Send + Sync>;

#[derive(Clone)]
pub(crate) struct QueuedCapture {
  pub(crate) request: CapturedRequest,
  pub(crate) refresh: Option<Refresh>,
  pub(crate) selector: String,
}

/// Browser request details stay in Rust for the life of this queue entry.
pub(crate) struct PageFileRequest {
  pub(crate) id: String,
  pub(crate) url: Url,
  headers: thumbnail::Headers,
}

impl PageFileRequest {
  #[cfg(test)]
  pub(crate) fn with_test_cookie(url: Url, cookie: Cookie<'static>) -> Self {
    Self::new("1".into(), url.clone(), &url, None, vec![cookie])
  }

  pub(super) fn new(
    id: String,
    url: Url,
    page_url: &Url,
    agent: Option<&str>,
    cookies: Vec<Cookie<'static>>,
  ) -> Self {
    let (referer, cookies) = if Self::same_site(page_url, &url) {
      // Same-origin requests can use the page address. A sibling host gets only its origin.
      let referer = if page_url.origin() == url.origin() {
        let mut referer = page_url.clone();
        referer.set_fragment(None);
        Some(referer.into())
      } else {
        thumbnail::picture_referer(page_url)
      };
      (referer, thumbnail::media_cookies(&url, cookies))
    } else {
      (None, None)
    };
    Self {
      id,
      url,
      headers: thumbnail::Headers {
        referer,
        user_agent: thumbnail::user_agent(agent),
        cookies,
      },
    }
  }

  fn same_site(page: &Url, file: &Url) -> bool {
    let site = |url: &Url| {
      net::host(url)
        .filter(|host| host.parse::<IpAddr>().is_err())
        .and_then(psl::domain_str)
        .map(str::to_ascii_lowercase)
    };
    super::state::is_http(page)
      && (page.scheme() != "https" || file.scheme() == "https")
      && site(page).is_some_and(|site_name| site(file).as_ref() == Some(&site_name))
  }

  pub(crate) fn headers_for(&self, url: &Url) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if self.url.origin() != url.origin() {
      return headers;
    }
    if let Ok(agent) = HeaderValue::from_str(&self.headers.user_agent) {
      headers.insert(header::USER_AGENT, agent);
    }
    for (name, value) in [
      (header::REFERER, self.headers.referer.clone()),
      (header::COOKIE, thumbnail::cookie_for(&self.headers, url)),
    ] {
      if let Some(mut value) = value.and_then(|value| HeaderValue::from_str(&value).ok()) {
        value.set_sensitive(true);
        headers.insert(name, value);
      }
    }
    headers
  }
}

pub(super) async fn save_page_file(
  app: &AppHandle,
  page: tauri::Webview,
  page_url: &Url,
  url: &Url,
  agent: Option<&str>,
) -> Result<String, HandoffError> {
  if net::proxied() || app.get_webview("main").is_none() {
    return Err(HandoffError::Refused);
  }
  let sender = app
    .try_state::<DownloadSender>()
    .ok_or(HandoffError::Runner)?;
  let cookies = if PageFileRequest::same_site(page_url, url) {
    media_cookies(page, url.clone()).await.unwrap_or_default()
  } else {
    Vec::new()
  };
  let id = uuid::Uuid::new_v4().to_string();
  let mut entry = DownloadEntry::from((
    DownloadItem {
      id: id.clone(),
      playlist_child: false,
      url: page_url.origin().ascii_serialization(),
      format: FormatOptions {
        track_type: TrackType::Both,
        height: None,
        abr: None,
        fps: None,
        audio_encoding: None,
        video_encoding: None,
        audio_track: None,
        video_track: None,
      },
      fetch_engine: Some(FetchEngine::Files),
      resolved_url: None,
      attachment_ids: None,
      attachments: vec![],
      media_kind: Some(MediaKind::File),
      subtitle_inventory: None,
      overrides: None,
      template_context: TemplateContext {
        values: HashMap::new(),
      },
    },
    id.clone(),
  ));
  entry.page_file = Some(Arc::new(PageFileRequest::new(
    id.clone(),
    url.clone(),
    page_url,
    agent,
    cookies,
  )));
  let name = crate::files::runner::prepare_page_file(app, &mut entry)
    .await
    .map_err(|error| match error {
      // The system browser may reach an address outside Nezzel's address policy.
      crate::files::Error::Address => HandoffError::Refused,
      _ => HandoffError::Runner,
    })?;
  entry.generation = start_group_run(&id);
  if sender
    .0
    .send(DispatchRequest::Pipeline(DownloadRequest::Captured(
      Box::new(entry),
    )))
    .is_err()
  {
    remove_group(&id);
    crate::files::runner::forget(&id);
    return Err(HandoffError::Runner);
  }
  Ok(name)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatchFormat {
  pub id: String,
  pub label: FormatLabel,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub height: Option<u32>,
  pub ext: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub size_bytes: Option<u64>,
  pub best: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FormatLabel {
  Video,
  Audio,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandoffError {
  Protected,
  Live,
  Forbidden,
  Refused,
  InvalidContext,
  InvalidFormat,
  NoFormats,
  Cancelled,
  Runner,
}

impl HandoffError {
  pub fn reason(self) -> FailReason {
    match self {
      Self::Protected => FailReason::Protected,
      Self::Forbidden => FailReason::Unavailable,
      Self::Live | Self::NoFormats => FailReason::Unsupported,
      Self::Refused => FailReason::Network,
      _ => FailReason::Unknown,
    }
  }

  pub(crate) fn code(self) -> &'static str {
    match self {
      Self::Protected => "drmProtected",
      Self::Live => "liveStreamUnsupported",
      Self::Forbidden => "accessForbidden403",
      Self::Refused => "networkNameResolutionFailed",
      Self::InvalidContext | Self::InvalidFormat | Self::Runner | Self::Cancelled => "unknown",
      Self::NoFormats => "noMediaFound",
    }
  }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedDownload {
  pub download_id: String,
}

/// Thumbnails of caught downloads between `enqueue` and the dispatcher's announcement.
static QUEUED_THUMBNAILS: std::sync::LazyLock<std::sync::Mutex<Vec<(String, String)>>> =
  std::sync::LazyLock::new(Default::default);
const MAX_QUEUED_THUMBNAILS: usize = 32;

fn queued_thumbnails() -> std::sync::MutexGuard<'static, Vec<(String, String)>> {
  QUEUED_THUMBNAILS
    .lock()
    .unwrap_or_else(|error| error.into_inner())
}

fn take_thumbnail(id: &str) -> Option<String> {
  let mut queued = queued_thumbnails();
  let at = queued.iter().position(|(owner, _)| owner == id)?;
  Some(queued.remove(at).1)
}

pub(crate) fn announce_download(app: &AppHandle, entry: &DownloadEntry) {
  let kind = if entry.page_file.is_some() {
    "file"
  } else if matches!(entry.format.track_type, TrackType::Audio) {
    "audio"
  } else {
    "video"
  };
  let payload = serde_json::json!({
    "id": entry.id,
    "url": entry.url,
    "title": entry.template_context.values.get("title"),
    "thumbnail": take_thumbnail(&entry.id),
    "kind": kind,
    "sizeBytes": entry.template_context.values.get("filesize").and_then(|size| size.parse::<u64>().ok()),
    "height": entry.format.height,
  });
  let _ = app.emit_to(EventTarget::webview("main"), "catch://queued", payload);
}

#[derive(Default)]
pub(super) struct SaveSelection<'a> {
  pub(super) format_id: Option<&'a str>,
  pub(super) download_id: Option<String>,
  /// The caught video's picture (`data:` URL) for its queue row.
  pub(super) thumbnail: Option<String>,
}

struct Info {
  title: Option<String>,
  formats: Vec<CatchFormat>,
}

/// Keep one hand-off per candidate in the owning catch window. Dropping the window drops its
/// format cache. Queued requests own their context and run independently of the window.
pub struct Handoff {
  request: CapturedRequest,
  cache: OnceCell<(Info, CapturedRequest)>,
}

impl Handoff {
  pub fn new(request: CapturedRequest) -> Self {
    Self {
      request,
      cache: OnceCell::new(),
    }
  }

  pub async fn formats(
    &self,
    app: &AppHandle,
    refresh: Option<&Refresh>,
  ) -> Result<Vec<CatchFormat>, HandoffError> {
    Ok(self.info(app, refresh).await?.0.formats.clone())
  }

  /// What a frame of the caught video is fetched with: the download's own Referer, agent and
  /// cookies. DASH gives none.
  pub(super) fn frame_media(&self, hls: bool) -> super::thumbnail::Media {
    let context = &self.request.context;
    super::thumbnail::Media {
      url: self.request.url.clone(),
      hls,
      headers: super::thumbnail::Headers {
        referer: Some(context.frame_url.clone()),
        user_agent: context.user_agent.clone(),
        cookies: super::thumbnail::media_cookies(&self.request.url, context.cookies.clone()),
      },
    }
  }

  /// Title from the cached probe for the caller to use when adding the normal queue row.
  pub fn title(&self) -> Option<&str> {
    self.cache.get()?.0.title.as_deref()
  }

  pub(super) async fn save(
    &self,
    app: &AppHandle,
    page_url: &Url,
    page_title: Option<&str>,
    selection: SaveSelection<'_>,
    refresh: Option<Refresh>,
  ) -> Result<SavedDownload, HandoffError> {
    let (info, request) = self.info(app, refresh.as_ref()).await?;
    let sender = app
      .try_state::<DownloadSender>()
      .ok_or(HandoffError::Runner)?;
    enqueue(
      &sender,
      info,
      request,
      page_url,
      page_title,
      selection,
      refresh.filter(|_| request.url == self.request.url),
    )
  }

  async fn info(
    &self,
    app: &AppHandle,
    refresh: Option<&Refresh>,
  ) -> Result<&(Info, CapturedRequest), HandoffError> {
    self
      .cache
      .get_or_try_init(|| async {
        let mut request = self.request.clone();
        match probe(app, &request).await {
          Err(HandoffError::Forbidden) => {
            request = refreshed(&request, refresh).ok_or(HandoffError::Forbidden)?;
            let info = probe(app, &request).await?;
            Ok((info, request))
          }
          result => Ok((result?, request)),
        }
      })
      .await
  }
}

fn enqueue(
  sender: &DownloadSender,
  info: &Info,
  request: &CapturedRequest,
  page_url: &Url,
  page_title: Option<&str>,
  selection: SaveSelection<'_>,
  refresh: Option<Refresh>,
) -> Result<SavedDownload, HandoffError> {
  let format = match selection.format_id {
    Some(id) => info.formats.iter().find(|f| f.id == id),
    None => info.formats.iter().find(|f| f.best),
  }
  .ok_or(HandoffError::InvalidFormat)?;
  let selector = selector(format);
  // The probe runs yt-dlp's generic extractor on the media URL, whose title is the playlist or
  // file name ("index-v1-a1"); the page the user caught it on names it.
  // A site's own page is named by yt-dlp: the tab title can still be the previous video's
  // right after an in-page address change.
  let page_title = || page_title.and_then(safe_title);
  let title = if request.site.is_some() {
    info.title.clone().or_else(page_title)
  } else {
    page_title().or_else(|| info.title.clone())
  };
  let id = selection
    .download_id
    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
  let mut values = HashMap::from([(
    "filesize".into(),
    format.size_bytes.unwrap_or(0).to_string(),
  )]);
  if let Some(title) = title {
    values.insert("title".into(), title.clone());
    values.insert("fulltitle".into(), title);
  }
  let mut entry = DownloadEntry::from((
    DownloadItem {
      id: id.clone(),
      playlist_child: false,
      // The signed media URL and frame Referer stay inside CapturedRequest.
      url: page_url.origin().ascii_serialization(),
      format: FormatOptions {
        track_type: if format.label == FormatLabel::Audio {
          TrackType::Audio
        } else {
          TrackType::Both
        },
        height: format.height,
        abr: None,
        fps: None,
        audio_encoding: None,
        video_encoding: None,
        audio_track: None,
        video_track: None,
      },
      fetch_engine: Some(FetchEngine::Ytdlp),
      resolved_url: None,
      attachment_ids: None,
      attachments: vec![],
      media_kind: Some(if format.label == FormatLabel::Audio {
        MediaKind::Audio
      } else {
        MediaKind::Video
      }),
      subtitle_inventory: None,
      overrides: None,
      template_context: TemplateContext { values },
    },
    id.clone(),
  ));
  entry.catch = Some(Arc::new(QueuedCapture {
    request: request.clone(),
    refresh,
    selector,
  }));
  entry.generation = start_group_run(&id);
  if let Some(thumbnail) = selection.thumbnail {
    let mut queued = queued_thumbnails();
    if queued.len() >= MAX_QUEUED_THUMBNAILS {
      queued.remove(0);
    }
    queued.push((id.clone(), thumbnail));
  }
  if sender
    .0
    .send(DispatchRequest::Pipeline(DownloadRequest::Captured(
      Box::new(entry),
    )))
    .is_err()
  {
    remove_group(&id);
    take_thumbnail(&id);
    return Err(HandoffError::Runner);
  }
  Ok(SavedDownload { download_id: id })
}

/// yt-dlp's ftp and rtmp paths connect directly and bypass the SOCKS admission route.
const ALLOWED_PROTOCOLS: [&str; 5] = ["http", "https", "m3u8_native", "m3u8", "http_dash_segments"];
const PROTOCOL_FILTER: &str = "[protocol~='^(https?|m3u8(_native)?|http_dash_segments)$']";

fn selector(format: &CatchFormat) -> String {
  let p = PROTOCOL_FILTER;
  match (format.label, format.height) {
    // Generic direct audio links have unknown codecs, which `ba` alone does not match.
    (FormatLabel::Audio, _) => format!("ba{p}/b{p}"),
    (FormatLabel::Video, Some(h)) => format!("bv*[height={h}]{p}+ba{p}/bv*[height={h}]{p}"),
    (FormatLabel::Video, None) => format!("bv*[height=?0]{p}+ba{p}/bv*[height=?0]{p}"),
  }
}

pub(crate) fn refreshed(
  request: &CapturedRequest,
  refresh: Option<&Refresh>,
) -> Option<CapturedRequest> {
  let next = refresh?()?;
  (next.first_seen > request.first_seen
    && next.url != request.url
    && next.context.frame_url == request.context.frame_url
    && next.context.frame_origin == request.context.frame_origin)
    .then_some(next)
}

/// A refused caught stream (401/403) gets one fresh catch from the same frame: true when the
/// download should run again with it. The refresh is spent either way.
pub(crate) fn retry_refused(capture: &mut QueuedCapture) -> bool {
  let refresh = capture.refresh.take();
  match refreshed(&capture.request, refresh.as_ref()) {
    Some(next) => {
      capture.request = next;
      true
    }
    None => false,
  }
}

async fn probe(app: &AppHandle, request: &CapturedRequest) -> Result<Info, HandoffError> {
  let scope = ProbeGroup(uuid::Uuid::new_v4().to_string());
  let output = request
    .runner(YtdlpRunner::new(app).with_args(["--no-plugin-dirs", "-J"]))
    .await?
    .output_with_timeout(&scope.0, Duration::from_secs(60))
    .await
    .map_err(|_| HandoffError::Runner)?
    .ok_or(HandoffError::Cancelled)?;
  if output.code != 0 {
    return Err(classify_failure(&String::from_utf8_lossy(&output.stderr)));
  }
  parse_info(&output.stdout)
}

struct ProbeGroup(String);
impl Drop for ProbeGroup {
  fn drop(&mut self) {
    remove_group(&self.0);
  }
}

pub(crate) async fn download_runner<'a>(
  runner: YtdlpRunner<'a>,
  capture: &QueuedCapture,
) -> Result<YtdlpRunner<'a>, HandoffError> {
  capture
    .request
    .runner(runner.with_args(["-f", capture.selector.as_str()]))
    .await
}

/// A credential-free probe. Every redirect gets its own admission and pinned client.
/// The address returned is the one that answered, after redirects.
pub(super) async fn fetch_manifest(mut url: Url, max_bytes: usize) -> Option<(Url, String)> {
  if net::proxied() {
    return None;
  }
  for hop in 0..=5 {
    let addresses = admit_with(&url, |host, port| {
      let host = host.to_owned();
      async move { net::admit(&host, port, net::public_address).await }
    })
    .await
    .ok()?;
    let mut response = net::pinned_builder(net::host(&url)?, &addresses)
      .build()
      .ok()?
      .get(url.clone())
      .send()
      .await
      .ok()?;
    if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) && hop < 5 {
      url = manifest_redirect(
        &url,
        response
          .headers()
          .get(reqwest::header::LOCATION)?
          .to_str()
          .ok()?,
      )?;
      continue;
    }
    if !response.status().is_success()
      || response
        .content_length()
        .is_some_and(|n| n > max_bytes as u64)
    {
      return None;
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
      if chunk.len() > max_bytes - body.len() {
        return None;
      }
      body.extend_from_slice(&chunk);
    }
    return Some((url, String::from_utf8(body).ok()?));
  }
  None
}

fn manifest_redirect(url: &Url, location: &str) -> Option<Url> {
  super::candidate::normalize_url(location, Some(url))
}

pub(super) async fn media_cookies(
  page: tauri::Webview,
  url: Url,
) -> Result<Vec<Cookie<'static>>, HandoffError> {
  // The engine's own callback: no nested run loop, no abort when the window closes mid-read.
  let mut cookies = super::native::cookies(&page, url.clone())
    .await
    .map_err(|_| HandoffError::InvalidContext)?;
  cookies.retain(|cookie| cookie_matches_url(cookie, &url));
  Ok(cookies)
}

pub(super) fn cookie_matches_url(cookie: &Cookie<'_>, url: &Url) -> bool {
  let Some((host, domain)) = net::host(url).zip(cookie.domain_original()) else {
    return false;
  };
  let bare = domain.trim_start_matches('.').to_ascii_lowercase();
  let host_matches = host.eq_ignore_ascii_case(&bare)
    || (domain.starts_with('.')
      && host
        .strip_suffix(&bare)
        .is_some_and(|prefix| prefix.ends_with('.'))
      && psl::domain_str(&bare).is_some());
  let path = url.path();
  let scope = cookie.path().filter(|p| p.starts_with('/')).unwrap_or("/");
  host_matches
    && (cookie.secure() != Some(true) || url.scheme() == "https")
    && (path == scope
      || path
        .strip_prefix(scope)
        .is_some_and(|tail| scope.ends_with('/') || tail.starts_with('/')))
}

fn destination(url: &Url) -> Result<(&str, u16), HandoffError> {
  if !matches!(url.scheme(), "http" | "https") {
    return Err(HandoffError::Refused);
  }
  let host = net::host(url).ok_or(HandoffError::Refused)?;
  let port = url.port_or_known_default().ok_or(HandoffError::Refused)?;
  Ok((host, port))
}

async fn admit_with<F>(
  url: &Url,
  lookup: impl FnOnce(&str, u16) -> F,
) -> Result<Vec<SocketAddr>, HandoffError>
where
  F: Future<Output = Result<Vec<SocketAddr>, net::Refusal>>,
{
  let (host, port) = destination(url)?;
  let answer = if let Ok(ip) = host.parse::<IpAddr>() {
    vec![SocketAddr::new(ip, port)]
  } else {
    lookup(host, port)
      .await
      .map_err(|_| HandoffError::Refused)?
  };
  net::admit_answer(answer, port, net::public_address).map_err(|_| HandoffError::Refused)
}

fn request_args(
  url: &Url,
  context: &RequestContext,
  extractors: &str,
) -> Result<Vec<String>, HandoffError> {
  let mut args: Vec<String> = [
    "--use-extractors",
    extractors,
    "--no-playlist",
    "--downloader",
    "native",
    "--no-cache-dir",
    // A film has hundreds of fragments: retry a failed one with back-off (a 403 still ends in
    // the refresh path within ~30 s), and never save a file with a gap.
    "--abort-on-unavailable-fragments",
    "--fragment-retries",
    "5",
    "--retries",
    "3",
    "--retry-sleep",
    "fragment:exp=1:8",
    "--retry-sleep",
    "http:exp=1:8",
    // An uncommon HLS variant can trigger yt-dlp's ffmpeg fallback. Only normal local
    // postprocessing may use ffmpeg here; its network protocols bypass the SOCKS route.
    "--downloader-args",
    "ffmpeg_i:-protocol_whitelist pipe",
    "--postprocessor-args",
    "ffmpeg_i:-protocol_whitelist file,pipe,concat",
    "--referer",
    &context.frame_url,
    "--user-agent",
    &context.user_agent,
  ]
  .into_iter()
  .map(str::to_owned)
  .collect();
  for value in [
    &context.frame_url,
    &context.frame_origin,
    &context.user_agent,
  ] {
    if value.is_empty() || value.starts_with('-') || value.chars().any(char::is_control) {
      return Err(HandoffError::InvalidContext);
    }
    HeaderValue::from_str(value).map_err(|_| HandoffError::InvalidContext)?;
  }
  // A player fetches its playlist and segments with `Accept: */*`. yt-dlp's default is a page
  // load's `text/html,...`, which a streaming proxy can refuse as a hotlink (401 on every segment
  // its CDN has not cached yet). A page probe (the media is the frame itself) keeps the default.
  if context.frame_url != url.as_str() {
    args.extend(["--add-header".into(), "Accept:*/*".into()]);
  }
  if context.frame_origin != "null" {
    args.extend([
      "--add-header".into(),
      format!("Origin:{}", context.frame_origin),
    ]);
  }
  args.extend(["--".into(), url.as_str().into()]);
  Ok(args)
}

fn cookie_args(
  config: &CookieContext,
  host: &str,
  context: &RequestContext,
) -> Result<CookieAccess, HandoffError> {
  CookieAccess::from_catch(config, host, &context.cookies).map_err(|_| HandoffError::InvalidContext)
}

pub(crate) fn classify_failure(stderr: &str) -> HandoffError {
  if stderr
    .lines()
    .any(|line| preview::fail_reason(line) == FailReason::Protected)
  {
    HandoffError::Protected
  } else if stderr.lines().any(refused_line) {
    HandoffError::Forbidden
  } else {
    HandoffError::Runner
  }
}

/// A final error line (yt-dlp's `ERROR:` or ffmpeg's `[http @`/`[https @`/`[hls @`) whose response
/// was 401 or 403: a stream server answers a refused token with either. Links are dropped first,
/// so a status inside a URL or a longer number is not a refusal.
fn refused_line(line: &str) -> bool {
  static LINK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\bhttps?://\S+").expect("valid link regex")
  });
  static STATUS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\bhttp(?:\s+error)?\s+40[13]\b").expect("valid status regex")
  });
  let line = line.trim_start();
  let error = line.starts_with("ERROR:")
    || ["[http @", "[https @", "[hls @"]
      .iter()
      .any(|prefix| line.starts_with(prefix));
  error && STATUS.is_match(&LINK.replace_all(line, ""))
}

#[derive(Default, Deserialize)]
struct RawFormat {
  ext: Option<String>,
  height: Option<u32>,
  vcodec: Option<String>,
  acodec: Option<String>,
  protocol: Option<String>,
  filesize: Option<f64>,
  filesize_approx: Option<f64>,
  #[serde(default)]
  has_drm: serde_json::Value,
}

#[derive(Deserialize)]
struct RawInfo {
  title: Option<String>,
  is_live: Option<bool>,
  #[serde(default)]
  formats: Option<Vec<RawFormat>>,
  #[serde(flatten)]
  direct: RawFormat,
}

fn parse_info(bytes: &[u8]) -> Result<Info, HandoffError> {
  let info: RawInfo = serde_json::from_slice(bytes).map_err(|_| HandoffError::NoFormats)?;
  if info.is_live == Some(true) {
    return Err(HandoffError::Live);
  }
  if info.direct.has_drm == true {
    return Err(HandoffError::Protected);
  }
  let mut raw = info.formats.unwrap_or_default();
  if raw.is_empty() {
    raw.push(info.direct);
  }
  let protected = raw.iter().any(|f| f.has_drm == true);
  let mut seen = HashSet::new();
  let mut formats: Vec<_> = raw
    .into_iter()
    .rev()
    .filter_map(|f| {
      let ext = f.ext?;
      if f.has_drm == true
        || !f
          .protocol
          .as_deref()
          .is_some_and(|p| ALLOWED_PROTOCOLS.contains(&p))
        || ext.is_empty()
        || !ext.bytes().all(|b| b.is_ascii_alphanumeric())
        || (f.vcodec.as_deref() == Some("none") && f.acodec.as_deref() == Some("none"))
      {
        return None;
      }
      let audio = f.vcodec.as_deref() == Some("none")
        || (f.vcodec.is_none()
          && matches!(
            ext.as_str(),
            "mp3" | "m4a" | "aac" | "flac" | "ogg" | "opus" | "wav"
          ));
      let height = f.height.filter(|h| *h > 0 && !audio);
      if !seen.insert((audio, height)) {
        return None;
      }
      Some(CatchFormat {
        id: if audio {
          "audio".into()
        } else {
          height.map_or_else(|| "video".into(), |h| format!("video-{h}"))
        },
        label: if audio {
          FormatLabel::Audio
        } else {
          FormatLabel::Video
        },
        height,
        ext,
        size_bytes: f
          .filesize
          .or(f.filesize_approx)
          .filter(|size| *size > 0.0 && *size < u64::MAX as f64)
          .map(|size| size as u64),
        best: false,
      })
    })
    .collect();
  formats.sort_by_key(|f| (f.label == FormatLabel::Audio, std::cmp::Reverse(f.height)));
  formats
    .first_mut()
    .ok_or(if protected {
      HandoffError::Protected
    } else {
      HandoffError::NoFormats
    })?
    .best = true;
  let title = info.title.as_deref().and_then(safe_title);
  Ok(Info { title, formats })
}

/// The last yt-dlp lines of a failed catch download for the local log: links (host, path and
/// query) are dropped before the usual scrub, so a signed media URL never reaches the log.
pub(crate) fn failure_detail(lines: &[String]) -> String {
  static LINK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\bhttps?://\S+").expect("valid link regex")
  });
  let text = lines.join("\n");
  crate::diagnostics::redact::scrub(
    &LINK.replace_all(&text, "[link]"),
    crate::diagnostics::redact::local_names(),
  )
  .as_str()
  .to_owned()
}

fn safe_title(title: &str) -> Option<String> {
  let title = title.trim();
  if title.is_empty() {
    return None;
  }
  Some(
    crate::diagnostics::redact::scrub(
      &title
        .chars()
        .filter(|c| !c.is_control())
        .take(512)
        .collect::<String>(),
      &[],
    )
    .as_str()
    .to_owned(),
  )
}

#[cfg(test)]
#[path = "handoff_tests.rs"]
mod tests;
