//! A local bar and a capability-free browser webview in one native window.

use super::{
  handoff,
  keys::{self, Shortcut},
  layering, manifest, native,
  state::{is_http, BrowserStatus, CatchState, CatchTab, PageState, Retry, TabPlace},
  tabs::{LinkPlace, NewTab, RecentSave, RecentSaves, RecentSite, RecentSites, Store, Tab, Tabs},
};
use futures::{future::BoxFuture, stream::FuturesUnordered, StreamExt};
use std::{
  collections::HashMap,
  fs::OpenOptions,
  sync::{Arc, Mutex},
  time::{Duration, Instant},
};
use tauri::{
  webview::{DownloadEvent, NewWindowResponse, PageLoadEvent, PermissionKind, PermissionResponse},
  window::Color,
  AppHandle, Emitter, EventTarget, LogicalPosition, LogicalSize, Manager, Theme, Url, Webview,
  WebviewBuilder, WebviewUrl, WebviewWindow, Window, WindowBuilder, WindowEvent,
};
use tokio::{sync::oneshot, task::AbortHandle};

const HOOKS: &str = include_str!("hooks.js");

fn capture_script(autoplay: bool) -> String {
  let (high, low) = uuid::Uuid::new_v4().as_u64_pair();
  let autoplay_document = if autoplay { "document" } else { "undefined" };
  format!(
    "(function(relayKey,autoplayDocument){{\n'use strict';\n{HOOKS}\n}})({{document,key:[{high}n,{low}n]}},{autoplay_document});"
  )
}

pub(crate) const LINK_TRACE_TARGET: &str = "catch::link_trace";
const BAR_HEIGHT: f64 = 112.0;
const MAX_ACTIVE_PROBES: usize = 2;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const EVENT: &str = "catch://state";
/// What a key pressed in a page asks of the shell: `"address"` (put the keyboard in the address
/// field) or `"tabLimit"` (say that no more tabs fit). A link from the main window that finds
/// six tabs sends `{ "tabLimit": <link> }`.
const SHORTCUT_EVENT: &str = "catch://shortcut";
const FILE_DOWNLOAD_EVENT: &str = "catch://file-download";

/// U3 supplies a bounded, SSRF-safe fetch with fresh admission at every redirect.
/// Treat observations as fully untrusted. This background probe sends no credentials.
pub(crate) trait ManifestFetcher: Send + Sync {
  /// The text and the address it came from, after any redirects.
  fn fetch(&self, url: Url) -> BoxFuture<'_, Option<(Url, String)>>;
}

struct PinnedManifestFetcher;

impl ManifestFetcher for PinnedManifestFetcher {
  fn fetch(&self, url: Url) -> BoxFuture<'_, Option<(Url, String)>> {
    Box::pin(handoff::fetch_manifest(url, MAX_MANIFEST_BYTES))
  }
}

/// Poll faster while a page loads or plays, slower while it sits, slowest on a blank page.
const TICK_BUSY: Duration = Duration::from_secs(1);
const TICK_QUIET: Duration = Duration::from_millis(2500);
const TICK_BLANK: Duration = Duration::from_secs(3);

/// A tab that is not shown is read slowly: enough for its "found" dot and its sleep clock.
const TICK_BACKGROUND: Duration = Duration::from_secs(5);

pub(super) fn background_interval(playing: bool) -> Duration {
  if playing {
    TICK_QUIET
  } else {
    TICK_BACKGROUND
  }
}

pub(super) fn tick_interval(web: bool, loading: bool, playing: bool) -> Duration {
  if !web {
    TICK_BLANK
  } else if loading || playing {
    TICK_BUSY
  } else {
    TICK_QUIET
  }
}

/// What to do when the page's renderer process dies.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Recovery {
  /// Reload the page after this pause.
  Reload(Duration),
  /// It died again soon after the last reload: stop and let the user reload from the bar.
  Stop,
}

/// Reload once with a pause; a second crash within a minute stops (no reload loop).
#[derive(Default)]
pub(super) struct CrashRecovery {
  last: Option<Instant>,
}

impl CrashRecovery {
  const BACKOFF: Duration = Duration::from_secs(1);
  const WINDOW: Duration = Duration::from_secs(60);

  pub(super) fn crashed(&mut self, now: Instant) -> Recovery {
    let repeat = self
      .last
      .is_some_and(|last| now.saturating_duration_since(last) < Self::WINDOW);
    self.last = Some(now);
    if repeat {
      Recovery::Stop
    } else {
      Recovery::Reload(Self::BACKOFF)
    }
  }
}

#[derive(Default)]
pub(crate) struct CatchWindows {
  /// Every live page by its webview label: a tab's page (`catch-<id>-t<n>`) or a hidden page
  /// (`catch-<id>`).
  sessions: Mutex<HashMap<String, Arc<Session>>>,
  browsers: Mutex<HashMap<String, Arc<Browser>>>,
  /// One closed window kept hidden for the next open: no cold shell boot.
  idle: Mutex<Option<(String, Arc<Browser>)>>,
  /// The home page's recent sites, read from disk on first use.
  recent: Mutex<Option<RecentSites>>,
  saves: Mutex<RecentSaves>,
  /// Held while `open` picks or makes the window: two opens at once never make two windows.
  opening: Mutex<()>,
}

impl CatchWindows {
  fn remove(&self, id: &str) -> Option<Arc<Session>> {
    self.sessions.lock().ok()?.remove(id)
  }

  fn insert(&self, id: &str, session: Arc<Session>) -> Result<(), &'static str> {
    self
      .sessions
      .lock()
      .map_err(|_| "catchOpenFailed")?
      .insert(id.to_owned(), session);
    Ok(())
  }

  pub(super) fn browser(&self, id: &str) -> Result<Arc<Browser>, &'static str> {
    self
      .browsers
      .lock()
      .ok()
      .and_then(|browsers| browsers.get(id).cloned())
      .ok_or("catchWindowClosed")
  }

  /// The Nezzel browser window on screen (a parked one is not listed): the app keeps one.
  fn shown(&self) -> Option<(String, Arc<Browser>)> {
    let open: Vec<_> = self
      .browsers
      .lock()
      .ok()?
      .iter()
      .map(|(id, browser)| (id.clone(), browser.clone()))
      .collect();
    open
      .into_iter()
      .find(|(_, browser)| browser.active().is_ok())
  }

  fn remove_browser(&self, id: &str) -> Option<Arc<Browser>> {
    self.browsers.lock().ok()?.remove(id)
  }

  /// Keeps a closed window for reuse when the slot is free.
  fn park(&self, id: &str, browser: Arc<Browser>) -> bool {
    let Ok(mut idle) = self.idle.lock() else {
      return false;
    };
    if idle.is_some() {
      return false;
    }
    *idle = Some((id.to_owned(), browser));
    true
  }

  fn take_idle(&self) -> Option<(String, Arc<Browser>)> {
    self.idle.lock().ok()?.take()
  }

  fn forget_idle(&self, id: &str) {
    if let Ok(mut idle) = self.idle.lock() {
      if idle.as_ref().is_some_and(|(parked, _)| parked == id) {
        *idle = None;
      }
    }
  }

  /// The page a command means: a window id names its active tab's page (`catchNoPage` on the
  /// home page), a page label names that page.
  pub(super) fn session(&self, id: &str) -> Result<Arc<Session>, &'static str> {
    match self.browser(id) {
      Ok(browser) => browser.active()?.active_page().ok_or("catchNoPage"),
      Err(_) => self.page_session(id),
    }
  }

  /// The page a tab command means: the named tab's, else the active tab's. `catchNoPage` when
  /// that tab shows the home page or sleeps.
  pub(super) fn tab(&self, id: &str, tab: Option<&str>) -> Result<TabPage, &'static str> {
    let browser = self.browser(id)?;
    let state = browser.active()?;
    let tab = state
      .tabs
      .get(tab.unwrap_or(state.tabs.active_id()))
      .ok_or("catchTabClosed")?;
    Ok(TabPage {
      label: tab_label(id, &tab.id),
      session: state.pages.get(&tab.id).cloned().ok_or("catchNoPage")?,
      private: tab.private(),
    })
  }

  fn page_session(&self, label: &str) -> Result<Arc<Session>, &'static str> {
    self
      .sessions
      .lock()
      .ok()
      .and_then(|sessions| sessions.get(label).cloned())
      .ok_or("catchWindowClosed")
  }
}

/// One tab's page, as a command needs it.
pub(super) struct TabPage {
  /// The page webview's label: `window::page` and the hand-off read cookies from this one.
  pub(super) label: String,
  pub(super) session: Arc<Session>,
  pub(super) private: bool,
}

pub(super) struct Session {
  pub(super) state: Mutex<SessionState>,
  pub(super) action: tokio::sync::Mutex<()>,
  // Serialize a destructive hook snapshot with applying its observations.
  drain: tokio::sync::Mutex<()>,
  pub(super) formats: tokio::sync::Mutex<()>,
  pub(super) save: tokio::sync::Mutex<()>,
  /// Held while a browser sign-in import runs: a second request answers at once.
  pub(super) signin: tokio::sync::Mutex<()>,
}

pub(super) struct SessionState {
  pub(super) page: PageState,
  pub(super) closed: bool,
  /// A private tab's page: site tools never copy a browser sign-in into it, nor offer to.
  pub(super) private: bool,
  /// The page presents the iPhone identity (set by `site_tools::catch_try_mobile`).
  pub(super) mobile: bool,
  /// A tab's page (never a hidden session): only these get the sign-in offer.
  shown: bool,
  loading: bool,
  pub(super) load_failed: bool,
  blank_loaded: Option<oneshot::Sender<()>>,
  last_browser_status: BrowserStatus,
  used: bool,
  /// A tab of a window already in use: its first load is not a new use of the window.
  quiet: bool,
  /// A tab that is not shown: read slowly.
  background: bool,
  /// A pop-up opened as a tab: it closes when its page closes itself.
  linked: bool,
  /// "Allow once" holds the engine's pop-up gate open for this page.
  gate_open: bool,
  /// The page's window was opened with the Settings proxy on: nothing is fetched beside the
  /// page (`BrowserProxy::fetches_beside`), whatever the setting says by now.
  proxied: bool,
  /// Windows: whether a click caused each pop-up request not answered yet, by its address.
  pub(super) clicks: std::collections::VecDeque<(String, bool)>,
  /// When this page last opened a pop-up, and its address (`popup::tab_under`).
  pub(super) popup_opened: Option<(std::time::Instant, Url)>,
  /// When this page last handed off a file download.
  download_handed: Option<std::time::Instant>,
  drain: Option<AbortHandle>,
  popups: Vec<WebviewWindow>,
  crash: CrashRecovery,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum FileDownloadNotice {
  Saving { name: Option<String> },
  Opened { name: Option<String> },
  Failed { name: Option<String> },
  Unsupported,
  Private,
  Proxied,
}

impl SessionState {
  /// Whether this page may be offered the browser's sign-in: a tab the user sees, never a
  /// private one (nothing of the user's browser goes into a private tab).
  fn offers_signin(&self) -> bool {
    self.shown && !self.private
  }

