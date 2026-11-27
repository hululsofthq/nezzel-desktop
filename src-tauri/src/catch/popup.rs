//! Pop-ups of a tab's page. One the user caused (a click) opens as a linked tab in the same
//! window and store, with `window.opener` kept: "Continue with…" sign-ins need it. Any other is
//! blocked and counted.

use super::*;
use tauri::webview::NewWindowFeatures;

/// Whether the engine itself stops the pop-ups no click caused, so each request that arrives is
/// the user's. macOS: once `gate` is shut. Linux: WebKitGTK's default. Windows: WebView2 says
/// per request whether a click caused it (`watch_clicks`).
const ENGINE_GATE: bool = cfg!(any(target_os = "macos", target_os = "linux"));

/// Requests remembered for one page that were not answered yet: more means some were skipped.
#[cfg_attr(not(windows), allow(dead_code))]
const MAX_CLICKS: usize = 8;

/// A page that sends itself to another site this soon after it opened a pop-up is a tab-under:
/// the pop-up shows what the user clicked and the page behind it becomes an ad.
const TAB_UNDER: std::time::Duration = std::time::Duration::from_secs(1);

fn site(url: &Url) -> Option<String> {
  let host = url.host_str()?.trim_end_matches('.');
  Some(psl::domain_str(host).unwrap_or(host).to_ascii_lowercase())
}

/// Whether the navigation being decided now is the main frame's. Windows: the handler sees the
/// main frame only (frames have their own event). macOS: the engine says (vendored wry). Linux:
/// WebKitGTK says only a frame's name, so no navigation counts as the main frame's there.
pub(super) fn main_frame_navigation() -> bool {
  #[cfg(target_os = "macos")]
  return wry::navigation_main_frame() == Some(true);
  #[cfg(not(target_os = "macos"))]
  cfg!(windows)
}

/// Whether the navigation being decided now opens a new window (macOS says, vendored wry).
pub(super) fn new_window_navigation() -> bool {
  #[cfg(target_os = "macos")]
  return wry::navigation_new_window() == Some(true);
  #[cfg(not(target_os = "macos"))]
  false
}

/// Pop-up requests the engine denied before they reached `requested` (macOS: the vendored wry
/// delegate found no window or screen), since launch. For the log only.
fn dropped() -> u64 {
  #[cfg(target_os = "macos")]
  return wry::popups_dropped();
  #[cfg(not(target_os = "macos"))]
  0
}

/// Whether the page at `page` going to `to` now is a tab-under: its main frame (`main_frame`),
/// within `TAB_UNDER` of opening a pop-up (`opened`: when, and its address), to a site that is
/// neither the page's nor the pop-up's. A frame's load is never one: a click on play may open
/// an ad and load the player from another site in the same second.
pub(super) fn tab_under(
  opened: Option<&(std::time::Instant, Url)>,
  main_frame: bool,
  page: &Url,
  to: &Url,
  now: std::time::Instant,
) -> bool {
  let Some((at, popup)) = opened.filter(|_| main_frame) else {
    return false;
  };
  if now.saturating_duration_since(*at) > TAB_UNDER || !is_http(to) || !is_http(page) {
    return false;
  }
  let target = site(to);
  target != site(page) && target != site(popup)
}

/// Windows: WebView2 said whether a click caused its request for `url`; `requested` runs for it
/// after this. Too many unanswered: all are forgotten (no click).
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn clicked(state: &mut SessionState, url: &str, user: bool) {
  if state.clicks.len() >= MAX_CLICKS {
    state.clicks.clear();
  }
  // The address as the request reaches `requested` (parsed by Tauri).
  let url = Url::parse(url).map_or_else(|_| url.to_owned(), String::from);
  state.clicks.push_back((url, user));
}

/// Whether a click caused this request for `url`: the engine's gate where it has one (unless
/// "allow once" holds it open), else the engine's word recorded for that address; none: no click.
pub(super) fn gesture(
  state: &mut SessionState,
  url: &Url,
  engine_gate: bool,
  gate_open: bool,
) -> bool {
  if engine_gate {
    return !gate_open;
  }
  let at = state
    .clicks
    .iter()
    .position(|(seen, _)| seen == url.as_str());
  at.and_then(|at| state.clicks.remove(at))
    .is_some_and(|(_, user)| user)
}

