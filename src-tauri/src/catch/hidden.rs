//! The page automatic catch runs: a catch page in a window that is never shown. Real size so
//! players lay out, off-screen, never focused, out of the taskbar and window cycling. Its cookie
//! store is thrown away after each use; pop-ups, page downloads and script dialogs go nowhere.

use super::*;
use crate::catch::{
  auto::{self, Candidate, Fail, Probed, Snapshot, View},
  commands::{self, HandoffError, Kept},
  route::{Guard, Route},
};
use tauri::utils::config::BackgroundThrottlingPolicy;

const SIZE: (f64, f64) = (1280., 720.);
const OFF_SCREEN: f64 = -32000.;
/// The hidden page only plays media: it gets no WebRTC, whose sockets leave beside the guard
/// route. The engines are told too (`platform::quiet`, the Windows arguments); this covers a
/// frame an engine setting missed.
const NO_WEBRTC_SCRIPT: &str = "for(const name of['RTCPeerConnection','webkitRTCPeerConnection',\
  'RTCDataChannel','RTCIceTransport','RTCDtlsTransport','RTCSctpTransport'])\
  {try{delete globalThis[name]}catch(_){}}";

#[derive(Default)]
pub(in crate::catch) struct HiddenWindows {
  /// macOS never frees a closed WKWebView (wry retains it on drop): one emptied page is kept
  /// for the next session instead of leaking one per pasted link.
  parked: Mutex<Parked>,
  /// The admitting route every hidden page connects through, started with the first page.
  guard: Guard,
}

#[derive(Default)]
struct Parked {
  /// With the address of the route its engine was given at creation.
  page: Option<(String, Arc<Session>, std::net::SocketAddr)>,
  stopped: bool,
}

struct HiddenPage {
  app: AppHandle,
  id: String,
  session: Arc<Session>,
  /// The guard route this page's engine connects through.
  route: std::net::SocketAddr,
}

/// Where the hidden page's engine connects: every connection of every frame goes through the
/// guard route, which admits public addresses only (`route::Route::guard`).
fn proxy_url(guard: &Route) -> Option<Url> {
  Url::parse(&format!("socks5://{}", guard.address)).ok()
}

/// Windows: true once a hidden page of this run has taken a profile folder. Held while the
/// folders are swept, so the sweep at startup never removes the folder of a page being made.
#[cfg(windows)]
static PROFILE_TAKEN: Mutex<bool> = Mutex::new(false);

#[cfg(windows)]
fn profile_root(app: &AppHandle) -> Option<std::path::PathBuf> {
  Some(app.path().app_local_data_dir().ok()?.join("catch-hidden"))
}

/// Folders of closed pages; one still held by a live engine stays until the next try.
#[cfg(windows)]
fn sweep(root: &std::path::Path) {
  if let Ok(entries) = std::fs::read_dir(root) {
    for entry in entries.flatten() {
      let _ = std::fs::remove_dir_all(entry.path());
    }
  }
}

/// Windows, at startup: the last hidden page of the run before left its profile folder.
#[cfg(windows)]
pub(in crate::catch) fn sweep_profiles(app: &AppHandle) {
  let Some(root) = profile_root(app) else {
    return;
  };
  std::thread::spawn(move || {
    if let Ok(taken) = PROFILE_TAKEN.lock() {
      if !*taken {
        sweep(&root);
      }
    }
  });
}

/// Windows: the engine's browser arguments (wry's defaults plus the proxy) and a profile folder
/// of the page's own. WebView2 takes arguments per profile folder, and Chromium skips the proxy
/// for loopback unless told not to; WebRTC may not send UDP around the proxy. Names are looked
/// up by the route only (SOCKS5 carries the name): the engine's own resolver answers "not found"
/// for everything, so no lookup, prefetch included, goes to the system's DNS.
#[cfg(windows)]
fn engine_profile(app: &AppHandle, guard: &Route) -> Option<(std::path::PathBuf, String)> {
  let root = profile_root(app)?;
  let mut taken = PROFILE_TAKEN.lock().ok()?;
  sweep(&root);
  let folder = root.join(uuid::Uuid::new_v4().simple().to_string());
  std::fs::create_dir_all(&folder).ok()?;
  *taken = true;
  let arguments = format!(
    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
     --autoplay-policy=no-user-gesture-required \
     --proxy-server=socks5://{} --proxy-bypass-list=<-loopback> \
     --host-resolver-rules=\"MAP * ~NOTFOUND , EXCLUDE 127.0.0.1\" \
     --force-webrtc-ip-handling-policy=disable_non_proxied_udp",
    guard.address
  );
  Some((folder, arguments))
}