  /// Only a visible, unproxied, non-private tab can hand off a file download.
  fn hands_off_download(&mut self, url: &Url, now: Instant) -> Option<FileDownloadNotice> {
    if !self.shown || self.background {
      return None;
    }
    if self.private {
      return Some(FileDownloadNotice::Private);
    }
    if self.proxied {
      return Some(FileDownloadNotice::Proxied);
    }
    if !is_http(url) {
      return Some(FileDownloadNotice::Unsupported);
    }
    if self
      .download_handed
      .is_some_and(|at| now.saturating_duration_since(at) < Duration::from_secs(5))
    {
      return None;
    }
    self.download_handed = Some(now);
    let name = url
      .path_segments()
      .and_then(|mut parts| parts.next_back())
      .and_then(|segment| {
        percent_encoding::percent_decode_str(segment)
          .decode_utf8()
          .ok()
      })
      .map(|decoded| decoded.trim().to_owned())
      .filter(|name| !name.is_empty() && !name.chars().any(char::is_control));
    Some(FileDownloadNotice::Saving { name })
  }
}

/// A policy refusal can fall back to the system browser. Other errors produce a shell notice.
fn after_page_file(
  saved: Result<String, handoff::HandoffError>,
  name: Option<String>,
) -> Result<FileDownloadNotice, Option<String>> {
  match saved {
    Ok(saved) => Ok(FileDownloadNotice::Saving { name: Some(saved) }),
    Err(handoff::HandoffError::Refused) => Err(name),
    Err(_) => Ok(FileDownloadNotice::Failed { name }),
  }
}

impl Session {
  pub(super) fn new(url: Url) -> Self {
    let loading = is_http(&url);
    Self {
      state: Mutex::new(SessionState {
        page: PageState::new(url),
        closed: false,
        private: false,
        mobile: false,
        shown: false,
        loading,
        load_failed: false,
        blank_loaded: None,
        quiet: false,
        background: false,
        linked: false,
        gate_open: false,
        proxied: false,
        clicks: std::collections::VecDeque::new(),
        popup_opened: None,
        download_handed: None,
        last_browser_status: BrowserStatus {
          loading,
          ..Default::default()
        },
        used: false,
        drain: None,
        popups: Vec::new(),
        crash: CrashRecovery::default(),
      }),
      action: tokio::sync::Mutex::new(()),
      drain: tokio::sync::Mutex::new(()),
      formats: tokio::sync::Mutex::new(()),
      save: tokio::sync::Mutex::new(()),
      signin: tokio::sync::Mutex::new(()),
    }
  }

  /// The page of a tab the user sees; `private`: the tab keeps its own throwaway store.
  fn tab(url: Url, private: bool) -> Self {
    let session = Self::new(url);
    if let Ok(mut state) = session.state.lock() {
      state.shown = true;
      state.private = private;
      state.page.offers_page = true;
      state.page.offer_page();
    }
    session
  }

  pub(super) fn active(&self) -> Result<std::sync::MutexGuard<'_, SessionState>, &'static str> {
    let state = self.state.lock().map_err(|_| "catchWindowClosed")?;
    if state.closed {
      Err("catchWindowClosed")
    } else {
      Ok(state)
    }
  }

  pub(super) fn stop(&self) -> Vec<WebviewWindow> {
    let Ok(mut state) = self.state.lock() else {
      return Vec::new();
    };
    state.closed = true;
    state.page.navigated(blank());
    state.page.reset_delivery();
    state.page.saved_urls.clear();
    if let Some(drain) = state.drain.take() {
      drain.abort();
    }
    std::mem::take(&mut state.popups)
  }

  /// A page that must not be put to sleep now: it plays, saves or reads formats.
  fn busy(&self) -> bool {
    self.state.lock().map_or(true, |state| {
      state.page.document.playing || state.page.document.saving
    }) || self.save.try_lock().is_err()
      || self.formats.try_lock().is_err()
  }
  /// Loads the blank page with `navigate` and waits until it finished: playback has stopped and
  /// no earlier load is still on its way. A blank load left over from an earlier close does not count.
  async fn blanked(&self, navigate: impl FnOnce() -> bool) -> bool {
    let (send, receive) = oneshot::channel();
    {
      let Ok(mut state) = self.state.lock() else {
        return false;
      };
      state.blank_loaded = Some(send);
    }
    let loaded = navigate()
      && tokio::time::timeout(Duration::from_millis(400), receive)
        .await
        .is_ok_and(|result| result.is_ok());
    if let Ok(mut state) = self.state.lock() {
      state.blank_loaded = None;
    }
    loaded
  }

  /// A parked window opens again: a fresh page state, a new generation so late results of
  /// the previous use never land, and the bar back at its body (an overlay open at close is gone).
  fn reopen(&self, url: Url, first_run: bool) -> Result<(), &'static str> {
    let mut state = self.state.lock().map_err(|_| "catchOpenFailed")?;
    let loading = is_http(&url);
    let generation = state.page.generation.wrapping_add(1);
    state.page = PageState::new(url);
    state.page.generation = generation;
    state.page.first_run = first_run;
    state.closed = false;
    state.private = false;
    state.mobile = false;
    state.loading = loading;
    state.load_failed = false;
    state.last_browser_status = BrowserStatus {
      loading,
      ..Default::default()
    };
    state.used = false;
    state.crash = CrashRecovery::default();
    Ok(())
  }
}

fn bar_label(id: &str) -> String {
  format!("catch-bar-{}", id.strip_prefix("catch-").unwrap_or(id))
}

fn tab_label(id: &str, tab: &str) -> String {
  format!("{id}-{tab}")
}

/// The window a page label belongs to (the label itself when it is not a page's).
pub(super) fn window_of(label: &str) -> &str {
  split_label(label).map_or(label, |(id, _)| id)
}

/// Splits a page label into its window id and, for a tab's page, the tab id.
fn split_label(label: &str) -> Option<(&str, Option<&str>)> {
  let (id, rest) = label.split_at_checked(38)?;
  if !id.strip_prefix("catch-").is_some_and(is_id_suffix) {
    return None;
  }
  if rest.is_empty() {
    return Some((id, None));
  }
  let number = rest.strip_prefix("-t")?;
  (!number.is_empty() && number.len() <= 9 && number.bytes().all(|b| b.is_ascii_digit()))
    .then(|| (id, Some(&rest[1..])))
}

/// What the shell reports about its own layout (logical px).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Fit {
  /// Height of the address and found rows.
  pub(super) top: f64,
  /// Width of the side list, on the start side.
  pub(super) side: f64,
  /// A menu, picker or prompt draws over the page and takes the clicks there.
  pub(super) overlay: bool,
  pub(super) rtl: bool,
}

impl Default for Fit {
  fn default() -> Self {
    Self {
      top: BAR_HEIGHT,
      side: 0.,
      overlay: false,
      rtl: false,
    }
  }
}

/// Where page webviews sit (x, y, width, height): under the top rows and beside the side list,
/// which is on the left, or on the right in a right-to-left language.
pub(super) fn content_rect(width: f64, height: f64, fit: Fit) -> (f64, f64, f64, f64) {
  let top = fit.top.clamp(0., (height - 1.).max(0.));
  let side = fit.side.clamp(0., (width - 1.).max(0.));
  (
    if fit.rtl { 0. } else { side },
    top,
    (width - side).max(1.),
    (height - top).max(1.),
  )
}

/// One Nezzel browser window: its tab table and the page of every tab that has one.
/// Lock order: a browser's state before a page's, never the reverse; neither across a webview call.
pub(super) struct Browser {
  pub(super) state: Mutex<BrowserState>,
}

pub(super) struct BrowserState {
  pub(super) tabs: Tabs,
  pages: HashMap<String, Arc<Session>>,
  /// Tabs whose page is being built: one build per tab (`claim_page`).
  building: std::collections::HashSet<String>,
  /// What the shell gets while the active tab is the home page.
  home: PageState,
  shell_loaded: bool,
  fit: Fit,
  last: Option<CatchState>,
  closed: bool,
  /// Some tab loaded a page: see `reusable`.
  had_page: bool,
  /// The Settings proxy when the window opened: every page of the window gets it (`proxy`).
  proxy: crate::net::BrowserProxy,
}

impl Browser {
  fn new(first_run: bool) -> Self {
    let mut home = PageState::new(blank());
    home.first_run = first_run;
    Self {
      state: Mutex::new(BrowserState {
        tabs: Tabs::new(String::new()),
        pages: HashMap::new(),
        building: Default::default(),
        home,
        shell_loaded: false,
        fit: Fit::default(),
        last: None,
        closed: false,
        had_page: false,
        proxy: crate::net::browser_proxy(),
      }),
    }
  }

  pub(super) fn active(&self) -> Result<std::sync::MutexGuard<'_, BrowserState>, &'static str> {
    let state = self.state.lock().map_err(|_| "catchWindowClosed")?;
    if state.closed {
      Err("catchWindowClosed")
    } else {
      Ok(state)
    }
  }
}

/// What a closed window leaves to stop: its pages and the private stores to delete after them.
#[derive(Default)]
pub(super) struct Shut {
  pages: Vec<(String, Arc<Session>)>,
  stores: Vec<String>,
}

impl BrowserState {
  /// Reserves a tab's page slot for one build. False when the tab has a page or another build
  /// holds the slot: two commands at once never build two webviews under one label.
  fn claim_page(&mut self, tab: &str) -> bool {
    !self.pages.contains_key(tab) && self.building.insert(tab.to_owned())
  }

  /// Whether a pop-up's page may still move into its tab: the tab is open and has no page.
  fn may_adopt(&self, tab: &str) -> bool {
    self.tabs.get(tab).is_some() && !self.has_page(tab)
  }

  /// Whether a tab has a page or one is being built.
  fn has_page(&self, tab: &str) -> bool {
    self.pages.contains_key(tab) || self.building.contains(tab)
  }

  /// Closes the window's state: no page, build or pop-up adoption gets in after this. Returns
  /// its pages and every private store of its tabs (forgotten on close, contract §Tabs).
  fn shut(&mut self) -> Shut {
    self.closed = true;
    self.building.clear();
    let mut stores: Vec<String> = Vec::new();
    for tab in self.tabs.iter() {
      if let Store::Private(store) = &tab.store {
        if !stores.contains(store) {
          stores.push(store.clone());
        }
      }
    }
    Shut {
      pages: self.pages.drain().collect(),
      stores,
    }
  }

  fn active_page(&self) -> Option<Arc<Session>> {
    self.pages.get(self.tabs.active_id()).cloned()
  }

  /// Whether some page of the window may open a pop-up without a click right now.
  fn gate_open(&self) -> bool {
    self
      .pages
      .values()
      .any(|page| page.state.lock().is_ok_and(|page| page.gate_open))
  }

