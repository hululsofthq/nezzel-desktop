//! Catch bar IPC and the boundary to U3's normal download queue.

use super::{
  candidate::Candidate,
  native,
  state::is_http,
  tabs::{RecentSave, RecentSite},
  window::{self, CatchWindows, Session, TabPage},
};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State, Url, Webview};

use crate::state::preferences_models::{Preferences, SearchEngine};

use super::handoff::{CatchFormat, SaveSelection};

/// The frame values are untrusted hints. Use the admitted media URL to select cookies from
/// the webview named by `page_label` (the Catch profile). Windows and Linux use `cookies_for_url`.
/// WKWebView needs `cookies()` filtered by media domain, path and secure flag because its
/// `cookies_for_url` drops domain cookies.
pub struct HandoffRequest {
  pub page_label: String,
  pub page_url: Url,
  pub candidate: Candidate,
  pub frame_url: Option<Url>,
  pub frame_origin: Option<String>,
  pub user_agent: Option<String>,
  page_title: Option<String>,
  /// The page's picture for the queued row (`data:` URL).
  thumbnail: Option<String>,
  prepared: Arc<tokio::sync::OnceCell<super::handoff::Handoff>>,
}

#[derive(Debug)]
pub enum HandoffError {
  DrmProtected,
  Failed(&'static str),
}

pub trait Handoff: Send + Sync {
  fn formats(
    &self,
    request: HandoffRequest,
  ) -> BoxFuture<'_, Result<Vec<CatchFormat>, HandoffError>>;
  fn save(
    &self,
    request: HandoffRequest,
    format_id: Option<String>,
  ) -> BoxFuture<'_, Result<String, HandoffError>>;
}

struct BrowserHandoff<'a>(&'a AppHandle);

impl BrowserHandoff<'_> {
  async fn prepare<'a>(
    &self,
    request: &'a HandoffRequest,
  ) -> Result<&'a super::handoff::Handoff, HandoffError> {
    use super::handoff::{self, CapturedRequest, RequestContext};
    request
      .prepared
      .get_or_try_init(|| async {
        let page = window::page(self.0, &request.page_label).map_err(HandoffError::Failed)?;
        let cookies =
          handoff::media_cookies(page, request.candidate.download_url().clone()).await?;
        let context = RequestContext {
          frame_url: request
            .frame_url
            .as_ref()
            .unwrap_or(&request.page_url)
            .to_string(),
          frame_origin: request
            .frame_origin
            .clone()
            .unwrap_or_else(|| "null".into()),
          user_agent: request
            .user_agent
            .clone()
            .filter(|agent| !agent.is_empty())
            .unwrap_or_else(|| crate::net::CHROME_UA.into()),
          cookies,
        };
        Ok(handoff::Handoff::new(CapturedRequest::new(
          &request.candidate,
          context,
        )))
      })
      .await
  }
}

impl From<super::handoff::HandoffError> for HandoffError {
  fn from(error: super::handoff::HandoffError) -> Self {
    match error {
      super::handoff::HandoffError::Protected => Self::DrmProtected,
      _ => Self::Failed(error.code()),
    }
  }
}

impl Handoff for BrowserHandoff<'_> {
  fn formats(
    &self,
    request: HandoffRequest,
  ) -> BoxFuture<'_, Result<Vec<CatchFormat>, HandoffError>> {
    Box::pin(async move { Ok(self.prepare(&request).await?.formats(self.0, None).await?) })
  }

  fn save(
    &self,
    request: HandoffRequest,
    format_id: Option<String>,
  ) -> BoxFuture<'_, Result<String, HandoffError>> {
    Box::pin(async move {
      let result = self
        .prepare(&request)
        .await?
        .save(
          self.0,
          &request.page_url,
          request.page_title.as_deref(),
          SaveSelection {
            format_id: format_id.as_deref(),
            thumbnail: request.thumbnail.clone(),
            ..Default::default()
          },
          None,
        )
        .await?;
      crate::usage::count_catch(self.0, crate::usage::keys::CATCH_SAVED, &request.page_url);
      Ok(result.download_id)
    })
  }
}