/// Whether this system's engine can be told to open no WebRTC connection. Where it cannot,
/// there is no hidden page (`auto::Gate::unguarded`).
pub(in crate::catch) fn closes_webrtc() -> bool {
  platform::closes_webrtc()
}

fn create(
  app: &AppHandle,
  url: &Url,
  guard: &Route,
) -> Result<(String, Arc<Session>), &'static str> {
  if !closes_webrtc() {
    return Err("catchOpenFailed");
  }
  let id = format!("catch-{}", uuid::Uuid::new_v4().simple());
  let session = Arc::new(Session::new(url.clone()));
  let navigation = session.clone();
  let (deciding_app, deciding_label) = (app.clone(), id.clone());
  let builder = page_with(app, &id, &session, false)
    // A hidden page never opens the user's browser.
    .on_download(|_, _| false)
    .initialization_script_for_all_frames(capture_script(true))
    .proxy_url(proxy_url(guard).ok_or("catchOpenFailed")?);
  #[cfg(windows)]
  let builder = {
    let (folder, arguments) = engine_profile(app, guard).ok_or("catchOpenFailed")?;
    builder
      .data_directory(folder)
      .additional_browser_args(&arguments)
  };
  let builder = builder
    .initialization_script_for_all_frames(NO_WEBRTC_SCRIPT)
    // A throwaway store: the saved catch profile belongs to the window the user sees.
    .incognito(true)
    // macOS 14+: a page nobody sees keeps its timers and playback.
    .background_throttling(BackgroundThrottlingPolicy::Disabled)
    .on_new_window(|_, _| NewWindowResponse::Deny)
    .on_navigation(move |url| {
      let allowed = navigation.state.lock().is_ok_and(|state| {
        url.as_str() == "about:blank" || (!state.closed && is_http(url) && auto::may_be_public(url))
      });
      if allowed && is_http(url) {
        crate::catch::adblock::deciding(&deciding_app, &deciding_label, url);
      }
      allowed
    });
  let window = WindowBuilder::new(app, &id)
    .title(&app.package_info().name)
    .inner_size(SIZE.0, SIZE.1)
    .position(OFF_SCREEN, OFF_SCREEN)
    .visible(false)
    .focused(false)
    .focusable(false)
    .skip_taskbar(true)
    .decorations(false)
    .shadow(false)
    .build()
    .map_err(|_| "catchOpenFailed")?;
  let page = match window.add_child(
    builder,
    LogicalPosition::new(0., 0.),
    LogicalSize::new(SIZE.0, SIZE.1),
  ) {
    Ok(page) => page,
    Err(_) => {
      let _ = window.destroy();
      return Err("catchOpenFailed");
    }
  };
  platform::conceal(&window);
  platform::quiet(&page);
  crate::catch::adblock::attach(app, &page, &session);
  #[cfg(not(target_os = "macos"))]
  {
    let (app, id) = (app.clone(), id.clone());
    native::watch_crash(&page, move || {
      renderer_crashed(&app, &id);
    });
  }
  Ok((id, session))
}

fn destroy(app: &AppHandle, id: &str, session: &Session) {
  app.state::<CatchWindows>().remove(id);
  session.stop();
  if let Some(window) = app.get_window(id) {
    let _ = window.destroy();
  }
}

/// Registers the session, starts its drain loop and loads the link. The window is never shown.
fn begin(app: &AppHandle, id: &str, session: &Arc<Session>, url: &Url) -> Result<(), &'static str> {
  let page = page(app, id)?;
  app.state::<CatchWindows>().insert(id, session.clone())?;
  let drain = tauri::async_runtime::spawn(drain_loop(
    app.clone(),
    id.to_owned(),
    session.clone(),
    page.clone(),
    Arc::new(PinnedManifestFetcher),
  ));
  let drain = drain.inner().abort_handle();
  match session.active() {
    Ok(mut state) => {
      state.drain = Some(drain);
      // Counted as `catch_auto_used` by the caller, not as a use of the visible window.
      state.used = true;
    }
    Err(error) => {
      drain.abort();
      return Err(error);
    }
  }
  crate::catch::adblock::navigate(app, &page, session, url.clone())
}