  /// Copies each page's address and title into the tab table (what a sleeping tab keeps) and
  /// tells every page whether it is shown. Returns the pages to add to the recent sites.
  fn sync(&mut self) -> Vec<(Url, Option<String>)> {
    let active = self.tabs.active_id().to_owned();
    let mut visited = Vec::new();
    for (id, session) in &self.pages {
      let (url, title, failed) = {
        let Ok(mut page) = session.state.lock() else {
          continue;
        };
        page.background = *id != active;
        (
          page.page.url.clone(),
          page.page.title.clone(),
          page.load_failed,
        )
      };
      let Some(tab) = self.tabs.get(id) else {
        continue;
      };
      if is_http(&url) && (tab.url != url.as_str() || tab.title != title) {
        if !tab.private() && !failed {
          visited.push((url.clone(), title.clone()));
        }
        self.tabs.set_page(id, url.into(), title);
      }
    }
    visited
  }

  /// The window's state: the active tab's fields and one row per tab.
  fn snapshot(&self, id: &str) -> CatchState {
    let active = self.tabs.active_id();
    let mut shown = None;
    let rows = self
      .tabs
      .iter()
      .map(|tab| {
        let page = self.pages.get(&tab.id).and_then(|session| {
          let page = session.state.lock().ok()?;
          (!page.closed).then(|| {
            let state = page.page.snapshot(id, page.last_browser_status);
            let state = if page.load_failed {
              state.with_load_failure()
            } else {
              state
            };
            (state, page.mobile)
          })
        });
        let place = TabPlace {
          id: &tab.id,
          private: tab.private(),
          opener: tab.opener.as_deref(),
          asleep: tab.asleep,
          // The identity lives in the page's webview: a home or sleeping tab has none.
          mobile: page.as_ref().is_some_and(|(_, mobile)| *mobile),
        };
        match page.map(|(state, _)| state) {
          Some(state) => {
            let row = CatchTab::live(place, &state);
            if tab.id == active {
              shown = Some(state);
            }
            row
          }
          None => CatchTab::idle(place, &tab.url, tab.title.as_deref()),
        }
      })
      .collect();
    shown
      .unwrap_or_else(|| self.home.snapshot(id, BrowserStatus::default()))
      .with_tabs(rows, active)
  }

  /// The state to send, when it differs from the last one sent.
  fn changed(&mut self, id: &str) -> Option<CatchState> {
    let state = self.snapshot(id);
    if self.last.as_ref() == Some(&state) {
      return None;
    }
    self.last = Some(state.clone());
    Some(state)
  }

  /// Whether this background tab goes to sleep now. A private tab never does (its store would be
  /// lost), and a tab that plays or saves starts its wait again.
  fn due_sleep(&mut self, tab: &str, now: tokio::time::Instant) -> bool {
    if !self.tabs.sleepy(now).iter().any(|id| id == tab) {
      return false;
    }
    // A sleeping page loses `window.opener`: neither side of a linked pair sleeps.
    let linked = |t: &Tab| t.id == tab && t.opener.is_some() || t.opener.as_deref() == Some(tab);
    let busy = self.tabs.get(tab).is_none_or(Tab::private)
      || self.tabs.iter().any(linked)
      || self.pages.get(tab).is_none_or(|page| page.busy());
    if busy {
      self.tabs.keep_awake(tab, now);
    }
    !busy
  }

  /// Whether a closing window may be kept for the next open: only one that never loaded a page.
  /// No engine can clear a webview's back/forward list, a deep-mode script stays for good, and a
  /// save still running would land in the next use; a window without any page has none of these.
  fn reusable(&self) -> bool {
    !self.had_page && self.pages.is_empty() && self.tabs.len() == 1 && self.tabs.active().home()
  }

  /// A parked window starts over: a fresh home tab (tab ids are never used twice), no overlay
  /// left open, and the whole state sent again.
  fn reopen(&mut self, first_run: bool) {
    let old = self.tabs.active_id().to_owned();
    let _ = self.tabs.close(&old);
    self.home.first_run = first_run;
    self.fit.overlay = false;
    self.last = None;
    self.closed = false;
    // A parked window has no page left: it takes the setting as it is now.
    self.proxy = crate::net::browser_proxy();
  }
}

/// Sends the shell what changed since its last state, and keeps the recent sites.
fn publish(app: &AppHandle, id: &str, browser: &Browser) {
  let visited = {
    let Ok(mut state) = browser.active() else {
      return;
    };
    let visited = state.sync();
    if state.shell_loaded {
      if let Some(changed) = state.changed(id) {
        let _ = app.emit_to(EventTarget::webview(bar_label(id)), EVENT, changed);
      }
    }
    visited
  };
  for (url, title) in visited {
    recent(app, |sites| {
      (sites.record(&url, title.as_deref(), false), ())
    });
  }
}

/// A page changed: tell its window's shell, and put the tab to sleep when it is due.
/// Pages outside a browser window (automatic catch) have no shell.
pub(super) fn notify(app: &AppHandle, label: &str) {
  let Some((id, Some(tab))) = split_label(label) else {
    return;
  };
  let Ok(browser) = app.state::<CatchWindows>().browser(id) else {
    return;
  };
  publish(app, id, &browser);
  let due = browser
    .active()
    .is_ok_and(|mut state| state.due_sleep(tab, tokio::time::Instant::now()));
  if due {
    let (app, id, tab) = (app.clone(), id.to_owned(), tab.to_owned());
    tauri::async_runtime::spawn(async move { sleep_tab(&app, &id, &tab).await });
  }
}

/// Drops a background tab's webview; the tab keeps its address and title and loads again when shown.
async fn sleep_tab(app: &AppHandle, id: &str, tab: &str) {
  let Ok(browser) = app.state::<CatchWindows>().browser(id) else {
    return;
  };
  let session = {
    let Ok(mut state) = browser.active() else {
      return;
    };
    if state.tabs.active_id() == tab {
      return;
    }
    state.tabs.set_asleep(tab, true);
    state.pages.remove(tab)
  };
  if let Some(session) = session {
    drop_page(app, tab_label(id, tab), &session).await;
  }
  publish(app, id, &browser);
}

/// Stops a page and closes its webview; the blank page stops playback first.
async fn drop_page(app: &AppHandle, label: String, session: &Session) {
  app.state::<CatchWindows>().remove(&label);
  // The identity a page presents goes with its webview: the tab's next page starts as desktop.
  native::forget_agent(&label);
  for popup in session.stop() {
    let _ = popup.destroy();
  }
  let linked = session.state.lock().is_ok_and(|state| state.linked);
  if let Some(page) = app.get_webview(&label) {
    let _ = page.hide();
    if linked && cfg!(target_os = "macos") {
      // macOS keeps a dropped webview alive: only this makes `closed` true for its opener.
      let _ = page.eval("window.close()");
    }
    session.blanked(|| page.navigate(blank()).is_ok()).await;
    let _ = page.close();
  }
}

/// The shown Nezzel browser's tabs as the hidden sign-in copy sees them (page label, private,
/// asleep or without a page): it reads the saved profile through a normal tab only.
pub(super) fn profile_tabs(app: &AppHandle) -> Vec<super::hidden_signin::TabSource<String>> {
  let Some((id, browser)) = app.state::<CatchWindows>().shown() else {
    return Vec::new();
  };
  let Ok(state) = browser.active() else {
    return Vec::new();
  };
  state
    .tabs
    .iter()
    .map(|tab| {
      let page = state.pages.get(&tab.id);
      // Lock order: the browser's state, then a page's.
      let page_private = page.is_some_and(|page| page.state.lock().map_or(true, |p| p.private));
      super::hidden_signin::TabSource {
        page: tab_label(&id, &tab.id),
        private: tab.private() || page_private,
        asleep: tab.asleep || page.is_none(),
      }
    })
    .collect()
}

/// The recent sites, read from disk once; `edit` says whether to write them back.
fn recent<T>(app: &AppHandle, edit: impl FnOnce(&mut RecentSites) -> (bool, T)) -> Option<T> {
  let registry = app.state::<CatchWindows>();
  let mut recent = registry.recent.lock().ok()?;
  let file = app.path().app_data_dir().ok()?.join("catch-recent.json");
  let sites = recent.get_or_insert_with(|| {
    std::fs::read_to_string(&file)
      .map(|raw| RecentSites::from_json(&raw))
      .unwrap_or_default()
  });
  let (changed, out) = edit(sites);
  if changed {
    let _ = std::fs::write(&file, sites.to_json());
  }
  Some(out)
}

/// The home page's lists: recent sites and the last videos saved from the Nezzel browser.
pub(super) fn home(app: &AppHandle) -> (Vec<RecentSite>, Vec<RecentSave>) {
  let sites = recent(app, |sites| (false, sites.list().to_vec())).unwrap_or_default();
  let saves = app
    .state::<CatchWindows>()
    .saves
    .lock()
    .map(|saves| saves.list().to_vec())
    .unwrap_or_default();
  (sites, saves)
}

/// Forgets one site of the recent list, or all of them.
pub(super) fn forget_recent(app: &AppHandle, site: Option<&Url>) {
  recent(app, |sites| match site {
    Some(url) => (sites.forget(url), ()),
    None => {
      sites.clear();
      (true, ())
    }
  });
}

pub(super) fn remember_save(app: &AppHandle, save: RecentSave, private: bool) {
  if let Ok(mut saves) = app.state::<CatchWindows>().saves.lock() {
    saves.record(save, private);
  }
}

/// Whether the home page lists this download: the only ones it may ask to show.
pub(super) fn saved_here(app: &AppHandle, download_id: &str) -> bool {
  app.state::<CatchWindows>().saves.lock().is_ok_and(|saves| {
    saves
      .list()
      .iter()
      .any(|save| save.download_id == download_id)
  })
}

fn blank() -> Url {
  Url::parse("about:blank").expect("constant URL")
}

fn first_run(app: &AppHandle) -> bool {
  let Ok(dir) = app.path().app_data_dir() else {
    return false;
  };
  std::fs::create_dir_all(&dir).is_ok()
    && OpenOptions::new()
      .write(true)
      .create_new(true)
      .open(dir.join("catch-seen"))
      .is_ok()
}

pub(super) fn bar_window_id(label: &str) -> Option<&str> {
  label
    .strip_prefix("catch-bar-")
    .filter(|id| is_id_suffix(id))
}

fn is_id_suffix(id: &str) -> bool {
  id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The page webview a command means: a window id names its active tab's page.
pub(super) fn page(app: &AppHandle, id: &str) -> Result<Webview, &'static str> {
  let (window, tab) = split_label(id).ok_or("catchWindowClosed")?;
  let label = match (tab, app.state::<CatchWindows>().browser(window)) {
    (None, Ok(browser)) => {
      let state = browser.active()?;
      state.active_page().ok_or("catchNoPage")?;
      tab_label(window, state.tabs.active_id())
    }
    _ => id.to_owned(),
  };
  app.get_webview(&label).ok_or("catchWindowClosed")
}