/// The retry ladder's last step: yt-dlp reads the page address itself, with the page's cookies
/// and identity. A result that is not DRM is offered as the page's video.
pub(super) async fn page_probe(
  app: &AppHandle,
  page_label: &str,
  page_url: &Url,
  user_agent: Option<String>,
) -> Result<(Candidate, super::handoff::Handoff, Vec<CatchFormat>), HandoffError> {
  use super::handoff::{self, CapturedRequest, RequestContext};
  let candidate = Candidate::page(page_url.clone());
  let page = window::page(app, page_label).map_err(HandoffError::Failed)?;
  let cookies = handoff::media_cookies(page, page_url.clone()).await?;
  let context = RequestContext {
    frame_url: candidate.frame_url.clone(),
    frame_origin: candidate.frame_origin.clone(),
    user_agent: user_agent
      .filter(|agent| !agent.is_empty())
      .unwrap_or_else(|| crate::net::CHROME_UA.into()),
    cookies,
  };
  let handoff = handoff::Handoff::new(CapturedRequest::new(&candidate, context));
  let formats = handoff.formats(app, None).await?;
  if formats.is_empty() {
    return Err(HandoffError::Failed("noMediaFound"));
  }
  Ok((candidate, handoff, formats))
}

/// Automatic catch: one candidate's hand-off (admitted request, cookies, formats), prepared
/// while its hidden page is open and kept after that page is gone.
pub(super) struct Kept(HandoffRequest);

impl Kept {
  pub(super) async fn new(app: &AppHandle, request: HandoffRequest) -> Result<Self, HandoffError> {
    BrowserHandoff(app).prepare(&request).await?;
    Ok(Self(request))
  }

  fn handoff(&self) -> Result<&super::handoff::Handoff, HandoffError> {
    self
      .0
      .prepared
      .get()
      .ok_or(HandoffError::Failed("catchCandidateUnavailable"))
  }

  pub(super) fn media_url(&self) -> &Url {
    self.0.candidate.download_url()
  }

  pub(super) fn page_url(&self) -> &Url {
    &self.0.page_url
  }

  pub(super) fn user_agent(&self) -> Option<&str> {
    self.0.user_agent.as_deref()
  }

  /// One frame of the caught video comes through the download's own request; DASH gives none.
  pub(super) fn frame_media(&self) -> Option<super::thumbnail::Media> {
    use super::candidate::Kind;
    let hls = match self.0.candidate.kind {
      Kind::Hls => true,
      Kind::Direct => false,
      _ => return None,
    };
    Some(self.handoff().ok()?.frame_media(hls))
  }

  pub(super) async fn formats(&self, app: &AppHandle) -> Result<Vec<CatchFormat>, HandoffError> {
    Ok(self.handoff()?.formats(app, None).await?)
  }

  /// Queues the caught media address; the page link only names the row.
  pub(super) async fn save(
    &self,
    app: &AppHandle,
    format_id: Option<&str>,
    download_id: String,
    thumbnail: Option<String>,
  ) -> Result<(), HandoffError> {
    self
      .handoff()?
      .save(
        app,
        &self.0.page_url,
        self.0.page_title.as_deref(),
        SaveSelection {
          format_id,
          download_id: Some(download_id),
          thumbnail,
        },
        None,
      )
      .await?;
    Ok(())
  }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Opened {
  window_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Saved {
  download_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenedTab {
  tab_id: String,
}

/// `catch_tab_new`'s options: no address opens the home page.
#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct TabOptions {
  url: Option<String>,
  private: bool,
}

/// What the shell's own layout takes of the window (logical px).
#[derive(Deserialize)]
pub(crate) struct ShellFit {
  top: f64,
  side: f64,
  overlay: bool,
  #[serde(default)]
  rtl: bool,
}

/// The home page's lists.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Home {
  recent_sites: Vec<RecentSite>,
  saved: Vec<RecentSave>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum History {
  Back,
  Forward,
  Reload,
}

#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub(crate) enum Navigation {
  History(History),
  Address { url: String },
}

pub(crate) fn bar_command(label: &str, cmd: &str) -> bool {
  window::bar_window_id(label).is_some()
    && matches!(
      cmd,
      "catch_open"
        | "catch_navigate"
        | "catch_save"
        | "catch_formats"
        | "catch_allow_popup_once"
        | "catch_forget_site"
        | "catch_deep_retry"
        | "catch_use_browser_signin"
        | "catch_try_mobile"
        | "catch_close"
        | "catch_bar_ready"
        | "catch_bar_fit"
        | "catch_show_download"
        | "catch_report"
        | "catch_tab_new"
        | "catch_tab_select"
        | "catch_tab_close"
        | "catch_shell_fit"
        | "catch_home"
        | "catch_clear_recent"
        | "capture_read_clipboard"
        | "config_get"
        | "catch_search_engine_get"
        | "catch_search_engine_set"
        | "catch_signin_off_get"
        | "catch_signin_off_set"
        | "catch_adblock_set"
    )
}

pub(super) fn owns_window(caller: &str, id: &str) -> bool {
  id.strip_prefix("catch-")
    .is_some_and(|id| window::bar_window_id(caller) == Some(id))
}

fn session(caller: &Webview, app: &AppHandle, id: &str) -> Result<Arc<Session>, &'static str> {
  if !owns_window(caller.label(), id) {
    return Err("catchWrongWindow");
  }
  app.state::<CatchWindows>().session(id)
}