/// macOS ignores a page's own `window.close()` (nothing is destroyed), and only the real call
/// makes `closed` true for the opener. Windows and Linux would destroy the webview behind
/// Tauri's back, so there the call only leaves a mark and the tab is closed from here.
const NATIVE_CLOSE: &str = if cfg!(target_os = "macos") {
  "true"
} else {
  "false"
};

/// Main frame of every tab page: counts the page's own pop-ups that did not open (those the
/// hooks' pop-up guard stopped too), and marks a `window.close()`.
const SCRIPT: &str = r#"(() => {
  if (window.__nezzelPopup) return;
  const guard = window[Symbol.for("nezzel.popup.guard")];
  const stopped = () => { try { return guard ? Number(guard.count()) || 0 : 0; } catch { return 0; } };
  let failed = 0;
  let taken = 0;
  let closing = false;
  const open = window.open;
  const close = window.close;
  const trace = __LINK_TRACE__;
  const stringify = JSON.stringify;
  const presses = [];
  const remember = (kind, event) => {
    const mouse = event instanceof MouseEvent;
    const link = mouse && event.composedPath().find(node =>
      (node instanceof HTMLAnchorElement || node instanceof HTMLAreaElement) && node.hasAttribute("href"));
    // Modifier bits, low to high: Alt, Control, Meta, Shift.
    const mods = mouse ? +event.altKey + 2 * event.ctrlKey + 4 * event.metaKey + 8 * event.shiftKey : 0;
    if (presses.length === 16) presses.shift();
    presses.push({ kind, event, button: mouse ? event.button : -1, mods,
      target_blank: Boolean(link && link.target.toLowerCase() === "_blank"),
      scripted_open: kind === "windowOpen" });
  };
  if (trace) {
    for (const kind of ["click", "auxclick"]) {
      addEventListener(kind, event => remember(kind, event), { capture: true });
    }
  }
  window.open = function (...args) {
    if (trace) {
      try {
        const event = window.event;
        const press = event && presses.find(press => press.event === event);
        if (press) press.scripted_open = true;
        remember("windowOpen", event);
      } catch {}
    }
    const before = stopped();
    const opened = Reflect.apply(open, this ?? window, args);
    // The guard counts its own; the rest the engine refused.
    if (opened === null && stopped() === before) failed += 1;
    return opened;
  };
  window.close = function () {
    closing = true;
    if (__NATIVE_CLOSE__) Reflect.apply(close, window, []);
  };
  Object.defineProperty(window, "__nezzelPopup", {
    value: () => {
      const total = stopped();
      const seen = (failed + Math.max(0, total - taken)) + ":" + (closing ? 1 : 0) + ":" + (window.closed ? 1 : 0);
      failed = 0;
      taken = total;
      if (trace && presses.length) {
        // Sample defaultPrevented after the site's handlers have had their turn.
        try {
          return seen + "\n" + stringify(presses.splice(0).map(({ event, ...press }) => ({
            ...press, trusted: Boolean(event?.isTrusted), prevented: Boolean(event?.defaultPrevented),
          })));
        } catch {}
      }
      return seen;
    },
  });
})();"#;

/// Tells the main frame's pop-up guard whether the user allowed the next pop-up.
const GUARD_OPEN: &str =
  r#"try { window[Symbol.for("nezzel.popup.guard")]?.gate(true); } catch {}"#;
const GUARD_SHUT: &str =
  r#"try { window[Symbol.for("nezzel.popup.guard")]?.gate(false); } catch {}"#;

const READ: &str = r#"(() => {
  try {
    const seen = window.__nezzelPopup?.();
    // 18 bytes of popup state, one separator, then at most 4 KiB of trace.
    if (typeof seen === "string" && seen.length <= 4115) return seen;
  } catch {}
  return "";
})()"#;