/// The shell covers the whole window; every page fills the content rectangle and only the
/// active tab's page is shown. `order` also puts the shell over or under the page.
fn layout(app: &AppHandle, id: &str, browser: &Browser, order: bool) {
  let Some(window) = app.get_window(id) else {
    return;
  };
  let (fit, pages, active) = {
    let Ok(state) = browser.active() else {
      return;
    };
    (
      state.fit,
      state
        .pages
        .keys()
        .map(|tab| tab_label(id, tab))
        .collect::<Vec<_>>(),
      tab_label(id, state.tabs.active_id()),
    )
  };
  let (Ok(size), Ok(scale)) = (window.inner_size(), window.scale_factor()) else {
    return;
  };
  let size = size.to_logical::<f64>(scale);
  let shell = app.get_webview(&bar_label(id));
  if let Some(shell) = &shell {
    layering::place(shell, 0., 0., size.width, size.height);
  }
  let (x, y, width, height) = content_rect(size.width, size.height, fit);
  for label in pages {
    let Some(page) = app.get_webview(&label) else {
      continue;
    };
    layering::place(&page, x, y, width, height);
    let _ = if label == active {
      page.show()
    } else {
      page.hide()
    };
  }
  // Last: a page that just came into the window is on top of the shell until this.
  if let (true, Some(shell)) = (order, &shell) {
    layering::order(
      &window,
      shell,
      app.get_webview(&active).as_ref(),
      fit.overlay,
    );
  }
}

/// After any change of tabs: place the webviews, then tell the shell.
fn refresh(app: &AppHandle, id: &str, browser: &Browser) {
  layout(app, id, browser, true);
  publish(app, id, browser);
}

pub(super) fn fit(app: &AppHandle, id: &str, fit: Fit) -> Result<(), &'static str> {
  if !(fit.top.is_finite() && fit.side.is_finite() && fit.top >= 0. && fit.side >= 0.) {
    return Err("catchInvalidBarHeight");
  }
  let browser = app.state::<CatchWindows>().browser(id)?;
  browser.active()?.fit = fit;
  layout(app, id, &browser, true);
  Ok(())
}

/// v1 `catch_bar_fit`: the bar's own height, and the height it draws over the page with.
pub(super) fn bar_fit(
  app: &AppHandle,
  id: &str,
  height: f64,
  body: Option<f64>,
) -> Result<(), &'static str> {
  let body = body.unwrap_or(BAR_HEIGHT.min(height));
  if !height.is_finite() || !body.is_finite() || body <= 0. || body > height {
    return Err("catchInvalidBarHeight");
  }
  let last = app.state::<CatchWindows>().browser(id)?.active()?.fit;
  fit(
    app,
    id,
    Fit {
      top: body,
      overlay: height > body,
      ..last
    },
  )
}

/// The shell listens: send it the whole state.
pub(super) fn shell_ready(app: &AppHandle, id: &str) -> Result<(), &'static str> {
  let browser = app.state::<CatchWindows>().browser(id)?;
  {
    let mut state = browser.active()?;
    state.shell_loaded = true;
    state.last = None;
  }
  publish(app, id, &browser);
  Ok(())
}

/// Counts the encrypted verdict a tab shows: only once its retries ran out.
pub(super) fn count_drm(app: &AppHandle, page: &mut PageState) {
  if page.drm_final(std::time::Instant::now())
    && !std::mem::replace(&mut page.document.drm_counted, true)
  {
    crate::usage::count_catch(app, crate::usage::keys::CATCH_DRM, &page.url);
  }
}

fn private_dir(app: &AppHandle) -> Option<std::path::PathBuf> {
  Some(app.path().app_data_dir().ok()?.join("catch-private"))
}

/// A private tab's own store. macOS: a new in-memory store for each webview.
#[cfg(target_os = "macos")]
fn private_store(
  _app: &AppHandle,
  _id: &str,
  _store: &str,
  builder: WebviewBuilder<tauri::Wry>,
) -> Result<WebviewBuilder<tauri::Wry>, &'static str> {
  Ok(builder.incognito(true))
}

/// A private tab's own store: a folder no other tab uses, deleted when the tab closes.
#[cfg(not(target_os = "macos"))]
fn private_store(
  app: &AppHandle,
  id: &str,
  store: &str,
  builder: WebviewBuilder<tauri::Wry>,
) -> Result<WebviewBuilder<tauri::Wry>, &'static str> {
  let dir = private_dir(app)
    .ok_or("catchOpenFailed")?
    .join(format!("{id}-{store}"));
  std::fs::create_dir_all(dir.parent().ok_or("catchOpenFailed")?).map_err(|_| "catchOpenFailed")?;
  crate::runners::browser_cookies::session_directory(&dir).map_err(|_| "catchOpenFailed")?;
  Ok(builder.incognito(true).data_directory(dir))
}

fn drop_store(app: &AppHandle, id: &str, store: &str) {
  if let Some(dir) = private_dir(app) {
    let _ = std::fs::remove_dir_all(dir.join(format!("{id}-{store}")));
  }
}

/// Gives a tab its page webview and loads `url` there. Call with no lock held. False: the tab
/// has a page already, or another command is building it (that one wins).
fn spawn_page(
  app: &AppHandle,
  id: &str,
  browser: &Browser,
  tab: &str,
  url: &Url,
  configure: impl FnOnce(WebviewBuilder<tauri::Wry>) -> WebviewBuilder<tauri::Wry>,
) -> Result<bool, &'static str> {
  let window = app.get_window(id).ok_or("catchWindowClosed")?;
  let label = tab_label(id, tab);
  let (session, store, fit, proxy) = {
    let mut state = browser.active()?;
    let place = state.tabs.get(tab).ok_or("catchTabClosed")?;
    let (store, private) = (place.store.clone(), place.private());
    // A window that refused its pages has none: it takes the setting as it is now.
    if state.proxy == crate::net::BrowserProxy::Blocked {
      state.proxy = crate::net::browser_proxy();
    }
    // On and unusable: no page, before the tab is claimed.
    let proxied = super::proxy::plan(
      &state.proxy,
      super::proxy::engine(),
      matches!(store, Store::Shared),
    )?;
    if !state.claim_page(tab) {
      return Ok(false);
    }
    let session = Arc::new(Session::tab(url.clone(), private));
    if let Ok(mut page) = session.active() {
      page.proxied = !state.proxy.fetches_beside();
      page.page.first_run = state.home.first_run;
      // `catch_used` counts a window once, not each of its tabs.
      page.quiet = std::mem::replace(&mut state.had_page, true);
    }
    (session, store, state.fit, proxied)
  };
  let builder = configure(page_builder(app, &label, &session));
  let built = build_page(
    app, id, browser, tab, url, &window, &label, &session, &store, fit, proxy, builder,
  );
  if let Ok(mut state) = browser.state.lock() {
    state.building.remove(tab);
  }
  built.map(|()| true)
}

/// `spawn_page` once the tab's slot is claimed. On failure it removes only what it made.
#[allow(clippy::too_many_arguments)]
fn build_page(
  app: &AppHandle,
  id: &str,
  browser: &Browser,
  tab: &str,
  url: &Url,
  window: &Window,
  label: &str,
  session: &Arc<Session>,
  store: &Store,
  fit: Fit,
  proxied: super::proxy::Plan,
  builder: WebviewBuilder<tauri::Wry>,
) -> Result<(), &'static str> {
  let (label, session) = (label.to_owned(), session.clone());
  let builder = match proxied.url {
    Some(url) => builder.proxy_url(url),
    None => builder,
  };
  #[cfg(windows)]
  let builder = match &proxied.arguments {
    Some(arguments) => builder.additional_browser_args(arguments),
    None => builder,
  };
  let builder = match store {
    Store::Shared => profile(app, builder)?,
    Store::Private(store) => private_store(app, id, store, builder)?,
  };
  let size = window
    .inner_size()
    .map_err(|_| "catchOpenFailed")?
    .to_logical::<f64>(window.scale_factor().map_err(|_| "catchOpenFailed")?);
  let (x, y, width, height) = content_rect(size.width, size.height, fit);
  let page = window
    .add_child(
      builder,
      LogicalPosition::new(x, y),
      LogicalSize::new(width, height),
    )
    .map_err(|_| "catchOpenFailed")?;
  if proxied.clear {
    super::proxy::clear(&page);
  }
  if proxied.sealed {
    super::proxy::seal(&page);
  }
  #[cfg(not(target_os = "macos"))]
  {
    let (app, label) = (app.clone(), label.clone());
    native::watch_crash(&page, move || {
      renderer_crashed(&app, &label);
    });
  }
  #[cfg(not(target_os = "macos"))]
  watch_load_failure(app, &page);
  keys::watch_page(app, id, &page);
  super::adblock::attach(app, &page, &session);
  // Before the page loads: only a click may open a pop-up.
  popup::gate(&page, false);
  popup::watch_clicks(&page, &session);
  let started = (|| {
    app
      .state::<CatchWindows>()
      .insert(&label, session.clone())?;
    let drain = tauri::async_runtime::spawn(drain_loop(
      app.clone(),
      label.clone(),
      session.clone(),
      page.clone(),
      Arc::new(PinnedManifestFetcher),
    ));
    session.active()?.drain = Some(drain.inner().abort_handle());
    let background = {
      let mut state = browser.active()?;
      if state.tabs.get(tab).is_none() {
        return Err("catchTabClosed");
      }
      state.tabs.set_asleep(tab, false);
      state.pages.insert(tab.to_owned(), session.clone());
      state.tabs.active_id() != tab
    };
    if background {
      let _ = page.hide();
    }
    // Install the pair and register close handling before running remote scripts.
    super::adblock::navigate(app, &page, &session, url.clone())
  })();
  if started.is_err() {
    app.state::<CatchWindows>().remove(&label);
    if let Ok(mut state) = browser.state.lock() {
      if state
        .pages
        .get(tab)
        .is_some_and(|page| Arc::ptr_eq(page, &session))
      {
        state.pages.remove(tab);
      }
    }
    session.stop();
    let _ = page.close();
  }
  started
}

/// Loads an address in the active tab when it has no page yet (the home page). Returns false
/// when the tab has a page: the caller navigates that one.
pub(super) fn load_active(app: &AppHandle, id: &str, url: &Url) -> Result<bool, &'static str> {
  load_tab(app, id, None, url)
}

/// `load_active` for the named tab (the active one without a name): the home page or a sleeping tab.
pub(super) fn load_tab(
  app: &AppHandle,
  id: &str,
  tab: Option<&str>,
  url: &Url,
) -> Result<bool, &'static str> {
  let browser = app.state::<CatchWindows>().browser(id)?;
  let tab = {
    let mut state = browser.active()?;
    let tab = tab.unwrap_or(state.tabs.active_id()).to_owned();
    if state.tabs.get(&tab).is_none() {
      return Err("catchTabClosed");
    }
    if state.pages.contains_key(&tab) {
      return Ok(false);
    }
    // Another command is giving this tab its page: that address wins.
    if state.building.contains(&tab) {
      return Ok(true);
    }
    state.tabs.set_page(&tab, url.to_string(), None);
    tab
  };
  let loaded = spawn_page(app, id, &browser, &tab, url, std::convert::identity);
  refresh(app, id, &browser);
  loaded.map(|_| true)
}