pub(in crate::catch) async fn open(app: &AppHandle, url: &Url) -> Option<Arc<dyn auto::Page>> {
  let hidden = app.try_state::<HiddenWindows>()?;
  if hidden.parked.lock().map_or(true, |parked| parked.stopped) {
    return None;
  }
  // No guard route, no hidden page: it never connects around the public-address rule.
  let guard = hidden.guard.route().await.ok()?;
  let parked = {
    let mut parked = hidden.parked.lock().ok()?;
    if parked.stopped {
      return None;
    }
    parked.page.take()
  };
  let (id, session) = match parked {
    // A parked page whose engine points at a route that has ended is not used again.
    Some((id, session, route))
      if route == guard.address
        && page(app, &id).is_ok()
        && session.reopen(url.clone(), false).is_ok() =>
    {
      (id, session)
    }
    stale => {
      if let Some((id, session, _)) = stale {
        destroy(app, &id, &session);
      }
      create(app, url, &guard).ok()?
    }
  };
  // The user's sign-ins for the link's site, before the first load (copied in, never back).
  let copied = match page(app, &id) {
    Ok(view) => crate::catch::hidden_signin::copy(app, &view, url).await,
    Err(_) => true,
  };
  // The app may have stopped while the copy ran: nothing loads after that.
  let stopped = hidden.parked.lock().map_or(true, |parked| parked.stopped);
  if !copied || stopped || begin(app, &id, &session, url).is_err() {
    discard(app, &id, &session).await;
    return None;
  }
  Some(Arc::new(HiddenPage {
    app: app.clone(),
    id,
    session,
    route: guard.address,
  }))
}

/// Whether a closing page is empty: its own blank load finished and the address read back is
/// not another page's. An address the engine could not give says nothing against the load.
fn emptied(blanked: bool, now: Option<&Url>) -> bool {
  blanked && now.is_none_or(|url| url.as_str() == "about:blank")
}

/// macOS keeps a closed page's engine (and its store, with any copied sign-in) for the life of
/// the app: the store is emptied on every way out, before parking is even considered. Returns
/// whether the page may be parked.
async fn wipe_then_park(
  mac: bool,
  emptied: bool,
  mobile: bool,
  forget_all: impl std::future::Future<Output = bool>,
) -> bool {
  mac && forget_all.await && emptied && !mobile
}

/// A page that is not used: its store emptied (macOS), then the page destroyed.
async fn discard(app: &AppHandle, id: &str, session: &Session) {
  if let Ok(view) = page(app, id) {
    wipe_then_park(
      cfg!(target_os = "macos"),
      false,
      false,
      platform::forget_all(&view),
    )
    .await;
  }
  destroy(app, id, session);
}

async fn close(app: &AppHandle, id: &str, session: &Arc<Session>, route: std::net::SocketAddr) {
  // A page that presented the Mobile version keeps that identity: never parked for reuse.
  let mobile = session.active().map_or(true, |state| state.mobile);
  app.state::<CatchWindows>().remove(id);
  session.stop();
  let Ok(page) = page(app, id) else {
    destroy(app, id, session);
    return;
  };
  // The blank page stops playback before anything else.
  let blanked = session.blanked(|| page.navigate(blank()).is_ok()).await;
  let emptied = emptied(blanked, native::url(&page).await.as_ref());
  let mac = cfg!(target_os = "macos");
  if wipe_then_park(mac, emptied, mobile, platform::forget_all(&page)).await {
    if let Some(hidden) = app.try_state::<HiddenWindows>() {
      if let Ok(mut parked) = hidden.parked.lock() {
        if !parked.stopped && parked.page.is_none() {
          parked.page = Some((id.to_owned(), session.clone(), route));
          return;
        }
      }
    }
  }
  destroy(app, id, session);
}