pub(super) fn script() -> String {
  let trace = tracing::enabled!(target: LINK_TRACE_TARGET, tracing::Level::DEBUG);
  SCRIPT
    .replace("__NATIVE_CLOSE__", NATIVE_CLOSE)
    .replace("__LINK_TRACE__", if trace { "true" } else { "false" })
}

/// What a page says about its pop-ups since it was last asked.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Seen {
  /// Its own pop-ups that did not open.
  pub(super) failed: u32,
  /// It asked to be closed (by itself or by its opener).
  pub(super) closing: bool,
}

/// Reads `failed:closing:closed` plus the optional link trace from the current document.
/// A navigation can replace that document before the next read.
pub(super) fn parse(raw: &str) -> Option<Seen> {
  let text = serde_json::from_str::<String>(raw).ok()?;
  let (seen, trace) = text.split_once('\n').unwrap_or((&text, ""));
  if !trace.is_empty() && trace.len() <= 4096 {
    log_presses(trace);
  }
  if seen.len() > 18 {
    return None;
  }
  let mut parts = seen.split(':');
  let failed = parts.next()?.parse().ok()?;
  let mut flag = || match parts.next() {
    Some("0") => Some(false),
    Some("1") => Some(true),
    _ => None,
  };
  let (closing, closed) = (flag()?, flag()?);
  parts.next().is_none().then_some(Seen {
    failed,
    closing: closing || closed,
  })
}

// The page controls the reply. Only fixed kinds and typed flags reach the log.
fn log_presses(trace: &str) {
  if !tracing::enabled!(target: LINK_TRACE_TARGET, tracing::Level::DEBUG) {
    return;
  }
  #[derive(serde::Deserialize)]
  #[serde(rename_all = "camelCase")]
  enum Kind {
    Click,
    Auxclick,
    WindowOpen,
  }

  #[derive(serde::Deserialize)]
  struct Press {
    kind: Kind,
    trusted: bool,
    prevented: bool,
    button: i16,
    mods: u8,
    target_blank: bool,
    scripted_open: bool,
  }

  let Ok(presses) = serde_json::from_str::<Vec<Press>>(trace) else {
    return;
  };
  for press in presses.into_iter().take(16) {
    let kind = match press.kind {
      Kind::Click => "click",
      Kind::Auxclick => "auxclick",
      Kind::WindowOpen => "window.open",
    };
    tracing::debug!(
      target: LINK_TRACE_TARGET,
      trusted = press.trusted,
      prevented = press.prevented,
      button = press.button,
      mods = press.mods,
      target_blank = press.target_blank,
      scripted_open = press.scripted_open,
      kind,
      "catch link-press"
    );
  }
}

/// Asks a tab's page; pages outside a browser window are never asked.
pub(super) async fn read(label: &str, page: &Webview) -> Option<Seen> {
  matches!(split_label(label), Some((_, Some(_)))).then_some(())?;
  parse(&eval_text(page, READ).await?)
}

/// Takes what the page said into its state. Returns true when a linked tab must close now.
pub(super) fn apply(state: &mut SessionState, seen: Option<Seen>) -> bool {
  let Some(seen) = seen else {
    return false;
  };
  state.page.popup.page_count(seen.failed);
  let close = state.linked && seen.closing;
  if close {
    note("selfClose", "closed", "closing");
  }
  close
}

/// Closes a linked tab that closed itself: its opener is shown again.
pub(super) fn closed(app: &AppHandle, label: &str) {
  let Some((id, Some(tab))) = split_label(label) else {
    return;
  };
  let (app, id, tab) = (app.clone(), id.to_owned(), tab.to_owned());
  tauri::async_runtime::spawn(async move {
    let _ = tab_close(&app, &id, &tab).await;
  });
}

/// Lets a page open pop-ups without a click (`open`: the user allowed the next one), or not:
/// the engine's own gate and the page's pop-up guard.
pub(super) fn gate(page: &Webview, open: bool) {
  engine_gate(page, open);
  let _ = page.eval(if open { GUARD_OPEN } else { GUARD_SHUT });
}