/// Opens a tab and shows it: the home page, or `url`.
pub(super) fn tab_new(
  app: &AppHandle,
  id: &str,
  url: Option<Url>,
  private: bool,
) -> Result<String, &'static str> {
  let browser = app.state::<CatchWindows>().browser(id)?;
  let tab = browser.active()?.tabs.open(
    NewTab {
      url: url.as_ref().map(Url::to_string).unwrap_or_default(),
      private,
      opener: None,
    },
    tokio::time::Instant::now(),
  )?;
  finish_new_tab(app, id, &browser, tab, url.as_ref(), std::convert::identity)
}

fn finish_new_tab(
  app: &AppHandle,
  id: &str,
  browser: &Browser,
  tab: String,
  url: Option<&Url>,
  configure: impl FnOnce(WebviewBuilder<tauri::Wry>) -> WebviewBuilder<tauri::Wry>,
) -> Result<String, &'static str> {
  let loaded = match url {
    Some(url) => spawn_page(app, id, browser, &tab, url, configure).map(|_| ()),
    None => Ok(()),
  };
  if loaded.is_err() {
    let closed = browser
      .active()
      .ok()
      .and_then(|mut state| state.tabs.close(&tab).ok());
    // A private tab's store may exist already: it goes with the tab.
    if let Some(store) = closed.and_then(|closed| closed.drop_store) {
      drop_store(app, id, &store);
    }
  }
  refresh(app, id, browser);
  if loaded.is_ok() {
    crate::usage::count(app, crate::usage::keys::CATCH_TAB_OPENED);
  }
  loaded.map(|()| tab)
}

/// `WKNavigationAction.buttonNumber` on macOS is a button mask (WebKit `toNSButtonNumber`):
/// left 1, right 2, middle 4.
#[cfg(any(target_os = "macos", test))]
const MIDDLE_BUTTON: isize = 1 << 2;

#[cfg(any(target_os = "macos", test))]
fn opens_background_tab(
  main_frame: bool,
  new_window: bool,
  link_activated: bool,
  command: bool,
  button: isize,
  url: &Url,
) -> bool {
  // A new-window request (`target=_blank`, or a page's `window.open` during the click) has no
  // target frame, so it is never the main frame; Cmd or the middle button still mean "behind".
  (main_frame && link_activated || new_window)
    && (command || button == MIDDLE_BUTTON)
    && is_http(url)
}

#[cfg(target_os = "macos")]
fn queue_background_tab(app: &AppHandle, label: &str, opener: &Arc<Session>, url: Url) -> bool {
  let Some((id, Some(from))) = split_label(label) else {
    return false;
  };
  let (app, id, from, opener) = (app.clone(), id.to_owned(), from.to_owned(), opener.clone());
  // Dispatch from a worker: with_webview called from the delegate may run inline.
  tauri::async_runtime::spawn(async move {
    let Some(source) = app.get_webview(&tab_label(&id, &from)) else {
      return;
    };
    let _ = source.with_webview(move |view| {
      let Ok(browser) = app.state::<CatchWindows>().browser(&id) else {
        return;
      };
      let opened = browser.active().and_then(|mut state| {
        if !state
          .pages
          .get(&from)
          .is_some_and(|page| Arc::ptr_eq(page, &opener))
        {
          return Err("catchTabClosed");
        }
        state.tabs.open_background(
          NewTab {
            url: url.to_string(),
            private: false,
            // As with popup::requested, a linked tab inherits the opener's store.
            opener: Some(from),
          },
          tokio::time::Instant::now(),
        )
      });
      match opened {
        Ok(tab) => {
          let private = opener.active().is_ok_and(|state| state.private);
          let _ = finish_new_tab(&app, &id, &browser, tab, Some(&url), |builder| {
            let builder = builder.focused(false);
            let Some(main) = objc2::MainThreadMarker::new().filter(|_| private) else {
              return builder;
            };
            // Tauri supplies the opener on the main thread. Share its private store while
            // giving this independent page its own configuration and initialization scripts.
            let configuration = unsafe {
              let view = &*view.inner().cast::<objc2_web_kit::WKWebView>();
              let store = view.configuration().websiteDataStore();
              let configuration = objc2_web_kit::WKWebViewConfiguration::new(main);
              configuration.setWebsiteDataStore(&store);
              configuration
            };
            builder.with_webview_configuration(configuration)
          });
        }
        Err("catchTabLimit") => {
          if let Ok(mut state) = opener.active() {
            state.page.popup.deny();
          }
          publish(&app, &id, &browser);
        }
        Err(_) => {}
      }
    });
  });
  true
}

/// Shows a tab; a sleeping one loads its address again.
pub(super) fn tab_select(app: &AppHandle, id: &str, tab: &str) -> Result<(), &'static str> {
  let browser = app.state::<CatchWindows>().browser(id)?;
  let wake = {
    let mut state = browser.active()?;
    let has_page = state.has_page(tab);
    let shown = state.tabs.select(tab, tokio::time::Instant::now())?;
    Url::parse(&shown.url).ok().filter(|_| !has_page)
  };
  let woken = match wake {
    Some(url) => spawn_page(app, id, &browser, tab, &url, std::convert::identity).map(|_| ()),
    None => Ok(()),
  };
  refresh(app, id, &browser);
  woken
}

/// Closes a tab; its opener, else its neighbour, is shown (the home page after the last tab).
pub(super) async fn tab_close(app: &AppHandle, id: &str, tab: &str) -> Result<(), &'static str> {
  let browser = app.state::<CatchWindows>().browser(id)?;
  let (closed, session, wake) = {
    let mut state = browser.active()?;
    let closed = state.tabs.close(tab)?;
    let session = state.pages.remove(tab);
    let next = state.tabs.active();
    let wake = Url::parse(&next.url)
      .ok()
      .filter(|_| !state.has_page(&next.id))
      .map(|url| (next.id.clone(), url));
    (closed, session, wake)
  };
  if let Some((next, url)) = wake {
    let _ = spawn_page(app, id, &browser, &next, &url, std::convert::identity);
  }
  refresh(app, id, &browser);
  if let Some(session) = session {
    drop_page(app, tab_label(id, tab), &session).await;
  }
  if let Some(store) = closed.drop_store {
    drop_store(app, id, &store);
  }
  Ok(())
}

/// A tab shortcut pressed while the keyboard was in a page (in the shell, the shell acts itself).
pub(super) fn shortcut(app: &AppHandle, id: &str, shortcut: Shortcut) {
  let Ok(browser) = app.state::<CatchWindows>().browser(id) else {
    return;
  };
  let neighbour = |by| {
    browser
      .active()
      .map(|state| state.tabs.neighbour(by).to_owned())
  };
  let ask_shell = |what: &str| {
    if let Some(shell) = app.get_webview(&bar_label(id)) {
      let _ = shell.set_focus();
    }
    let _ = app.emit_to(EventTarget::webview(bar_label(id)), SHORTCUT_EVENT, what);
  };
  let done = match shortcut {
    Shortcut::Address => return ask_shell("address"),
    Shortcut::NewTab => {
      // The home page is the shell's: the keyboard goes to its field.
      return match tab_new(app, id, None, false) {
        Ok(_) => ask_shell("address"),
        Err("catchTabLimit") => ask_shell("tabLimit"),
        Err(_) => {}
      };
    }
    Shortcut::CloseTab => {
      let Ok(tab) = neighbour(0) else {
        return;
      };
      let (app, id) = (app.clone(), id.to_owned());
      tauri::async_runtime::spawn(async move {
        if tab_close(&app, &id, &tab).await.is_ok() {
          focus_active(&app, &id);
        }
      });
      return;
    }
    Shortcut::NextTab => neighbour(1).and_then(|tab| tab_select(app, id, &tab)),
    Shortcut::PreviousTab => neighbour(-1).and_then(|tab| tab_select(app, id, &tab)),
  };
  if done.is_ok() {
    focus_active(app, id);
  }
}

/// The keyboard follows the tab shown: its page, or the shell on the home page.
fn focus_active(app: &AppHandle, id: &str) {
  let shown = page(app, id)
    .ok()
    .or_else(|| app.get_webview(&bar_label(id)));
  if let Some(shown) = shown {
    let _ = shown.set_focus();
  }
}

/// A link from outside the window opens in the window on screen: the tab that already shows it,
/// a home tab, or a new tab. At six tabs with no home tab the shell asks which tab to close and
/// the caller gets `catchTabLimit`.
fn open_in(
  app: &AppHandle,
  id: &str,
  browser: &Browser,
  url: Option<&Url>,
) -> Result<(), &'static str> {
  let place = browser.active()?.tabs.place_link(url);
  let window = app.get_window(id).ok_or("catchWindowClosed")?;
  let _ = window.unminimize();
  let _ = window.show();
  let _ = window.set_focus();
  match (place, url) {
    (LinkPlace::Open(tab), _) | (LinkPlace::Home(tab), None) => tab_select(app, id, &tab)?,
    (LinkPlace::Home(tab), Some(url)) => {
      tab_select(app, id, &tab)?;
      load_tab(app, id, Some(&tab), url)?;
    }
    (LinkPlace::New, _) => {
      tab_new(app, id, url.cloned(), false)?;
    }
    // No link to place: the window in front is all that was asked.
    (LinkPlace::Full, None) => return Ok(()),
    (LinkPlace::Full, Some(url)) => {
      if let Some(shell) = app.get_webview(&bar_label(id)) {
        let _ = shell.set_focus();
      }
      // The link rides along: the tab that replaces the closed one opens it.
      let ask = serde_json::json!({ "tabLimit": url.as_str() });
      let _ = app.emit_to(EventTarget::webview(bar_label(id)), SHORTCUT_EVENT, ask);
      return Err("catchTabLimit");
    }
  }
  focus_active(app, id);
  Ok(())
}

fn browser_surface(setting: &str, system: Theme) -> Color {
  let dark = match setting {
    "dark" => true,
    "light" => false,
    _ => system == Theme::Dark,
  };
  // The browser's home and top rows use the --background token in src/app.css (DESIGN.md).
  if dark {
    Color(10, 15, 14, 255)
  } else {
    Color(250, 252, 251, 255)
  }
}

pub(crate) fn update_browser_theme(app: &AppHandle, setting: &str) {
  let window = app
    .try_state::<CatchWindows>()
    .and_then(|windows| windows.shown())
    .and_then(|(id, _)| app.get_window(&id));
  if let Some(window) = window {
    apply_browser_surface(&window, setting);
  }
}

fn apply_browser_surface(window: &Window, setting: &str) {
  let system = window.theme().unwrap_or(Theme::Light);
  let _ = window.set_background_color(Some(browser_surface(setting, system)));
}