pub(in crate::catch) fn stop_all(app: &AppHandle) {
  let Some(hidden) = app.try_state::<HiddenWindows>() else {
    return;
  };
  let parked = {
    let Ok(mut parked) = hidden.parked.lock() else {
      return;
    };
    parked.stopped = true;
    parked.page.take()
  };
  if let Some((id, session, _)) = parked {
    destroy(app, &id, &session);
  }
}

fn fail(error: HandoffError) -> Fail {
  match error {
    HandoffError::DrmProtected => Fail::Encrypted,
    HandoffError::Failed("liveStreamUnsupported") => Fail::Live,
    HandoffError::Failed("accessForbidden403") => Fail::Forbidden,
    HandoffError::Failed(_) => Fail::Other,
  }
}

impl auto::Page for HiddenPage {
  fn view(&self) -> Option<View> {
    let state = self.session.active().ok()?;
    Some(state.page.auto_view(state.loading))
  }

  fn final_view(&self) -> BoxFuture<'_, Option<View>> {
    Box::pin(async move {
      let _drain = self.session.drain.lock().await;
      let generation = self.session.active().ok()?.page.generation;
      let page = page(&self.app, &self.id).ok()?;
      let raw = eval_text(&page, crate::catch::state::auto_view::FINAL_SNAPSHOT).await?;
      let mut state = self.session.active().ok()?;
      if state.loading || state.load_failed || state.page.generation != generation {
        return None;
      }
      state.page.auto_final_view(&raw)
    })
  }

  fn capture<'a>(
    &'a self,
    candidate: &'a Candidate,
  ) -> BoxFuture<'a, Result<Arc<dyn Snapshot>, Fail>> {
    Box::pin(async move {
      let request = {
        let state = self.session.active().map_err(|_| Fail::Other)?;
        commands::request(&self.id, &state.page, &candidate.id).map_err(|_| Fail::Other)?
      };
      // Reads the cookies the page holds for the media address: the page must still be open.
      let kept = Kept::new(&self.app, request).await.map_err(fail)?;
      Ok(Arc::new(Caught {
        app: self.app.clone(),
        kept,
        thumbnail: std::sync::OnceLock::new(),
        measure: candidate.duration.is_none() && candidate.kind == "hls",
      }) as Arc<dyn Snapshot>)
    })
  }

  fn mobile(&self) -> BoxFuture<'_, bool> {
    Box::pin(async move {
      let Ok(page) = page(&self.app, &self.id) else {
        return false;
      };
      match self.session.active() {
        // Marked before the switch: a page that may present iPhone is never parked for reuse.
        Ok(mut state) if !state.mobile => state.mobile = true,
        _ => return false,
      }
      let agent = crate::catch::site_tools::mobile_agent();
      if native::set_agent(&page, Some(agent)).await.is_err() {
        return false;
      }
      {
        let Ok(mut state) = self.session.active() else {
          return false;
        };
        // The desktop page's evidence goes now, not when the reload starts.
        let url = state.page.url.clone();
        state.page.navigated(url);
      }
      page.reload().is_ok()
    })
  }

  fn close(&self) -> BoxFuture<'_, ()> {
    Box::pin(close(&self.app, &self.id, &self.session, self.route))
  }

  #[cfg(feature = "catch-matrix")]
  fn note(&self) -> Option<serde_json::Value> {
    let state = self.session.active().ok()?;
    Some(state.page.matrix_note(state.loading))
  }

  #[cfg(feature = "catch-matrix")]
  fn inspect_dom(&self) -> BoxFuture<'_, Option<serde_json::Value>> {
    Box::pin(async move {
      let page = page(&self.app, &self.id).ok()?;
      let raw = eval_text(&page, MATRIX_DOM).await?;
      // The engine answers with the JSON of the string the script returned.
      let text: String = serde_json::from_str(&raw).ok()?;
      serde_json::from_str(&text).ok()
    })
  }
}

/// Developer sweep only: what the top document looks like (hosts only, never addresses).
#[cfg(feature = "catch-matrix")]
const MATRIX_DOM: &str = include_str!("matrix_dom.js");

struct Caught {
  app: AppHandle,
  kept: Kept,
  /// The picture found for the row, kept for the queued download's row.
  thumbnail: std::sync::OnceLock<String>,
  /// A master playlist names no length: one of its variants does.
  measure: bool,
}