/// The page of the named tab, or of the active tab without a name.
fn tab_page(
  caller: &Webview,
  app: &AppHandle,
  id: &str,
  tab: Option<&str>,
) -> Result<TabPage, &'static str> {
  if !owns_window(caller.label(), id) {
    return Err("catchWrongWindow");
  }
  app.state::<CatchWindows>().tab(id, tab)
}

fn address(raw: &str) -> Result<Url, &'static str> {
  if raw.len() > 8192 || raw.trim() != raw || raw.chars().any(char::is_control) {
    return Err("catchInvalidUrl");
  }
  Url::parse(raw)
    .ok()
    .filter(is_http)
    .ok_or("catchInvalidUrl")
}

#[tauri::command]
pub(crate) async fn catch_open(
  app: AppHandle,
  caller: Webview,
  url: Option<String>,
) -> Result<Opened, &'static str> {
  if !crate::window_commands::allowed_for_window(caller.label(), "catch_open") {
    return Err("catchWrongWindow");
  }
  Ok(Opened {
    window_id: window::open(&app, url.as_deref().map(address).transpose()?)?,
  })
}

#[tauri::command]
pub(crate) async fn catch_navigate(
  app: AppHandle,
  caller: Webview,
  window_id: String,
  action: Navigation,
  tab_id: Option<String>,
) -> Result<(), &'static str> {
  if !owns_window(caller.label(), &window_id) {
    return Err("catchWrongWindow");
  }
  // The home page and a sleeping tab have no page webview: an address gives the tab one.
  if let Navigation::Address { url } = &action {
    if window::load_tab(&app, &window_id, tab_id.as_deref(), &address(url)?)? {
      return Ok(());
    }
  }
  let TabPage { label, session, .. } = tab_page(&caller, &app, &window_id, tab_id.as_deref())?;
  let _action = session.action.lock().await;
  // After a crash or provisional failure the engine may still have the old or blank address.
  let retry_url = {
    let state = session.active()?;
    Some(state.page.url.clone())
      .filter(|url| (state.load_failed || state.page.document.crashed) && is_http(url))
  };
  let page = window::page(&app, &label)?;
  match action {
    Navigation::Address { url } => page
      .navigate(address(&url)?)
      .map_err(|_| "catchNavigationFailed"),
    Navigation::History(History::Reload) => match retry_url {
      Some(url) => page.navigate(url),
      None => page.reload(),
    }
    .map_err(|_| "catchNavigationFailed"),
    Navigation::History(history) => native::history(&page, matches!(history, History::Back)).await,
  }
}

#[tauri::command]
pub(crate) fn catch_allow_popup_once(
  app: AppHandle,
  caller: Webview,
  window_id: String,
  tab_id: Option<String>,
) -> Result<(), &'static str> {
  let tab = tab_page(&caller, &app, &window_id, tab_id.as_deref())?;
  tab.session.active()?.page.popup.allow_once = true;
  Ok(())
}

#[tauri::command]
pub(crate) async fn catch_forget_site(
  app: AppHandle,
  caller: Webview,
  window_id: String,
  tab_id: Option<String>,
) -> Result<(), &'static str> {
  let TabPage { label, session, .. } = tab_page(&caller, &app, &window_id, tab_id.as_deref())?;
  let _action = session.action.lock().await;
  drop(session.active()?);
  let page = window::page(&app, &label)?;
  let url = native::url(&page)
    .await
    .filter(is_http)
    .ok_or("catchInvalidUrl")?;
  native::forget(&page, url.clone()).await?;
  window::forget_recent(&app, Some(&url));
  // A link or redirect may have changed the page while the store was clearing.
  if native::url(&page)
    .await
    .is_some_and(|current| current == url)
  {
    page.reload().map_err(|_| "catchNavigationFailed")?;
  }
  Ok(())
}