pub(super) fn open(app: &AppHandle, url: Option<Url>) -> Result<String, &'static str> {
  #[cfg(target_os = "macos")]
  {
    let app = app.clone();
    wry::set_navigation_failure_hook(move |_, label, code, address| {
      if let Some(address) = address.filter(|_| show_mac_load_failure(code)) {
        load_failed(&app, label, code as i64, address);
      }
    });
  }
  let url = url.filter(is_http);
  // Disk I/O, before any lock.
  let first = first_run(app);
  let registry = app.state::<CatchWindows>();
  let _opening = registry.opening.lock().map_err(|_| "catchOpenFailed")?;
  if let Some((id, browser)) = registry.shown() {
    match open_in(app, &id, &browser, url.as_ref()) {
      // Closed under us: open a window as if there was none.
      Err("catchWindowClosed") => {}
      done => return done.map(|()| id),
    }
  }
  if let Some((id, browser)) = registry.take_idle() {
    match reopen(app, &id, browser, url.as_ref(), first) {
      Ok(()) => return Ok(id),
      Err(_) => {
        discard(app, &id);
        if let Some(window) = app.get_window(&id) {
          let _ = window.destroy();
        }
      }
    }
  }
  if registry
    .browsers
    .lock()
    .is_ok_and(|browsers| browsers.is_empty())
  {
    // Private stores a crash or a busy engine left behind.
    if let Some(dir) = private_dir(app) {
      let _ = std::fs::remove_dir_all(dir);
    }
  }
  let suffix = uuid::Uuid::new_v4().simple().to_string();
  let id = format!("catch-{suffix}");
  let browser = Arc::new(Browser::new(first));
  let settings = app.state::<crate::SharedConfig>().load();
  let system = app
    .windows()
    .values()
    .find_map(|window| window.theme().ok())
    .unwrap_or(Theme::Light);
  let window = WindowBuilder::new(app, &id)
    .title(&app.package_info().name)
    .inner_size(960., 760.)
    .min_inner_size(640., 360.)
    .background_color(browser_surface(&settings.appearance.theme, system))
    .visible(false);
  #[cfg(target_os = "macos")]
  let window = window
    .title_bar_style(tauri::TitleBarStyle::Overlay)
    .hidden_title(true);
  let window = window.build().map_err(|_| "catchOpenFailed")?;
  let opened = (|| {
    // The local route owns the only Catch capability.
    let dev_url = app
      .config()
      .build
      .dev_url
      .clone()
      .filter(|_| tauri::is_dev());
    let shell_url = WebviewUrl::App(format!("catch-bar?windowId={id}").into());
    let shell = WebviewBuilder::new(bar_label(&id), shell_url)
      .transparent(true)
      .on_navigation(move |url| {
        url.path() == "/catch-bar"
          && (dev_url
            .as_ref()
            .is_some_and(|dev| dev.origin() == url.origin())
            || (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
            || (cfg!(windows)
              && url.scheme() == "http"
              && url.host_str() == Some("tauri.localhost")
              && url.port().is_none()))
      })
      .on_new_window(|_, _| NewWindowResponse::Deny)
      .on_download(|_, _| false);
    // The shell is added first: every page added later sits above it.
    let shell = window
      .add_child(
        shell,
        LogicalPosition::new(0., 0.),
        LogicalSize::new(960., 760.),
      )
      .map_err(|_| "catchOpenFailed")?;
    keys::watch_shell(app, &id, &shell);
    registry
      .browsers
      .lock()
      .map_err(|_| "catchOpenFailed")?
      .insert(id.clone(), browser.clone());
    attach_events(app, &id, &window);
    apply_browser_surface(
      &window,
      &app.state::<crate::SharedConfig>().load().appearance.theme,
    );
    window.show().map_err(|_| "catchOpenFailed")?;
    match &url {
      Some(url) => load_active(app, &id, url).map(|_| ()),
      None => Ok(()),
    }
  })();
  if opened.is_err() {
    discard(app, &id);
    let _ = window.destroy();
  }
  opened.map(|()| id)
}

/// WebView2 `COREWEBVIEW2_WEB_ERROR_STATUS` values that are not a failed page: unknown (0),
/// connection aborted (9: a navigation that became a download, or the page called
/// `window.stop()`), operation canceled (14) and the sign-in prompts (17, 18).
#[cfg(any(windows, test))]
pub(super) fn show_windows_load_failure(code: i32) -> bool {
  !matches!(code, 0 | 9 | 14 | 17 | 18)
}

#[cfg(any(target_os = "macos", test))]
fn show_mac_load_failure(code: isize) -> bool {
  // NSURLErrorCancelled, WebKit's FrameLoadInterruptedByPolicyChange (e.g. downloads) and
  // PlugInWillHandleLoad (a direct media link the page's player plays).
  !matches!(code, -999 | 102 | 204)
}

fn load_failed(app: &AppHandle, label: &str, code: i64, address: &str) {
  if !matches!(split_label(label), Some((_, Some(_)))) {
    return;
  }
  let Some(url) = Url::parse(address).ok().filter(is_http) else {
    return;
  };
  let Ok(session) = app.state::<CatchWindows>().page_session(label) else {
    return;
  };
  let Ok(mut state) = session.active() else {
    return;
  };
  let site = url.host_str().and_then(psl::domain_str).unwrap_or_default();
  tracing::debug!(code, site, "catch page load failed");
  state.load_failed = true;
  state.loading = false;
  state.last_browser_status.loading = false;
  // Discard the previous document's capture state without offering the failed address as media.
  let offers_page = std::mem::take(&mut state.page.offers_page);
  state.page.navigated(url);
  state.page.offers_page = offers_page;
  drop(state);
  let (app, label) = (app.clone(), label.to_owned());
  tauri::async_runtime::spawn(async move { notify(&app, &label) });
}

#[cfg(not(target_os = "macos"))]
fn watch_load_failure(app: &AppHandle, page: &Webview) {
  let (app, label) = (app.clone(), page.label().to_owned());
  native::watch_load_failure(page, move |code, address| {
    load_failed(&app, &label, code, address);
  });
}

/// A parked window opens again: nothing of its last use is left (it never loaded a page).
fn reopen(
  app: &AppHandle,
  id: &str,
  browser: Arc<Browser>,
  url: Option<&Url>,
  first: bool,
) -> Result<(), &'static str> {
  let window = app.get_window(id).ok_or("catchOpenFailed")?;
  app.get_webview(&bar_label(id)).ok_or("catchOpenFailed")?;
  browser
    .state
    .lock()
    .map_err(|_| "catchOpenFailed")?
    .reopen(first);
  app
    .state::<CatchWindows>()
    .browsers
    .lock()
    .map_err(|_| "catchOpenFailed")?
    .insert(id.to_owned(), browser.clone());
  apply_browser_surface(
    &window,
    &app.state::<crate::SharedConfig>().load().appearance.theme,
  );
  match url {
    Some(url) => {
      load_active(app, id, url)?;
    }
    // The shell stays loaded while parked: give it the fresh state now.
    None => refresh(app, id, &browser),
  }
  window.show().map_err(|_| "catchOpenFailed")?;
  let _ = window.set_focus();
  Ok(())
}

/// The page's renderer process died. Reload once after a pause; a repeat within a minute shows
/// the bar's Reload instead (no reload loop). Returns false for webviews that aren't Catch pages.
pub(crate) fn renderer_crashed(app: &AppHandle, id: &str) -> bool {
  let Ok(session) = app.state::<CatchWindows>().page_session(id) else {
    return false;
  };
  let Ok(mut state) = session.active() else {
    return true;
  };
  match state.crash.crashed(Instant::now()) {
    Recovery::Reload(after) => {
      let (target, generation) = (state.page.url.clone(), state.page.generation);
      drop(state);
      if !is_http(&target) {
        return true;
      }
      let (app, id) = (app.clone(), id.to_owned());
      tauri::async_runtime::spawn(async move {
        tokio::time::sleep(after).await;
        let current = app
          .state::<CatchWindows>()
          .page_session(&id)
          .ok()
          .and_then(|session| session.active().ok().map(|state| state.page.generation));
        if current == Some(generation) {
          if let Ok(page) = page(&app, &id) {
            // Navigate, not reload: a crashed WKWebView may have no address left to reload.
            let _ = page.navigate(target);
          }
        }
      });
    }
    Recovery::Stop => {
      state.page.document.crashed = true;
      // Windows reads "loading" from load events, and a dead page sends no "finished".
      state.loading = false;
      state.last_browser_status.loading = false;
      drop(state);
      notify(app, id);
    }
  }
  true
}

/// What a webview whose process died gets on macOS.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Crashed {
  /// A Catch page: the policy above, or nothing while it is parked or closing.
  CatchPage,
  /// A Catch pop-up shows a remote page too: no reload, the user closes it.
  CatchPopup,
  /// Every other webview keeps Tauri's default reload.
  Other,
}

#[cfg(any(target_os = "macos", test))]
pub(super) fn crashed_kind(label: &str) -> Crashed {
  if split_label(label).is_some() {
    Crashed::CatchPage
  } else if label.starts_with("catch-popup-") {
    Crashed::CatchPopup
  } else {
    Crashed::Other
  }
}

/// macOS: Tauri's default reloads a crashed page every time (a loop on pages that keep crashing).
/// Catch pages get the policy above; every other webview keeps the default reload.
#[cfg(target_os = "macos")]
pub(crate) fn with_crash_hook(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
  builder.on_web_content_process_terminate(|webview| {
    let label = webview.label();
    match crashed_kind(label) {
      Crashed::CatchPage => {
        renderer_crashed(webview.app_handle(), label);
      }
      Crashed::CatchPopup => {}
      Crashed::Other => {
        let _ = webview.reload();
      }
    }
  })
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn with_crash_hook(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
  builder
}

/// App exit: stop every drain loop and pop-up, parked window included.
pub(crate) fn stop_all(app: &AppHandle) {
  let Some(registry) = app.try_state::<CatchWindows>() else {
    return;
  };
  let sessions: Vec<_> = registry
    .sessions
    .lock()
    .map(|mut sessions| sessions.drain().map(|(_, session)| session).collect())
    .unwrap_or_default();
  for session in sessions {
    for popup in session.stop() {
      let _ = popup.destroy();
    }
  }
  let mut browsers: Vec<_> = registry
    .browsers
    .lock()
    .map(|mut browsers| browsers.drain().map(|(_, browser)| browser).collect())
    .unwrap_or_default();
  browsers.extend(registry.take_idle().map(|(_, browser)| browser));
  for browser in browsers {
    if let Ok(mut state) = browser.state.lock() {
      state.shut();
    }
  }
  // No tab outlives the app: every private store goes.
  if let Some(dir) = private_dir(app) {
    let _ = std::fs::remove_dir_all(dir);
  }
}

/// macOS 14+: the persistent catch profile's data store (normal tabs share it).
#[cfg(target_os = "macos")]
pub(super) const CATCH_STORE: [u8; 16] = [
  0x40, 0x62, 0x4d, 0xc7, 0x73, 0x2e, 0x44, 0x20, 0x98, 0x71, 0x15, 0xf9, 0xc3, 0x9b, 0x39, 0x29,
];

#[cfg(target_os = "macos")]
fn profile(
  _app: &AppHandle,
  builder: WebviewBuilder<tauri::Wry>,
) -> Result<WebviewBuilder<tauri::Wry>, &'static str> {
  Ok(if native::use_persistent_profile() {
    builder.data_store_identifier(CATCH_STORE)
  } else {
    builder.incognito(true)
  })
}