async fn playlist_duration(fetcher: &dyn ManifestFetcher, url: &Url) -> Result<Option<f64>, Fail> {
  match inspect_manifest(fetcher, url.clone()).await {
    Some(info) if info.has_drm => Err(Fail::Encrypted),
    Some(info) if info.is_live => Err(Fail::Live),
    info => Ok(info.and_then(|info| info.duration_seconds)),
  }
}

impl Snapshot for Caught {
  fn probe(&self) -> BoxFuture<'_, Result<Probed, Fail>> {
    Box::pin(async move {
      let formats = self.kept.formats(&self.app).await.map_err(fail)?;
      let duration = if self.measure {
        playlist_duration(&PinnedManifestFetcher, self.kept.media_url()).await?
      } else {
        None
      };
      Ok(Probed { formats, duration })
    })
  }

  fn thumbnail(&self, pictures: Vec<Url>) -> BoxFuture<'_, Option<String>> {
    Box::pin(async move {
      let found = super::super::thumbnail::find(
        &self.app,
        pictures,
        self.kept.page_url(),
        self.kept.user_agent(),
        async { self.kept.frame_media() },
      )
      .await?;
      let _ = self.thumbnail.set(found.clone());
      Some(found)
    })
  }

  fn save(
    &self,
    format_id: Option<String>,
    download_id: String,
  ) -> BoxFuture<'_, Result<(), Fail>> {
    Box::pin(async move {
      self
        .kept
        .save(
          &self.app,
          format_id.as_deref(),
          download_id,
          self.thumbnail.get().cloned(),
        )
        .await
        .map_err(fail)
    })
  }
}

#[cfg(target_os = "macos")]
mod platform {
  use super::*;
  use block2::RcBlock;
  use objc2::{msg_send, sel, ClassType, MainThreadMarker};
  use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};
  use objc2_foundation::NSDate;
  use objc2_web_kit::{WKPreferences, WKWebView, WKWebsiteDataStore};

  /// Should anything ever order the window in, it still takes no clicks and stays out of the
  /// Window menu, window cycling and Mission Control.
  pub(super) fn conceal(window: &Window) {
    let target = window.clone();
    let _ = window.run_on_main_thread(move || {
      let Ok(pointer) = target.ns_window() else {
        return;
      };
      // Tauri hands out its live NSWindow; this closure runs on the main thread.
      let window = unsafe { &*pointer.cast::<NSWindow>() };
      window.setIgnoresMouseEvents(true);
      window.setExcludedFromWindowsMenu(true);
      window.setHidesOnDeactivate(false);
      window.setCollectionBehavior(
        NSWindowCollectionBehavior::Stationary
          | NSWindowCollectionBehavior::IgnoresCycle
          | NSWindowCollectionBehavior::FullScreenAuxiliary,
      );
    });
  }

  /// WebKit's switch for `RTCPeerConnection` is a preference without a public name
  /// (`WKPreferencesPrivate.h`, macOS 10.13.4+). Asked of the class, so no page is needed.
  pub(super) fn closes_webrtc() -> bool {
    WKPreferences::class()
      .instance_method(sel!(_setPeerConnectionEnabled:))
      .is_some()
  }

  /// WKWebView shows no script dialog without a delegate method for it (wry has none), plays
  /// without a user action (wry's default) and has no public mute: the page hooks mute.
  /// WebRTC is turned off in the page's preferences, which every later document reads.
  pub(super) fn quiet(page: &Webview) {
    let _ = page.with_webview(|view| {
      // Tauri supplies a live WKWebView on the main thread. The setter exists
      // (`closes_webrtc`, checked before the page was made) and takes one BOOL.
      unsafe {
        let view = &*view.inner().cast::<WKWebView>();
        let preferences = view.configuration().preferences();
        if closes_webrtc() {
          let _: () = msg_send![&*preferences, _setPeerConnectionEnabled: false];
        }
        // Keep native animation and visibility running if WebKit exposes this preference.
        // Otherwise the hidden view's document-start hooks supply the fallback.
        if WKPreferences::class()
          .instance_method(sel!(_setWindowOcclusionDetectionEnabled:))
          .is_some()
        {
          let _: () = msg_send![&*preferences, _setWindowOcclusionDetectionEnabled: false];
        }
      }
    });
  }

  /// Empties the page's throwaway store, so a reused page starts with no cookies or site data.
  pub(super) async fn forget_all(page: &Webview) -> bool {
    let (send, receive) = tokio::sync::oneshot::channel();
    let sent = page.with_webview(move |view| {
      let Some(main) = MainThreadMarker::new() else {
        return;
      };
      // Tauri supplies a live WKWebView on the main thread; the block owns what it uses.
      let view = unsafe { &*view.inner().cast::<WKWebView>() };
      let send = Mutex::new(Some(send));
      let done = RcBlock::new(move || {
        if let Some(send) = send.lock().ok().and_then(|mut send| send.take()) {
          let _ = send.send(());
        }
      });
      unsafe {
        view
          .configuration()
          .websiteDataStore()
          .removeDataOfTypes_modifiedSince_completionHandler(
            &WKWebsiteDataStore::allWebsiteDataTypes(main),
            &NSDate::distantPast(),
            &done,
          );
      }
    });
    sent.is_ok()
      && tokio::time::timeout(Duration::from_secs(3), receive)
        .await
        .is_ok_and(|done| done.is_ok())
  }
}