#[cfg(target_os = "macos")]
fn engine_gate(page: &Webview, open: bool) {
  let _ = page.with_webview(move |view| {
    // Tauri supplies a live WKWebView on the main thread.
    let view = unsafe { &*view.inner().cast::<objc2_web_kit::WKWebView>() };
    unsafe {
      view
        .configuration()
        .preferences()
        .setJavaScriptCanOpenWindowsAutomatically(open);
    }
  });
}

#[cfg(target_os = "linux")]
fn engine_gate(page: &Webview, open: bool) {
  use webkit2gtk::{SettingsExt, WebViewExt};
  let _ = page.with_webview(move |view| {
    if let Some(settings) = WebViewExt::settings(&view.inner()) {
      settings.set_javascript_can_open_windows_automatically(open);
    }
  });
}

/// Windows: WebView2 has no such switch; `ENGINE_GATE` is false there.
#[cfg(windows)]
fn engine_gate(_page: &Webview, _open: bool) {}

/// Windows: records whether a click caused each pop-up request (`IsUserInitiated`). wry's own
/// handler, added first, defers its answer to the message loop, so this one runs before it.
#[cfg(windows)]
pub(super) fn watch_clicks(page: &Webview, session: &Arc<Session>) {
  use webview2_com::NewWindowRequestedEventHandler;
  let session = session.clone();
  let _ = page.with_webview(move |view| {
    // Tauri supplies the live WebView2 controller on its owning COM apartment.
    let Ok(view) = (unsafe { view.controller().CoreWebView2() }) else {
      return;
    };
    let handler = NewWindowRequestedEventHandler::create(Box::new(move |sender, args| {
      // wry answers only requests with both; the rest never reach `requested`.
      let (Some(_), Some(args)) = (sender, args) else {
        return Ok(());
      };
      let mut uri = windows::core::PWSTR::null();
      unsafe { args.Uri(&mut uri) }?;
      let url = unsafe { uri.to_string() }.unwrap_or_default();
      unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(uri.0 as _)) };
      let mut user = windows::core::BOOL::default();
      let user = unsafe { args.IsUserInitiated(&mut user) }.is_ok() && user.as_bool();
      if let Ok(mut state) = session.state.lock() {
        clicked(&mut state, &url, user);
      }
      Ok(())
    }));
    let mut token = 0i64;
    // WebView2 retains the handler for the webview's lifetime.
    let _ = unsafe { view.add_NewWindowRequested(&handler, &mut token) };
  });
}

/// macOS and Linux: the engine's gate decides (`ENGINE_GATE`).
#[cfg(not(windows))]
pub(super) fn watch_clicks(_page: &Webview, _session: &Arc<Session>) {}

/// A close the page's script cannot see (its opener, from another site, closed it): the engine
/// tells us. macOS needs nothing: the drain reads `window.closed`.
#[cfg(target_os = "macos")]
fn watch_close(_page: &Webview, _closed: impl Fn() + Send + 'static) {}

/// Linux: wry destroys the widget on the engine's close request; that is the signal left.
#[cfg(target_os = "linux")]
fn watch_close(page: &Webview, closed: impl Fn() + Send + 'static) {
  use gtk::prelude::WidgetExt;
  let _ = page.with_webview(move |view| {
    view.inner().connect_destroy(move |_| closed());
  });
}

#[cfg(windows)]
fn watch_close(page: &Webview, closed: impl Fn() + Send + 'static) {
  use webview2_com::WindowCloseRequestedEventHandler;
  let _ = page.with_webview(move |view| {
    // Tauri supplies the live WebView2 controller on its owning COM apartment.
    let Ok(view) = (unsafe { view.controller().CoreWebView2() }) else {
      return;
    };
    let handler = WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
      closed();
      Ok(())
    }));
    let mut token = 0i64;
    // WebView2 retains the handler for the webview's lifetime.
    let _ = unsafe { view.add_WindowCloseRequested(&handler, &mut token) };
  });
}