#[tauri::command]
pub(crate) async fn catch_deep_retry(
  app: AppHandle,
  caller: Webview,
  window_id: String,
  tab_id: Option<String>,
) -> Result<(), &'static str> {
  let TabPage { label, session, .. } = tab_page(&caller, &app, &window_id, tab_id.as_deref())?;
  let _action = session.action.lock().await;
  let page = window::page(&app, &label)?;
  let deep = session.active()?.page.deep;
  if !deep {
    native::enable_deep(&page).await?;
    session.active()?.page.deep = true;
  }
  page.reload().map_err(|_| "catchNavigationFailed")
}

#[tauri::command]
pub(crate) async fn catch_close(
  app: AppHandle,
  caller: Webview,
  window_id: String,
) -> Result<(), &'static str> {
  if !owns_window(caller.label(), &window_id) {
    return Err("catchWrongWindow");
  }
  window::close(&app, &window_id).await
}

#[tauri::command]
pub(crate) async fn catch_bar_ready(
  app: AppHandle,
  caller: Webview,
  window_id: String,
) -> Result<(), &'static str> {
  if !owns_window(caller.label(), &window_id) {
    return Err("catchWrongWindow");
  }
  window::shell_ready(&app, &window_id)
}

#[tauri::command]
pub(crate) async fn catch_bar_fit(
  app: AppHandle,
  caller: Webview,
  window_id: String,
  height: f64,
  body_height: Option<f64>,
) -> Result<(), &'static str> {
  if !owns_window(caller.label(), &window_id) {
    return Err("catchWrongWindow");
  }
  window::bar_fit(&app, &window_id, height, body_height)
}

#[tauri::command]
pub(crate) async fn catch_shell_fit(
  app: AppHandle,
  caller: Webview,
  window_id: String,
  fit: ShellFit,
) -> Result<(), &'static str> {
  if !owns_window(caller.label(), &window_id) {
    return Err("catchWrongWindow");
  }
  let fit = window::Fit {
    top: fit.top,
    side: fit.side,
    overlay: fit.overlay,
    rtl: fit.rtl,
  };
  window::fit(&app, &window_id, fit)
}

#[tauri::command]
pub(crate) async fn catch_tab_new(
  app: AppHandle,
  caller: Webview,
  window_id: String,
  opts: Option<TabOptions>,
) -> Result<OpenedTab, &'static str> {
  if !owns_window(caller.label(), &window_id) {
    return Err("catchWrongWindow");
  }
  let opts = opts.unwrap_or_default();
  let url = opts
    .url
    .as_deref()
    .filter(|url| !url.is_empty())
    .map(address)
    .transpose()?;
  Ok(OpenedTab {
    tab_id: window::tab_new(&app, &window_id, url, opts.private)?,
  })
}

#[tauri::command]
pub(crate) async fn catch_tab_select(
  app: AppHandle,
  caller: Webview,
  window_id: String,
  tab_id: String,
) -> Result<(), &'static str> {
  if !owns_window(caller.label(), &window_id) {
    return Err("catchWrongWindow");
  }
  window::tab_select(&app, &window_id, &tab_id)
}

#[tauri::command]
pub(crate) async fn catch_tab_close(
  app: AppHandle,
  caller: Webview,
  window_id: String,
  tab_id: String,
) -> Result<(), &'static str> {
  if !owns_window(caller.label(), &window_id) {
    return Err("catchWrongWindow");
  }
  window::tab_close(&app, &window_id, &tab_id).await
}

#[tauri::command]
pub(crate) async fn catch_home(
  app: AppHandle,
  caller: Webview,
  window_id: String,
) -> Result<Home, &'static str> {
  if !owns_window(caller.label(), &window_id) {
    return Err("catchWrongWindow");
  }
  let (recent_sites, saved) = window::home(&app);
  Ok(Home {
    recent_sites,
    saved,
  })
}