#[cfg(windows)]
mod platform {
  use super::*;
  use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_8;
  use windows::{
    core::Interface,
    Win32::{
      Foundation::HWND,
      UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
      },
    },
  };

  /// A tool window that never activates: no taskbar button, no Alt+Tab entry.
  pub(super) fn conceal(window: &Window) {
    let Ok(handle) = window.hwnd() else {
      return;
    };
    let handle = HWND(handle.0 as _);
    // The handle is this live window's; changing its extended style is valid from any thread.
    unsafe {
      let style = GetWindowLongPtrW(handle, GWL_EXSTYLE);
      SetWindowLongPtrW(
        handle,
        GWL_EXSTYLE,
        style | (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) as isize,
      );
    }
  }

  /// WebRTC stays inside the route by a browser argument (`engine_profile`).
  pub(super) fn closes_webrtc() -> bool {
    true
  }

  /// Muted by the engine, and no script dialog (`alert`, `confirm`, `beforeunload`) ever opens.
  pub(super) fn quiet(page: &Webview) {
    let _ = page.with_webview(|view| {
      // Tauri supplies the live WebView2 controller on its owning COM apartment.
      unsafe {
        let Ok(core) = view.controller().CoreWebView2() else {
          return;
        };
        if let Ok(settings) = core.Settings() {
          let _ = settings.SetAreDefaultScriptDialogsEnabled(false);
        }
        if let Ok(core) = core.cast::<ICoreWebView2_8>() {
          let _ = core.SetIsMuted(true);
        }
      }
    });
  }

  pub(super) async fn forget_all(_page: &Webview) -> bool {
    false
  }
}

#[cfg(target_os = "linux")]
mod platform {
  use super::*;
  use webkit2gtk::{FileChooserRequestExt, ScriptDialogType, SettingsExt, WebViewExt};

  /// X11: never mapped, placed off-screen, out of the taskbar (the builder sets all three).
  /// Wayland cannot place a window off-screen: automatic catch is off there (`auto::consider`).
  pub(super) fn conceal(_window: &Window) {}

  /// WebKitGTK has a setting for it (`quiet`).
  pub(super) fn closes_webrtc() -> bool {
    true
  }

  /// Muted by the engine, plays without a user action, and no script dialog or file chooser
  /// ever opens. No WebRTC and no device capture (their sockets leave beside the guard route),
  /// and no DNS prefetch (a lookup the route never sees).
  pub(super) fn quiet(page: &Webview) {
    let _ = page.with_webview(|view| {
      let view = view.inner();
      view.set_is_muted(true);
      if let Some(settings) = WebViewExt::settings(&view) {
        settings.set_media_playback_requires_user_gesture(false);
        settings.set_enable_webrtc(false);
        settings.set_enable_media_stream(false);
        settings.set_enable_dns_prefetching(false);
      }
      view.connect_script_dialog(|_, dialog| {
        if dialog.dialog_type() == ScriptDialogType::BeforeUnloadConfirm {
          dialog.confirm_set_confirmed(true);
        }
        true
      });
      view.connect_run_file_chooser(|_, request| {
        request.cancel();
        true
      });
    });
  }