/// Pairs the new page with its opener, as each engine asks: without it `window.opener` is lost.
#[cfg(target_os = "macos")]
fn pair(
  builder: WebviewBuilder<tauri::Wry>,
  features: &NewWindowFeatures,
  _private: bool,
) -> WebviewBuilder<tauri::Wry> {
  // The opener's own configuration: its store (a private tab's too) and its scripts.
  builder.with_webview_configuration(features.opener().target_configuration.clone())
}

#[cfg(windows)]
fn pair(
  builder: WebviewBuilder<tauri::Wry>,
  features: &NewWindowFeatures,
  private: bool,
) -> WebviewBuilder<tauri::Wry> {
  // The opener's environment is its data folder; a private tab's pages are InPrivate in it.
  builder
    .with_environment(features.opener().environment.clone())
    .incognito(private)
}

#[cfg(target_os = "linux")]
fn pair(
  builder: WebviewBuilder<tauri::Wry>,
  features: &NewWindowFeatures,
  _private: bool,
) -> WebviewBuilder<tauri::Wry> {
  // A related view shares the opener's web context, so its store too.
  builder.with_related_view(features.opener().webview.clone())
}

/// One log line per step of a tab a page's link or pop-up opens: step names, outcomes, kinds and
/// counts only, never an address (AGENTS.md, Privacy). A drop is a warning.
fn note(step: &'static str, outcome: &'static str, kind: &'static str) {
  if outcome == "ok" {
    tracing::info!(step, outcome, kind, "catch new-tab");
  } else {
    tracing::warn!(step, outcome, kind, "catch new-tab");
  }
}

/// A page asks for a new window. Runs on the main thread, inside the engine's request.
pub(super) fn requested(
  app: &AppHandle,
  opener_label: &str,
  opener: &Arc<Session>,
  url: &Url,
  features: NewWindowFeatures,
) -> NewWindowResponse<tauri::Wry> {
  let refuse = || {
    if let Ok(mut state) = opener.active() {
      state.page.popup.deny();
    }
    NewWindowResponse::Deny
  };
  let eligible = is_http(url) || url.as_str() == "about:blank";
  tracing::info!(
    step = "request",
    eligible,
    dropped_before = dropped(),
    "catch new-tab"
  );
  // Only a tab's page gets a linked tab.
  let Some((id, Some(from))) = split_label(opener_label) else {
    note("request", "refused", "notTab");
    return refuse();
  };
  let Ok(browser) = app.state::<CatchWindows>().browser(id) else {
    note("request", "refused", "noBrowser");
    return refuse();
  };
  // While "allow once" holds the engine's gate open (pages of one window may share it), a
  // request proves no click: it needs its own page's permission.
  let gate_open = browser.active().is_ok_and(|state| state.gate_open());
  let mut shut = false;
  let mut clicked = false;
  let admitted = opener.active().is_ok_and(|mut state| {
    shut = std::mem::take(&mut state.gate_open);
    let gesture = gesture(&mut state, url, ENGINE_GATE, gate_open);
    clicked = gesture;
    state.page.popup.admit(eligible, gesture)
  });
  tracing::info!(
    step = "admit",
    gate_open,
    gesture = clicked,
    admitted,
    "catch new-tab"
  );
  if shut {
    // One pop-up was allowed, not every one until the next read of the page.
    if let Some(page) = app.get_webview(opener_label) {
      gate(&page, false);
    }
  }
  if !admitted {
    note("admit", "refused", "noClick");
    return NewWindowResponse::Deny;
  }
  if let Ok(mut state) = opener.active() {
    state.popup_opened = Some((std::time::Instant::now(), url.clone()));
  }
  let opened = browser.active().and_then(|mut state| {
    state.tabs.open(
      NewTab {
        url: url.to_string(),
        private: false,
        opener: Some(from.to_owned()),
      },
      tokio::time::Instant::now(),
    )
  });
  // No room for one more tab: blocked like any other pop-up.
  let tab = match opened {
    Ok(tab) => tab,
    Err(kind) => {
      let tabs = browser
        .active()
        .map_or(0, |state| state.tabs.iter().count());
      tracing::warn!(
        step = "open",
        outcome = "refused",
        kind,
        tabs,
        cap = crate::catch::tabs::MAX_TABS,
        "catch new-tab"
      );
      return refuse();
    }
  };
  let built = build(app, id, &browser, &tab, &features);
  match &built {
    Ok(_) => note("build", "ok", "none"),
    Err(kind) => note("build", "err", kind),
  }
  let holder = built.as_ref().ok().map(|(window, _)| window.clone());
  match (answer(holder), built) {
    (NewWindowResponse::Create { window }, Ok((_, session))) => {
      // The engine reads the new page out of its holder window after this request returns:
      // only then may it move. `run_on_main_thread` called here, on the main thread, would run
      // at once (the page would leave its holder before the engine reads it, and the engine
      // aborts the app), so the move is sent from another thread: the next main-loop turn.
      note("answer", "ok", "create");
      let (handle, id) = (app.clone(), id.to_owned());
      tauri::async_runtime::spawn(async move {
        let main = handle.clone();
        let _ = main.run_on_main_thread(move || adopt(&handle, &id, &tab, session));
      });
      NewWindowResponse::Create { window }
    }
    (_, built) => {
      note("answer", "refused", "deny");
      if let Ok((window, session)) = built {
        session.stop();
        let _ = window.destroy();
      }
      if let Ok(mut state) = browser.active() {
        // A linked tab shares its opener's store, which outlives it while the opener lives.
        if let Some(store) = state
          .tabs
          .close(&tab)
          .ok()
          .and_then(|closed| closed.drop_store)
        {
          drop(state);
          drop_store(app, id, &store);
        }
      }
      refuse()
    }
  }
}