#[tauri::command]
pub(crate) async fn catch_clear_recent(
  app: AppHandle,
  caller: Webview,
  window_id: String,
) -> Result<(), &'static str> {
  if !owns_window(caller.label(), &window_id) {
    return Err("catchWrongWindow");
  }
  window::forget_recent(&app, None);
  Ok(())
}

#[tauri::command]
pub(crate) async fn catch_show_download(
  app: AppHandle,
  caller: Webview,
  window_id: String,
  download_id: Option<String>,
) -> Result<(), &'static str> {
  if !owns_window(caller.label(), &window_id) {
    return Err("catchWrongWindow");
  }
  let download_id = match download_id {
    // A row of the home page: only a download that list holds.
    Some(id) => Some(id).filter(|id| window::saved_here(&app, id)),
    None => session(&caller, &app, &window_id)
      .ok()
      .and_then(|session| session.active().ok()?.page.document.saved.clone()),
  }
  .ok_or("catchNoSavedDownload")?;
  crate::capture::outbox::show_catch_download(&app, download_id);
  crate::background::show_main(&app);
  Ok(())
}

#[tauri::command]
pub(crate) async fn catch_report(
  app: AppHandle,
  caller: Webview,
  window_id: String,
) -> Result<(), &'static str> {
  let session = session(&caller, &app, &window_id)?;
  let shape = report_shape(&session.active()?.page.url);
  crate::capture::outbox::report_catch_page(&app, shape);
  crate::background::show_main(&app);
  Ok(())
}

fn report_shape(url: &Url) -> Option<String> {
  crate::diagnostics::link::link_for_report(url.as_str(), false)
    .map(|report| report.link.to_string())
}

pub(super) fn request(
  page_label: &str,
  state: &super::state::PageState,
  candidate_id: &str,
) -> Result<HandoffRequest, &'static str> {
  let candidate = state
    .document
    .handoff_candidate(candidate_id)
    .ok_or("catchCandidateUnavailable")?;
  let (frame_url, frame_origin) = if candidate.frame_origin == "null" {
    (None, None)
  } else {
    (
      Url::parse(&candidate.frame_url).ok(),
      Some(candidate.frame_origin.clone()),
    )
  };
  Ok(HandoffRequest {
    page_label: page_label.to_owned(),
    page_url: state.url.clone(),
    candidate: candidate.clone(),
    frame_url,
    frame_origin,
    user_agent: state.document.user_agent.clone(),
    page_title: state.title.clone(),
    thumbnail: state.document.thumbnail.clone(),
    prepared: state.document.handoff(candidate.url()),
  })
}

fn handoff_error(error: HandoffError) -> &'static str {
  match error {
    HandoffError::DrmProtected => "drmProtected",
    HandoffError::Failed(code) => code,
  }
}

#[tauri::command]
pub(crate) async fn catch_formats(
  app: AppHandle,
  caller: Webview,
  window_id: String,
  candidate_id: String,
  tab_id: Option<String>,
) -> Result<Vec<CatchFormat>, &'static str> {
  let tab = tab_page(&caller, &app, &window_id, tab_id.as_deref())?;
  formats(
    &tab.session,
    &tab.label,
    &candidate_id,
    &BrowserHandoff(&app),
  )
  .await
}

async fn formats(
  session: &Session,
  page_label: &str,
  candidate_id: &str,
  handoff: &dyn Handoff,
) -> Result<Vec<CatchFormat>, &'static str> {
  let _formats = session.formats.lock().await;
  let (generation, request) = {
    let state = session.active()?;
    let request = request(page_label, &state.page, candidate_id)?;
    if let Some(cached) = state.page.document.formats(request.candidate.url()) {
      return Ok(cached.to_vec());
    }
    (state.page.generation, request)
  };
  let url = request.candidate.url().clone();
  let prepared = request.prepared.clone();
  let result = handoff.formats(request).await;
  let mut state = session.active()?;
  if state.page.generation != generation
    || !Arc::ptr_eq(&prepared, &state.page.document.handoff(&url))
  {
    return Err("catchCandidateUnavailable");
  }
  match &result {
    Ok(formats) => state.page.document.cache_formats(&url, formats.clone()),
    Err(HandoffError::DrmProtected) => state.page.document.mark_drm(),
    Err(_) => {}
  }
  result.map_err(handoff_error)
}