#[cfg(not(target_os = "macos"))]
fn profile(
  app: &AppHandle,
  builder: WebviewBuilder<tauri::Wry>,
) -> Result<WebviewBuilder<tauri::Wry>, &'static str> {
  let dir = app
    .path()
    .app_data_dir()
    .map_err(|_| "catchOpenFailed")?
    .join("catch-profile");
  std::fs::create_dir_all(dir.parent().ok_or("catchOpenFailed")?).map_err(|_| "catchOpenFailed")?;
  // The persistent profile holds site cookies: owner-only, never a symlink.
  crate::runners::browser_cookies::session_directory(&dir).map_err(|_| "catchOpenFailed")?;
  Ok(builder.data_directory(dir))
}

/// The name Safari gives itself. A bare WKWebView names no browser, and sites then ask for a newer one.
/// Built on every system: the iPhone identity shares its version check.
pub(super) fn safari_agent(version: &str) -> Option<String> {
  let plain = !version.is_empty()
    && version.len() <= 12
    && version
      .split('.')
      .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
  plain.then(|| {
    format!(
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) \
       Version/{version} Safari/605.1.15"
    )
  })
}

/// What pages see as the browser: the Safari this Mac has. Elsewhere the engine already names itself.
#[cfg(target_os = "macos")]
pub(super) fn browser_agent() -> Option<&'static str> {
  static AGENT: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
  AGENT
    .get_or_init(|| safari_agent(browser_version()?))
    .as_deref()
}

/// The installed Safari's version (macOS only).
#[cfg(target_os = "macos")]
pub(super) fn browser_version() -> Option<&'static str> {
  static VERSION: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
  VERSION
    .get_or_init(|| {
      let info = plist::Value::from_file("/Applications/Safari.app/Contents/Info.plist").ok()?;
      Some(
        info
          .as_dictionary()?
          .get("CFBundleShortVersionString")?
          .as_string()?
          .to_owned(),
      )
    })
    .as_deref()
}

#[cfg(not(target_os = "macos"))]
pub(super) fn browser_agent() -> Option<&'static str> {
  None
}

#[cfg(not(target_os = "macos"))]
pub(super) fn browser_version() -> Option<&'static str> {
  None
}

/// A video page needs none of camera, microphone, location, notifications, clipboard, screen
/// capture or devices: every request is refused without a prompt, in the visible window, its
/// popups and the hidden page alike. Autoplay keeps the engine default (WebView2 asks for it).
pub(super) fn permission(kind: PermissionKind) -> PermissionResponse {
  match kind {
    PermissionKind::Autoplay => PermissionResponse::Default,
    _ => PermissionResponse::Deny,
  }
}

fn page_builder(app: &AppHandle, id: &str, session: &Arc<Session>) -> WebviewBuilder<tauri::Wry> {
  page_with(app, id, session, true)
}

/// `scripts`: false for a page that gets its opener's scripts from the engine.
fn page_with(
  app: &AppHandle,
  id: &str,
  session: &Arc<Session>,
  scripts: bool,
) -> WebviewBuilder<tauri::Wry> {
  let navigation = session.clone();
  let loading = session.clone();
  let popups = session.clone();
  let usage_app = app.clone();
  let (download_app, downloads) = (app.clone(), session.clone());
  let download_target = bar_label(window_of(id));
  super::adblock::prepare(app);
  let (blocker_app, blocker_label) = (app.clone(), id.to_owned());
  let (deciding_app, deciding_label) = (app.clone(), id.to_owned());
  let app = app.clone();
  let opener = id.to_owned();
  let builder = WebviewBuilder::new(id, WebviewUrl::External(blank()));
  let builder = match browser_agent() {
    Some(agent) => builder.user_agent(agent),
    None => builder,
  };
  let builder = if scripts {
    builder.initialization_script_for_all_frames(capture_script(false))
  } else {
    builder
  };
  let builder = if scripts && matches!(split_label(id), Some((_, Some(_)))) {
    builder.initialization_script(popup::script())
  } else {
    builder
  };
  builder
    .disable_drag_drop_handler()
    .on_permission_request(|_, kind| permission(kind))
    .on_navigation(move |url| {
      let main_frame = popup::main_frame_navigation();
      let new_window = popup::new_window_navigation();
      let allowed = 'allowed: {
        let Ok(state) = navigation.state.lock() else {
          break 'allowed false;
        };
        if state.closed {
          break 'allowed url.as_str() == "about:blank";
        }
        let opened = state.popup_opened.as_ref();
        if popup::tab_under(
          opened,
          main_frame,
          &state.page.url,
          url,
          std::time::Instant::now(),
        ) {
          break 'allowed false;
        }
        url.as_str() == "about:blank" || is_http(url)
      };
      #[cfg(target_os = "macos")]
      let allowed = allowed
        && !wry::navigation_modifiers().is_some_and(|input| {
          opens_background_tab(
            main_frame,
            new_window,
            input.navigation_type == objc2_web_kit::WKNavigationType::LinkActivated,
            input
              .modifier_flags
              .contains(objc2_app_kit::NSEventModifierFlags::Command),
            input.button_number,
            url,
          ) && queue_background_tab(&deciding_app, &deciding_label, &navigation, url.clone())
        });
      #[cfg(target_os = "macos")]
      let (navigation_type, button) = wry::navigation_modifiers().map_or((-1, -1), |input| {
        (input.navigation_type.0, input.button_number)
      });
      #[cfg(not(target_os = "macos"))]
      let (navigation_type, button) = (-1, -1);
      tracing::debug!(
        target: LINK_TRACE_TARGET,
        main_frame,
        new_window,
        navigation_type,
        button,
        allowed,
        "catch navigation"
      );
      if new_window {
        // A link or script opens a new window: `requested` logs next, unless the engine drops it.
        if allowed {
          tracing::info!(step = "decide", outcome = "ok", "catch new-tab");
        } else {
          tracing::warn!(step = "decide", outcome = "refused", "catch new-tab");
        }
      }
      if allowed && is_http(url) {
        super::adblock::deciding(&deciding_app, &deciding_label, url);
      }
      allowed
    })
    .on_page_load(move |_, load| {
      if is_http(load.url()) && matches!(load.event(), PageLoadEvent::Started) {
        super::adblock::navigating(&blocker_app, &blocker_label, load.url());
      }
      let mut state = match loading.state.lock() {
        Ok(state) => state,
        Err(_) => return,
      };
      if state.closed {
        if load.url().as_str() == "about:blank" && matches!(load.event(), PageLoadEvent::Finished) {
          if let Some(send) = state.blank_loaded.take() {
            let _ = send.send(());
          }
        }
        return;
      }
      if state.load_failed && !matches!(load.event(), PageLoadEvent::Started) {
        return;
      }
      if !is_http(load.url()) {
        if load.url().as_str() == "about:blank" && state.used {
          state.loading = false;
          state.load_failed = false;
          if is_http(&state.page.url) {
            state.page.navigated(blank());
          }
        }
        return;
      }
      state.load_failed = false;
      state.loading = matches!(load.event(), PageLoadEvent::Started);
      if !state.used {
        state.used = true;
        if !state.quiet {
          crate::usage::count_catch(&usage_app, crate::usage::keys::CATCH_USED, load.url());
        }
      }
      if state.loading || state.page.url != *load.url() {
        state.page.navigated(load.url().clone());
      }
    })
    // Replay the request through Nezzel's file engine and cancel the native download.
    .on_download(move |page, event| {
      if let DownloadEvent::Requested { url, .. } = event {
        let decision = downloads.active().ok().and_then(|mut state| {
          let notice = state.hands_off_download(&url, Instant::now())?;
          Some((
            notice,
            state.page.url.clone(),
            state.page.document.user_agent.clone(),
          ))
        });
        match decision {
          Some((FileDownloadNotice::Saving { name }, page_url, agent)) => {
            let app = download_app.clone();
            let target = download_target.clone();
            tauri::async_runtime::spawn(async move {
              use tauri_plugin_opener::OpenerExt;
              let saved =
                handoff::save_page_file(&app, page, &page_url, &url, agent.as_deref()).await;
              let notice = match after_page_file(saved, name) {
                Ok(notice) => notice,
                Err(name) if app.opener().open_url(url.as_str(), None::<&str>).is_ok() => {
                  FileDownloadNotice::Opened { name }
                }
                Err(name) => FileDownloadNotice::Failed { name },
              };
              if matches!(notice, FileDownloadNotice::Failed { .. }) {
                tracing::warn!("catch file download hand-off failed");
              }
              let _ = app.emit_to(EventTarget::webview(&target), FILE_DOWNLOAD_EVENT, notice);
            });
          }
          Some((notice, _, _)) => {
            let _ = download_app.emit_to(
              EventTarget::webview(&download_target),
              FILE_DOWNLOAD_EVENT,
              notice,
            );
          }
          None => {}
        }
      }
      false
    })
    .on_new_window(move |url, features| {
      let answer = popup::requested(&app, &opener, &popups, &url, features);
      let allowed = !matches!(&answer, NewWindowResponse::Deny);
      tracing::debug!(target: LINK_TRACE_TARGET, allowed, "catch new-window decision");
      answer
    })
}

fn attach_events(app: &AppHandle, id: &str, window: &Window) {
  let (app, id) = (app.clone(), id.to_owned());
  window.on_window_event(move |event| match event {
    WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
      if let Ok(browser) = app.state::<CatchWindows>().browser(&id) {
        layout(&app, &id, &browser, false);
      }
    }
    WindowEvent::CloseRequested { api, .. } => {
      api.prevent_close();
      let (app, id) = (app.clone(), id.clone());
      tauri::async_runtime::spawn(async move {
        let _ = close(&app, &id).await;
      });
    }
    WindowEvent::Destroyed => discard(&app, &id),
    WindowEvent::ThemeChanged(theme) => {
      if let Some(window) = app.get_window(&id) {
        let settings = app.state::<crate::SharedConfig>().load();
        let _ =
          window.set_background_color(Some(browser_surface(&settings.appearance.theme, *theme)));
      }
    }
    _ => {}
  });
}

fn discard(app: &AppHandle, id: &str) {
  let registry = app.state::<CatchWindows>();
  registry.forget_idle(id);
  keys::forget(app, id);
  super::site_tools::forget_window(id);
  let Some(browser) = registry.remove_browser(id) else {
    return;
  };
  let shut = browser
    .state
    .lock()
    .map(|mut state| state.shut())
    .unwrap_or_default();
  for (tab, session) in shut.pages {
    let label = tab_label(id, &tab);
    native::forget_agent(&label);
    registry.remove(&label);
    for popup in session.stop() {
      let _ = popup.destroy();
    }
  }
  // The window and its webviews are gone.
  for store in shut.stores {
    drop_store(app, id, &store);
  }
}