/// What the engine is told about a pop-up built in `window`. Only a window whose one webview is
/// the new page may be handed over: the engine takes that webview without checking and aborts
/// the app when there is none. Anything else is refused.
pub(super) fn answer<R: tauri::Runtime>(window: Option<WebviewWindow<R>>) -> NewWindowResponse<R> {
  let holds_page = |window: &WebviewWindow<R>| {
    // The page's own window, which a move changes; it must still be the holder.
    let host = window.as_ref().window();
    host.label() == window.label() && host.webviews().len() == 1
  };
  match window {
    Some(window) if holds_page(&window) => NewWindowResponse::Create { window },
    _ => NewWindowResponse::Deny,
  }
}

/// The engine only accepts a new page that is the first webview of its own window, so the page
/// is born in a hidden holder window, at the size it will have in its tab.
fn build(
  app: &AppHandle,
  id: &str,
  browser: &Browser,
  tab: &str,
  features: &NewWindowFeatures,
) -> Result<(WebviewWindow, Arc<Session>), &'static str> {
  let window = app.get_window(id).ok_or("catchWindowClosed")?;
  let label = tab_label(id, tab);
  let (session, private, fit) = {
    let mut state = browser.active()?;
    let private = state.tabs.get(tab).ok_or("catchTabClosed")?.private();
    let session = Arc::new(Session::tab(blank(), private));
    {
      let mut page = session.active()?;
      page.linked = true;
      page.proxied = !state.proxy.fetches_beside();
      page.page.first_run = state.home.first_run;
      page.quiet = std::mem::replace(&mut state.had_page, true);
    }
    (session, private, state.fit)
  };
  let size = window
    .inner_size()
    .map_err(|_| "catchOpenFailed")?
    .to_logical::<f64>(window.scale_factor().map_err(|_| "catchOpenFailed")?);
  let (_, _, width, height) = content_rect(size.width, size.height, fit);
  // macOS: the opener's configuration brings its scripts; adding them again would run them
  // twice in the opener too (both pages share one script list).
  let builder = pair(
    page_with(app, &label, &session, !cfg!(target_os = "macos")),
    features,
    private,
  );
  let holder = WindowBuilder::new(app, &label)
    .title(&app.package_info().name)
    .inner_size(width, height)
    .visible(false)
    .focused(false)
    .skip_taskbar(true)
    .build()
    .map_err(|_| "catchOpenFailed")?;
  let added = holder.add_child(
    builder,
    LogicalPosition::new(0., 0.),
    LogicalSize::new(width, height),
  );
  if let Ok(page) = &added {
    crate::catch::adblock::attach(app, page, &session);
  }
  // A window whose only webview has its label is what the engine's answer takes.
  match added.ok().and(app.get_webview_window(&label)) {
    Some(window) => Ok((window, session)),
    None => {
      let _ = holder.destroy();
      Err("catchOpenFailed")
    }
  }
}