  pub(super) async fn forget_all(_page: &Webview) -> bool {
    false
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_closed_page_is_kept_when_its_blank_load_finished_and_no_other_address_is_read() {
    let blank = blank();
    let other = Url::parse("https://example.com/watch").unwrap();
    assert!(emptied(true, Some(&blank)));
    assert!(emptied(true, None), "the address could not be read");
    assert!(!emptied(true, Some(&other)));
    assert!(!emptied(false, Some(&blank)) && !emptied(false, None));
  }

  #[tokio::test]
  async fn a_closing_page_is_wiped_on_macos_whether_or_not_it_is_parked() {
    for (mac, emptied, mobile, parks, wipes) in [
      (true, true, false, true, true),
      // Not emptied (blank load timed out), Mobile, or never begun: still wiped.
      (true, false, false, false, true),
      (true, true, true, false, true),
      (true, false, true, false, true),
      // Elsewhere the page and its store go with the window.
      (false, true, false, false, false),
    ] {
      let wiped = std::sync::atomic::AtomicBool::new(false);
      let forget = async {
        wiped.store(true, std::sync::atomic::Ordering::SeqCst);
        true
      };
      assert_eq!(wipe_then_park(mac, emptied, mobile, forget).await, parks);
      assert_eq!(wiped.into_inner(), wipes, "{mac} {emptied} {mobile}");
    }
  }

  struct Playlists;

  impl ManifestFetcher for Playlists {
    fn fetch(&self, url: Url) -> BoxFuture<'_, Option<(Url, String)>> {
      Box::pin(async move {
        let text = match url.as_str() {
          "https://cdn.example.org/film/master.m3u8" => {
            Some(include_str!("fixtures/master.m3u8").to_owned())
          }
          url if url.starts_with("https://cdn.example.org/film/") => {
            Some(include_str!("fixtures/short-aes128.m3u8").to_owned())
          }
          "https://cdn.example.org/live.m3u8" => Some("#EXTM3U\n#EXTINF:10,\na.ts".into()),
          "https://cdn.example.org/encrypted.m3u8" => Some(
            "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"skd://key\"\n#EXTINF:10,\na.ts\n#EXT-X-ENDLIST".into(),
          ),
          _ => None,
        };
        text.map(|text| (url, text))
      })
    }
  }

  #[tokio::test]
  async fn a_playlist_probe_returns_length_or_the_live_or_encrypted_verdict() {
    let master = Url::parse("https://cdn.example.org/film/master.m3u8").unwrap();
    let variant = Url::parse("https://cdn.example.org/film/any.m3u8").unwrap();
    let direct = playlist_duration(&Playlists, &variant).await;
    assert!(direct.unwrap().is_some_and(|seconds| seconds > 0.0));
    assert_eq!(playlist_duration(&Playlists, &master).await, direct);
    let missing = Url::parse("https://cdn.example.org/other.m3u8").unwrap();
    assert_eq!(playlist_duration(&Playlists, &missing).await, Ok(None));
    for (name, expected) in [("live", Fail::Live), ("encrypted", Fail::Encrypted)] {
      let url = Url::parse(&format!("https://cdn.example.org/{name}.m3u8")).unwrap();
      assert_eq!(playlist_duration(&Playlists, &url).await, Err(expected));
    }
  }

  /// The setter has no public name: a wrong one would switch automatic catch off for good.
  #[cfg(target_os = "macos")]
  #[test]
  fn this_engine_can_be_told_to_open_no_webrtc_connection() {
    assert!(closes_webrtc());
  }

  #[test]
  fn a_hidden_page_never_loads_local_or_private_hosts() {
    for (url, allowed) in [
      ("https://example.org/watch", true),
      ("https://93.184.216.34/watch", true),
      ("http://localhost:8080/", false),
      ("http://dev.localhost/", false),
      ("http://127.0.0.1/", false),
      ("http://192.168.1.1/admin", false),
      ("http://[::1]/", false),
      ("http://router/", false),
      ("http://printer.local/", false),
    ] {
      assert_eq!(
        auto::may_be_public(&Url::parse(url).unwrap()),
        allowed,
        "{url}"
      );
    }
  }
}