pub(super) async fn close(app: &AppHandle, id: &str) -> Result<(), &'static str> {
  let registry = app.state::<CatchWindows>();
  let Some(browser) = registry.remove_browser(id) else {
    return Ok(());
  };
  super::site_tools::forget_window(id);
  let (reusable, Shut { pages, stores }) = {
    let Ok(mut state) = browser.state.lock() else {
      return Ok(());
    };
    (state.reusable(), state.shut())
  };
  let window = app.get_window(id);
  // Gone from view at once; the blank pages below stop playback.
  let hidden = window.as_ref().is_some_and(|window| window.hide().is_ok());
  // Let the blank navigations stop playback before destroying the native window.
  futures::future::join_all(
    pages
      .iter()
      .map(|(tab, session)| drop_page(app, tab_label(id, tab), session)),
  )
  .await;
  // Their webviews are closed: a private tab's store is forgotten with its window.
  for store in stores {
    drop_store(app, id, &store);
  }
  if let Some(window) = window {
    if hidden && reusable && registry.park(id, browser.clone()) {
      // The shell keeps running hidden: show it an empty window so nothing stale is left on reopen.
      if let Ok(mut state) = browser.state.lock() {
        state.reopen(false);
        state.closed = true;
        if state.shell_loaded {
          let snapshot = state.snapshot(id);
          let _ = app.emit_to(EventTarget::webview(bar_label(id)), EVENT, snapshot);
        }
      }
      return Ok(());
    }
    window.destroy().map_err(|_| "catchCloseFailed")?;
  }
  Ok(())
}

async fn eval_drain(page: &Webview) -> Option<String> {
  // One string the hook serialized with its own JSON.stringify (captured before page scripts):
  // a page can't make the engine serialize an odd value. Anything else yields "".
  const SCRIPT: &str = r#"(() => {
    try {
      const snapshot = window.__nezzelCatch?.snapshot();
      if (typeof snapshot === "string" && snapshot.length <= 2097152) return snapshot;
    } catch {}
    return "";
  })()"#;
  eval_text(page, SCRIPT).await
}

/// The engine's JSON for what `script` evaluates to, when it answers in time.
async fn eval_text(page: &Webview, script: &str) -> Option<String> {
  let (send, receive) = tokio::sync::oneshot::channel();
  let send = Mutex::new(Some(send));
  page
    .eval_with_callback(script, move |raw| {
      if let Ok(mut send) = send.lock() {
        if let Some(send) = send.take() {
          let _ = send.send(raw);
        }
      }
    })
    .ok()?;
  tokio::time::timeout(Duration::from_millis(850), receive)
    .await
    .ok()?
    .ok()
}

async fn drain_loop(
  app: AppHandle,
  id: String,
  session: Arc<Session>,
  page: Webview,
  manifests: Arc<dyn ManifestFetcher>,
) {
  let mut next = tokio::time::Instant::now() + TICK_BUSY;
  let mut probes = FuturesUnordered::new();
  let mut thumbnails = FuturesUnordered::new();
  let mut probe_generation = None;
  loop {
    tokio::select! {
      _ = tokio::time::sleep_until(next) => {
        next = tokio::time::Instant::now() + TICK_BUSY;
        if session.active().is_err() {
          break;
        }
        // Status and address in one main-thread round trip.
        let (status, url) = match native::inspect(&page).await {
          Ok((status, url)) => (status, url.unwrap_or_else(blank)),
          Err("catchWindowClosed") => break,
          Err(_) => continue,
        };
        let (generation, status, crashed, web) = {
          let Ok(mut state) = session.active() else { break };
          #[cfg(windows)]
          let status = super::state::BrowserStatus { loading: state.loading, ..status };
          let status = BrowserStatus { loading: !state.load_failed && status.loading, ..status };
          let web = is_http(&url) && !state.load_failed;
          if web && !status.loading && state.page.moved_to(&url) {
            state.page.navigated(url.clone());
          }
          (state.page.generation, status, state.page.document.crashed, web)
        };
        if probe_generation != Some(generation) {
          probes.clear();
          thumbnails.clear();
          probe_generation = Some(generation);
        }
        // A dead page has nothing to read until the user reloads it.
        let drain = session.drain.lock().await;
        let raw = if web && !crashed { eval_drain(&page).await } else { None };
        let seen = if web && !crashed { popup::read(&id, &page).await } else { None };
        let Ok(mut state) = session.active() else { break };
        if state.page.generation != generation {
          continue;
        }
        let drained = raw.is_some();
        if let Some(raw) = raw {
          state.page.apply_drain(&url, &raw);
        }
        drop(drain);
        if popup::apply(&mut state, seen) {
          drop(state);
          popup::closed(&app, &id);
          break;
        }
        if state.gate_open != state.page.popup.allow_once {
          state.gate_open = state.page.popup.allow_once;
          popup::gate(&page, state.gate_open);
        }
        state.last_browser_status = status;
        // A tab the user sees tries harder before it shows an encrypted verdict.
        if state.shown {
          let mobile = state.mobile;
          if let Some(retry) = state.page.next_retry(mobile, std::time::Instant::now()) {
            tauri::async_runtime::spawn(run_retry(
              app.clone(),
              id.clone(),
              session.clone(),
              retry,
              state.page.url.clone(),
              state.page.document.user_agent.clone(),
            ));
          }
        }
        count_drm(&app, &mut state.page);
        // Never for a hidden session or a private tab.
        if drained && state.offers_signin() {
          super::site_tools::observe(&app, &id, &state.page);
        }
        // A proxied window fetches nothing beside its pages: these requests connect directly.
        while !state.proxied && probes.len() < MAX_ACTIVE_PROBES {
          let Some((url, response_url)) = state.page.document.next_manifest() else { break };
          let manifests = manifests.clone();
          probes.push(async move {
            let info = inspect_manifest(manifests.as_ref(), response_url.clone()).await;
            (generation, url, response_url, info)
          });
        }
        // A page the user sees; automatic catch finds its own after the hidden page is gone.
        if state.shown && !state.proxied && thumbnails.is_empty() {
          if let Some(job) = state.page.thumbnail_job() {
            thumbnails.push(page_thumbnail(app.clone(), page.clone(), job, generation));
          }
        }
        let playing = state.page.document.playing;
        next = tokio::time::Instant::now()
          + if state.linked && !state.background {
            // A sign-in pop-up that closes itself goes away at once.
            TICK_BUSY
          } else if state.background {
            background_interval(playing)
          } else {
            tick_interval(web, status.loading, playing)
          };
        drop(state);
        notify(&app, &id);
      }
      Some((generation, found)) = thumbnails.next(), if !thumbnails.is_empty() => {
        let Ok(mut state) = session.active() else { break };
        if state.page.generation == generation && found.is_some() {
          state.page.document.thumbnail = found;
          drop(state);
          notify(&app, &id);
        }
      }
      Some((generation, url, response_url, info)) = probes.next(), if !probes.is_empty() => {
        let Ok(mut state) = session.active() else { break };
        if state.page.generation == generation {
          if let Some(info) = info {
            state.page.document.inspected_response(&url, &response_url, info);
            count_drm(&app, &mut state.page);
            // Show the result now, not at the next tick.
            drop(state);
            notify(&app, &id);
          }
        }
      }
    }
  }
}

/// Runs one step of a visible tab's retry ladder (`PageState::next_retry`).
async fn run_retry(
  app: AppHandle,
  id: String,
  session: Arc<Session>,
  retry: Retry,
  url: Url,
  agent: Option<String>,
) {
  match retry {
    Retry::Mobile | Retry::Restore => {
      let on = retry == Retry::Mobile;
      let _ = super::site_tools::switch_identity(&app, &id, &session, on).await;
    }
    Retry::Ytdlp => {
      let result = super::commands::page_probe(&app, &id, &url, agent).await;
      let Ok(mut state) = session.active() else {
        return;
      };
      // A reload of the same page keeps the result; another page does not.
      if state.page.url == url {
        if let Ok((candidate, handoff, formats)) = result {
          state.page.document.rescue(candidate, handoff, formats);
        }
        state.page.ytdlp_done();
      }
      drop(state);
      notify(&app, &id);
    }
  }
}

/// The page's main video's picture; a frame is fetched with the cookies the page holds for it.
async fn page_thumbnail(
  app: AppHandle,
  page: Webview,
  job: super::state::ThumbnailJob,
  generation: u64,
) -> (u64, Option<String>) {
  use super::thumbnail::{find, media_cookies, user_agent, Headers, Media};
  let user_agent = user_agent(job.user_agent.as_deref());
  // Cookies are read only when the frame is the source left.
  let media = async {
    let (url, hls, frame_url) = job.media?;
    let cookies = super::handoff::media_cookies(page, url.clone())
      .await
      .unwrap_or_default();
    Some(Media {
      headers: Headers {
        referer: Some(frame_url),
        user_agent: user_agent.clone(),
        cookies: media_cookies(&url, cookies),
      },
      url,
      hls,
    })
  };
  let found = find(&app, job.pictures, &job.page, Some(&user_agent), media).await;
  (generation, found)
}

/// A master playlist names no length: it takes the length of its first variant, so a film whose
/// player has not started yet still ranks above a trailer that has. The variant's address is
/// read from where the master answered, after redirects.
async fn inspect_manifest(
  fetcher: &dyn ManifestFetcher,
  url: Url,
) -> Option<manifest::ManifestInfo> {
  let (from, mut info) = inspect_one(fetcher, url).await?;
  if info.duration_seconds.is_none() && !info.is_live && !info.has_drm {
    let variant = info
      .variants
      .first()
      .and_then(|variant| super::candidate::normalize_url(&variant.uri, Some(&from)));
    if let Some(variant) = variant {
      if let Some((_, measured)) = inspect_one(fetcher, variant).await {
        info.has_drm |= measured.has_drm;
        info.is_live |= measured.is_live;
        info.duration_seconds = measured.duration_seconds.filter(|_| !measured.is_live);
      }
    }
  }
  Some(info)
}

async fn inspect_one(
  fetcher: &dyn ManifestFetcher,
  url: Url,
) -> Option<(Url, manifest::ManifestInfo)> {
  let (from, text) = tokio::time::timeout(Duration::from_secs(5), fetcher.fetch(url))
    .await
    .ok()??;
  if text.len() > MAX_MANIFEST_BYTES {
    return None;
  }
  Some((from, manifest::inspect(&text)?))
}

/// Pop-ups as linked tabs.
#[path = "popup.rs"]
mod popup;

/// The never-shown page of automatic catch.
#[path = "hidden.rs"]
pub(super) mod hidden;

#[cfg(test)]
#[path = "window_tests.rs"]
mod tests;