#[tauri::command]
pub(crate) async fn catch_save(
  app: AppHandle,
  caller: Webview,
  window_id: String,
  candidate_id: String,
  format_id: Option<String>,
  tab_id: Option<String>,
) -> Result<Saved, &'static str> {
  let tab = tab_page(&caller, &app, &window_id, tab_id.as_deref())?;
  let note = recent_save(&tab.session, &candidate_id, format_id.as_deref());
  let saved = save(
    &tab.session,
    &tab.label,
    &candidate_id,
    format_id,
    &BrowserHandoff(&app),
  )
  .await?;
  if let Some(note) = note {
    let save = RecentSave {
      download_id: saved.download_id.clone(),
      ..note
    };
    window::remember_save(&app, save, tab.private);
  }
  Ok(saved)
}

/// The home page's row for a save: what the page and the picked format already say about it.
fn recent_save(
  session: &Session,
  candidate_id: &str,
  format_id: Option<&str>,
) -> Option<RecentSave> {
  let state = session.active().ok()?;
  let page = &state.page;
  let host = crate::usage::keys::site_of(page.url.as_str())?;
  let (duration_sec, height) = page.save_details(candidate_id);
  let format = page
    .document
    .handoff_candidate(candidate_id)
    .and_then(|candidate| page.document.formats(candidate.url()))
    .and_then(|formats| formats.iter().find(|f| Some(f.id.as_str()) == format_id));
  Some(RecentSave {
    download_id: String::new(),
    title: page.title.clone().unwrap_or_default(),
    duration_sec,
    size_bytes: format.and_then(|format| format.size_bytes),
    height: format.and_then(|format| format.height).or(height),
    host,
  })
}

async fn save(
  session: &Session,
  page_label: &str,
  candidate_id: &str,
  format_id: Option<String>,
  handoff: &dyn Handoff,
) -> Result<Saved, &'static str> {
  let _save = session.save.try_lock().map_err(|_| "catchAlreadySaving")?;
  let (generation, request) = {
    let mut state = session.active()?;
    let request = request(page_label, &state.page, candidate_id)?;
    if state.page.document.saving || state.page.document.saved.is_some() {
      return Err("catchAlreadySaving");
    }
    if state.page.saved_urls.contains(request.candidate.url()) {
      return Err("catchAlreadySaved");
    }
    state.page.document.saving = true;
    (state.page.generation, request)
  };
  let url = request.candidate.url().clone();
  let result = handoff.save(request, format_id).await;
  if let Ok(mut state) = session.active() {
    if result.is_ok() {
      state.page.remember_saved(url);
    }
    if state.page.generation == generation {
      state.page.document.saving = false;
      match &result {
        Ok(id) => state.page.document.saved = Some(id.clone()),
        Err(HandoffError::DrmProtected) => state.page.document.mark_drm(),
        Err(_) => {}
      }
    }
  }
  result
    .map(|download_id| Saved { download_id })
    .map_err(handoff_error)
}

/// The only two preferences the shell can read or change (not `preferences_get`/`_set`).
#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchChoice {
  engine: SearchEngine,
  hint_shown: bool,
}

impl SearchChoice {
  fn of(prefs: &Preferences) -> Self {
    Self {
      engine: prefs.search_engine,
      hint_shown: prefs.search_hint_shown,
    }
  }
}

fn set_search_choice(prefs: &mut Preferences, engine: Option<SearchEngine>, hint: Option<bool>) {
  if let Some(engine) = engine {
    prefs.search_engine = engine;
  }
  if let Some(hint) = hint {
    prefs.search_hint_shown = hint;
  }
}

#[tauri::command]
pub(crate) fn catch_search_engine_get(pref: State<'_, crate::SharedPreferences>) -> SearchChoice {
  SearchChoice::of(&pref.load())
}

/// An engine outside the four ids fails to deserialize, so nothing is saved.
#[tauri::command]
pub(crate) fn catch_search_engine_set(
  app: AppHandle,
  pref: State<'_, crate::SharedPreferences>,
  engine: Option<SearchEngine>,
  hint_shown: Option<bool>,
) -> Result<SearchChoice, &'static str> {
  pref
    .modify(&app, |prefs| set_search_choice(prefs, engine, hint_shown))
    .map(|prefs| SearchChoice::of(&prefs))
    .map_err(|_| "catchPreferencesFailed")
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