/// Moves the new page from its holder window into its tab. Main thread.
fn adopt(app: &AppHandle, id: &str, tab: &str, session: Arc<Session>) {
  let label = tab_label(id, tab);
  let registry = app.state::<CatchWindows>();
  let holder = app.get_window(&label);
  let page = app.get_webview(&label);
  let browser = registry.browser(id);
  // The step that failed, for the log.
  let mut step = "lookup";
  let moved = (|| {
    let (Some(page), Some(window)) = (&page, app.get_window(id)) else {
      return Err("catchWindowClosed");
    };
    step = "browser";
    let browser = browser.as_ref().map_err(|_| "catchWindowClosed")?;
    step = "mayAdopt";
    // A window closed or a tab closed since the request: the adoption is cancelled. `shut` sets
    // `closed` before the window is destroyed, so a close always lands here first.
    if !browser.active()?.may_adopt(tab) {
      return Err("catchTabClosed");
    }
    // A destroy already processed leaves the window listed with no native window: Tauri's
    // reparent then panics (its reply is never sent). Asked here, on the main thread, nothing
    // can destroy it between this answer and the move.
    step = "visible";
    window.is_visible().map_err(|_| "catchWindowClosed")?;
    step = "reparent";
    page.reparent(&window).map_err(|_| "catchOpenFailed")?;
    gate(page, false);
    watch_clicks(page, &session);
    {
      let (app, label) = (app.clone(), label.clone());
      watch_close(page, move || closed(&app, &label));
    }
    #[cfg(not(target_os = "macos"))]
    {
      watch_load_failure(app, page);
      let (app, label) = (app.clone(), label.clone());
      native::watch_crash(page, move || {
        renderer_crashed(&app, &label);
      });
    }
    step = "register";
    registry.insert(&label, session.clone())?;
    let drain = tauri::async_runtime::spawn(drain_loop(
      app.clone(),
      label.clone(),
      session.clone(),
      page.clone(),
      Arc::new(PinnedManifestFetcher),
    ));
    step = "drain";
    session.active()?.drain = Some(drain.inner().abort_handle());
    step = "tab";
    let mut state = browser.active()?;
    if state.tabs.get(tab).is_none() {
      return Err("catchTabClosed");
    }
    state.pages.insert(tab.to_owned(), session.clone());
    Ok(())
  })();
  if let Some(holder) = holder {
    let _ = holder.destroy();
  }
  match moved {
    Ok(()) => note("adopt", "ok", "none"),
    Err(kind) => tracing::warn!(
      step = "adopt",
      at = step,
      outcome = "err",
      kind,
      "catch new-tab"
    ),
  }
  let Ok(browser) = browser else {
    note("adopt", "err", "noBrowser");
    session.stop();
    return;
  };
  if moved.is_err() {
    registry.remove(&label);
    session.stop();
    let closed = browser.active().ok().and_then(|mut state| {
      if state
        .pages
        .get(tab)
        .is_some_and(|page| Arc::ptr_eq(page, &session))
      {
        state.pages.remove(tab);
      }
      state.tabs.close(tab).ok()
    });
    if let Some(store) = closed.and_then(|closed| closed.drop_store) {
      drop_store(app, id, &store);
    }
    if let Some(page) = &page {
      let _ = page.close();
    }
  }
  // Sizes the page for its tab, puts the shell back in its place and tells it about the tab.
  refresh(app, id, &browser);
  if let (true, Some(page)) = (moved.is_ok(), &page) {
    let _ = page.set_focus();
  }
}
