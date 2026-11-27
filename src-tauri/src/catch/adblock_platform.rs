//! Puts the blocker into each engine's page webview and counts what it blocked.

#[cfg(any(target_os = "macos", windows))]
use std::sync::Weak;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tauri::{AppHandle, Url, Webview};
use tokio::sync::watch;

use super::super::window::Session;

/// The longest a new page waits for the lists (first run of a release: WebKit compiles them once).
/// After that it loads unblocked rather than not at all.
const WAIT: Duration = Duration::from_secs(20);

/// One more request blocked on this page (shown in the side list's "This site" footer).
#[cfg(any(target_os = "macos", windows))]
fn count(session: &Weak<Session>) {
  if let Some(session) = session.upgrade() {
    if let Ok(mut state) = session.state.lock() {
      state.page.ads_blocked = state.page.ads_blocked.saturating_add(1);
    }
  }
}

/// True once pages can get the lists, or once it is clear they cannot (pages then load unblocked).
fn settled() -> &'static watch::Sender<bool> {
  static SETTLED: OnceLock<watch::Sender<bool>> = OnceLock::new();
  SETTLED.get_or_init(|| watch::channel(false).0)
}

/// The lists are ready, or `problem` says why blocking is off for this run (no address or page
/// in it).
fn settle(problem: Option<&'static str>) {
  if let Some(problem) = problem {
    tracing::warn!(problem, "ad blocking is off for this run");
  }
  settled().send_replace(true);
}

/// Starts making the lists (once) and waits until pages can get them, at most `WAIT`.
pub(in crate::catch) async fn ready(app: &AppHandle) {
  let mut settled = settled().subscribe();
  if *settled.borrow() {
    return;
  }
  begin(app);
  if tokio::time::timeout(WAIT, settled.wait_for(|done| *done))
    .await
    .is_err()
  {
    tracing::warn!("ad lists not ready in time: the page loads unblocked");
  }
}

fn begin(app: &AppHandle) {
  #[cfg(windows)]
  {
    let _ = app;
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
      std::thread::spawn(|| {
        super::engine();
        settle(None);
      });
    });
  }
  #[cfg(any(target_os = "macos", target_os = "linux"))]
  {
    let main = app.clone();
    if app.run_on_main_thread(move || imp::begin(&main)).is_err() {
      settle(Some("mainThread"));
    }
  }
  #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
  {
    let _ = app;
    settle(Some("unsupported"));
  }
}

/// Loads `url` in a page made a moment ago, once its lists are on: at once when they are ready,
/// else when they are (at most `WAIT`), unless the page went elsewhere or closed meanwhile.
pub(in crate::catch) fn navigate(
  app: &AppHandle,
  page: &Webview,
  session: &Arc<Session>,
  url: Url,
) -> Result<(), &'static str> {
  // The first page follows its site's switch from its first load (queued before the load).
  #[cfg(any(target_os = "macos", target_os = "linux"))]
  {
    let on = super::on(&url);
    let _ = page.with_webview(move |view| imp::apply(&view, on));
  }
  if *settled().borrow() {
    return page.navigate(url).map_err(|_| "catchOpenFailed");
  }
  let generation = session
    .state
    .lock()
    .map(|state| state.page.generation)
    .map_err(|_| "catchOpenFailed")?;
  let (app, page, session) = (app.clone(), page.clone(), session.clone());
  tauri::async_runtime::spawn(async move {
    ready(&app).await;
    let same = session
      .state
      .lock()
      .is_ok_and(|state| !state.closed && state.page.generation == generation);
    if same {
      let _ = page.navigate(url);
    }
  });
  Ok(())
}

/// A page's top frame commits `url`: its lists follow the site's switch (again, after
/// `deciding` and Linux's load start; a redirect may have changed the site).
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(in crate::catch) fn navigating(app: &AppHandle, label: &str, url: &Url) {
  use tauri::Manager;
  let on = super::on(url);
  if let Some(page) = app.get_webview(label) {
    let _ = page.with_webview(move |view| imp::apply(&view, on));
  }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(in crate::catch) fn navigating(_app: &AppHandle, _label: &str, _url: &Url) {}

/// The page's navigation handler allowed a load of `url`: when it is the top frame's, its lists
/// follow the site's switch before the new page asks for anything. Only macOS says which frame
/// a decision is for (wry patch); Linux follows its top frame's `load-changed` (`imp::attach`),
/// and Windows asks the switch on every request.
#[cfg(target_os = "macos")]
pub(in crate::catch) fn deciding(app: &AppHandle, label: &str, url: &Url) {
  if wry::navigation_main_frame() == Some(true) {
    navigating(app, label, url);
  }
}

#[cfg(not(target_os = "macos"))]
pub(in crate::catch) fn deciding(_app: &AppHandle, _label: &str, _url: &Url) {}

/// The switch changed for the page's site: its lists follow before the caller reloads it.
/// (Windows asks the switch on every request.)
pub(in crate::catch) async fn switched(page: &Webview, on: bool) {
  #[cfg(any(target_os = "macos", target_os = "linux"))]
  {
    let (done, applied) = tokio::sync::oneshot::channel();
    let sent = page.with_webview(move |view| {
      imp::apply(&view, on);
      let _ = done.send(());
    });
    if sent.is_ok() {
      let _ = applied.await;
    }
  }
  #[cfg(not(any(target_os = "macos", target_os = "linux")))]
  let _ = (page, on);
}

/// Before a page webview is made: whatever the engine needs in place first.
pub(in crate::catch) fn prepare(app: &AppHandle) {
  super::load(app);
}

/// Puts the blocker on a page webview made a moment ago (before its first navigation).
#[cfg(target_os = "macos")]
pub(in crate::catch) fn attach(app: &AppHandle, page: &Webview, session: &Arc<Session>) {
  begin(app);
  let session = Arc::downgrade(session);
  let _ = page.with_webview(move |view| imp::attach(&view, session));
}

#[cfg(target_os = "linux")]
pub(in crate::catch) fn attach(app: &AppHandle, page: &Webview, _session: &Arc<Session>) {
  begin(app);
  let _ = page.with_webview(move |view| imp::attach(&view));
}

/// Answers each request of a page webview made a moment ago with the Rust matcher.
#[cfg(windows)]
pub(in crate::catch) fn attach(app: &AppHandle, page: &Webview, session: &Arc<Session>) {
  begin(app);
  let session = Arc::downgrade(session);
  let _ = page.with_webview(move |view| imp::attach(&view, session));
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub(in crate::catch) fn attach(_app: &AppHandle, _page: &Webview, _session: &Arc<Session>) {}

#[cfg(target_os = "macos")]
pub(super) mod imp {
  use super::*;
  use block2::RcBlock;
  use objc2::rc::{Retained, Weak as ObjcWeak};
  use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
  use objc2::{sel, MainThreadMarker};
  use objc2_foundation::{NSArray, NSError, NSString};
  use objc2_web_kit::{
    WKContentRuleList, WKContentRuleListStore, WKUserContentController, WKWebView,
  };
  use std::cell::RefCell;
  use std::collections::HashMap;
  use tauri::webview::PlatformWebview;

  #[derive(Default)]
  struct Lists {
    started: bool,
    ready: Option<Vec<Retained<WKContentRuleList>>>,
    /// Pages made before the lists were ready: they get them when they are.
    waiting: Vec<ObjcWeak<WKWebView>>,
    /// Each page's session, by its WKWebView's address, for the count.
    pages: HashMap<usize, Weak<Session>>,
    /// Whether the lists are on, per content controller: a pop-up shares its opener's, so the
    /// switch belongs to the controller, not to a page.
    applied: Vec<(ObjcWeak<WKUserContentController>, bool)>,
  }

  impl Lists {
    fn applied(&self, controller: &WKUserContentController) -> Option<bool> {
      self
        .applied
        .iter()
        .find(|(known, _)| {
          known
            .load()
            .is_some_and(|known| std::ptr::eq(&*known, controller))
        })
        .map(|(_, on)| *on)
    }

    /// Records `on`; returns the earlier value.
    fn record(&mut self, controller: &WKUserContentController, on: bool) -> Option<bool> {
      let was = self.applied(controller);
      self.applied.retain(|(known, _)| {
        known
          .load()
          .is_some_and(|known| !std::ptr::eq(&*known, controller))
      });
      self.applied.push((ObjcWeak::new(controller), on));
      was
    }
  }

  thread_local! {
    // WebKit calls back on the main thread only.
    static LISTS: RefCell<Lists> = RefCell::new(Lists::default());
  }

  pub(super) fn attach(view: &PlatformWebview, session: Weak<Session>) {
    // Tauri supplies a live WKWebView on the main thread.
    let view = unsafe { &*view.inner().cast::<WKWebView>() };
    count_blocked(view);
    let controller = unsafe { view.configuration().userContentController() };
    let put = LISTS.with(|lists| {
      let mut lists = lists.borrow_mut();
      lists.pages.retain(|_, session| session.strong_count() > 0);
      lists
        .pages
        .insert(view as *const WKWebView as usize, session);
      if lists.ready.is_none() {
        lists.waiting.retain(|page| page.load().is_some());
        lists.waiting.push(ObjcWeak::new(view));
      }
      // A pop-up with its opener's controller keeps what the opener has.
      let fresh = lists.applied(&controller).is_none();
      if fresh {
        lists.record(&controller, true);
      }
      fresh && lists.ready.is_some()
    });
    if put {
      put_lists(&controller, true);
    }
  }

  /// Makes the lists once per run (main thread).
  pub(super) fn begin(app: &AppHandle) {
    let start = LISTS.with(|lists| !std::mem::replace(&mut lists.borrow_mut().started, true));
    if start {
      self::start(app.clone());
    }
  }

  /// The page's lists on or off (kept for when they are ready). Takes effect for the loads that
  /// follow.
  pub(super) fn apply(view: &PlatformWebview, on: bool) {
    // Tauri supplies a live WKWebView on the main thread.
    let view = unsafe { &*view.inner().cast::<WKWebView>() };
    let controller = unsafe { view.configuration().userContentController() };
    let put = LISTS.with(|lists| {
      let mut lists = lists.borrow_mut();
      lists.record(&controller, on) != Some(on) && lists.ready.is_some()
    });
    if put {
      put_lists(&controller, on);
    }
  }

  fn put_lists(controller: &WKUserContentController, on: bool) {
    LISTS.with(|lists| {
      let lists = lists.borrow();
      let Some(ready) = lists.ready.as_ref() else {
        return;
      };
      for list in ready {
        unsafe {
          controller.removeContentRuleList(list);
          if on {
            controller.addContentRuleList(list);
          }
        }
      }
    });
  }

  /// Finds the compiled lists WebKit kept from an earlier run, or compiles them once.
  fn start(app: AppHandle) {
    let Some(store) =
      MainThreadMarker::new().and_then(|mtm| unsafe { WKContentRuleListStore::defaultStore(mtm) })
    else {
      return settle(Some("noStore"));
    };
    let found = RcBlock::new(move |ids: *mut NSArray<NSString>| {
      let ids: Vec<String> = unsafe { ids.as_ref() }
        .map(|ids| ids.iter().map(|id| id.to_string()).collect())
        .unwrap_or_default();
      forget_old(&ids);
      match super::super::kept_parts(&ids) {
        Some(total) => look_up(total, app.clone()),
        None => convert(app.clone()),
      }
    });
    unsafe { store.getAvailableContentRuleListIdentifiers(Some(&found)) };
  }

  /// Old lists (an earlier release's) are removed from WebKit's store.
  fn forget_old(ids: &[String]) {
    let Some(store) =
      MainThreadMarker::new().and_then(|mtm| unsafe { WKContentRuleListStore::defaultStore(mtm) })
    else {
      return;
    };
    for id in super::super::old_ids(ids) {
      let done = RcBlock::new(|_: *mut NSError| {});
      unsafe {
        store.removeContentRuleListForIdentifier_completionHandler(
          Some(&NSString::from_str(&id)),
          Some(&done),
        )
      };
    }
  }

  /// Kept lists from an earlier run; if WebKit no longer reads one (a newer WebKit), the list is
  /// compiled again.
  fn look_up(total: usize, app: AppHandle) {
    let Some(store) =
      MainThreadMarker::new().and_then(|mtm| unsafe { WKContentRuleListStore::defaultStore(mtm) })
    else {
      return settle(Some("noStore"));
    };
    for part in 0..total {
      let id = NSString::from_str(&super::super::list_id(part, total));
      let app = app.clone();
      let done = RcBlock::new(move |list: *mut WKContentRuleList, _: *mut NSError| {
        collect(unsafe { Retained::retain(list) }, total, Some(&app));
      });
      unsafe { store.lookUpContentRuleListForIdentifier_completionHandler(Some(&id), Some(&done)) };
    }
  }

  /// Turns the list into WebKit rules off the main thread (a few seconds, once per release), then compiles.
  fn convert(app: AppHandle) {
    std::thread::spawn(move || {
      let parts = match super::super::webkit_rules() {
        Ok(parts) => parts,
        Err(problem) => return settle(Some(problem)),
      };
      if parts.is_empty() {
        return settle(Some("emptyList"));
      }
      if app.run_on_main_thread(move || compile(parts)).is_err() {
        settle(Some("mainThread"));
      }
    });
  }

  fn compile(parts: Vec<String>) {
    let Some(store) =
      MainThreadMarker::new().and_then(|mtm| unsafe { WKContentRuleListStore::defaultStore(mtm) })
    else {
      return settle(Some("noStore"));
    };
    let total = parts.len();
    for (part, rules) in parts.into_iter().enumerate() {
      let id = NSString::from_str(&super::super::list_id(part, total));
      let rules = NSString::from_str(&rules);
      let done = RcBlock::new(move |list: *mut WKContentRuleList, _: *mut NSError| {
        collect(unsafe { Retained::retain(list) }, total, None);
      });
      unsafe {
        store.compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
          Some(&id),
          Some(&rules),
          Some(&done),
        )
      };
    }
  }

  /// One part is ready (or failed: `None`). With every part in, waiting pages get them. Parts
  /// looked up from the store (`kept`: the app to compile with) that failed are compiled again.
  fn collect(list: Option<Retained<WKContentRuleList>>, total: usize, kept: Option<&AppHandle>) {
    thread_local! {
      static PARTS: RefCell<(usize, Vec<Retained<WKContentRuleList>>)> = const { RefCell::new((0, Vec::new())) };
    }
    let complete = PARTS.with(|parts| {
      let mut parts = parts.borrow_mut();
      parts.0 += 1;
      parts.1.extend(list);
      (parts.0 == total).then(|| {
        parts.0 = 0;
        std::mem::take(&mut parts.1)
      })
    });
    let Some(ready) = complete else {
      return;
    };
    let missing = total - ready.len();
    if missing > 0 {
      if let Some(app) = kept {
        tracing::warn!(
          missing,
          total,
          "kept ad list unreadable: compiling it again"
        );
        return convert(app.clone());
      }
      tracing::warn!(missing, total, "WebKit refused part of the ad list");
    }
    let empty = ready.is_empty();
    let waiting = LISTS.with(|lists| {
      let mut lists = lists.borrow_mut();
      lists.ready = Some(ready);
      std::mem::take(&mut lists.waiting)
    });
    for page in waiting {
      if let Some(view) = page.load() {
        let controller = unsafe { view.configuration().userContentController() };
        let on = LISTS.with(|lists| lists.borrow().applied(&controller).unwrap_or(true));
        put_lists(&controller, on);
      }
    }
    settle(empty.then_some("compile"));
  }

  /// WebKit tells a navigation delegate each time a content rule list acted, through a private
  /// method (`WKNavigationDelegatePrivate`, macOS 11+) it calls only when the delegate answers
  /// to it, which it asks when the delegate is set. The page's own delegate (wry's, registered by
  /// objc2 under a generated name, so never looked up by name) gets the method, then is set again.
  fn count_blocked(view: &WKWebView) {
    // SAFETY: `navigationDelegate` is a plain getter on a live WKWebView (main thread).
    let delegate: Option<Retained<AnyObject>> =
      unsafe { objc2::msg_send![view, navigationDelegate] };
    let Some(delegate) = delegate else {
      return;
    };
    add_counter(delegate.class());
    // SAFETY: sets back the delegate the view already had (wry keeps it alive).
    unsafe {
      let _: () = objc2::msg_send![view, setNavigationDelegate: &*delegate];
    }
  }

  /// Gives `class` the rule-list callback; false when it already answers to it.
  pub(in crate::catch) fn add_counter(class: &AnyClass) -> bool {
    let selector = sel!(_webView:contentRuleListWithIdentifier:performedAction:forURL:);
    if class.responds_to(selector) {
      return false;
    }
    type Acted = unsafe extern "C-unwind" fn(
      *mut AnyObject,
      Sel,
      *mut AnyObject,
      *mut AnyObject,
      *mut AnyObject,
      *mut AnyObject,
    );
    let imp: Acted = acted;
    // SAFETY: the types string matches `acted` (void; self, _cmd, four objects).
    unsafe {
      objc2::ffi::class_addMethod(
        class as *const AnyClass as *mut AnyClass,
        selector,
        std::mem::transmute::<Acted, Imp>(imp),
        c"v@:@@@@".as_ptr(),
      )
      .as_bool()
    }
  }

  unsafe extern "C-unwind" fn acted(
    _this: *mut AnyObject,
    _cmd: Sel,
    view: *mut AnyObject,
    _identifier: *mut AnyObject,
    action: *mut AnyObject,
    _url: *mut AnyObject,
  ) {
    let Some(action) = (unsafe { action.as_ref() }) else {
      return;
    };
    // `_WKContentRuleListAction.blockedLoad`; asked only when the object answers to it.
    let blocked = action.class().responds_to(sel!(blockedLoad))
      && unsafe { objc2::msg_send![action, blockedLoad] };
    if !blocked {
      return;
    }
    let session = LISTS.with(|lists| lists.borrow().pages.get(&(view as usize)).cloned());
    if let Some(session) = session {
      count(&session);
    }
  }
}

#[cfg(windows)]
mod imp {
  use super::*;
  use tauri::webview::PlatformWebview;
  use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2_22, COREWEBVIEW2_WEB_RESOURCE_CONTEXT,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST,
    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
  };
  use webview2_com::{take_pwstr, WebResourceRequestedEventHandler};
  use windows::core::{Interface, HSTRING, PWSTR};

  use super::super::Kind;

  fn kind(context: COREWEBVIEW2_WEB_RESOURCE_CONTEXT) -> Kind {
    match context {
      COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT => Kind::Document,
      COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT => Kind::Script,
      COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE => Kind::Image,
      COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET => Kind::Style,
      COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT => Kind::Font,
      COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA => Kind::Media,
      COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH
      | COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST => Kind::Fetch,
      _ => Kind::Other,
    }
  }

  pub(super) fn attach(view: &PlatformWebview, session: Weak<Session>) {
    let Ok(webview) = (unsafe { view.controller().CoreWebView2() }) else {
      return;
    };
    let filter = HSTRING::from("*");
    // Out-of-process frames and service/shared workers too where the runtime has it (WebView2
    // 1.0.2365.46+); older runtimes get the plain filter, which does not see worker requests.
    let added = match webview.cast::<ICoreWebView2_22>() {
      Ok(webview) => unsafe {
        webview.AddWebResourceRequestedFilterWithRequestSourceKinds(
          &filter,
          COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
          COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
        )
      },
      Err(_) => unsafe {
        webview.AddWebResourceRequestedFilter(&filter, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)
      },
    };
    if added.is_err() {
      return;
    }
    let environment = view.environment();
    let requested = WebResourceRequestedEventHandler::create(Box::new(
      move |sender: Option<ICoreWebView2>, args| {
        let (Some(sender), Some(args)) = (sender, args) else {
          return Ok(());
        };
        let request = unsafe { args.Request()? };
        let mut uri = PWSTR::null();
        unsafe { request.Uri(&mut uri)? };
        let uri = take_pwstr(uri);
        let mut source = PWSTR::null();
        unsafe { sender.Source(&mut source)? };
        let page = take_pwstr(source);
        let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
        unsafe { args.ResourceContext(&mut context)? };
        // A document (any frame's) is never blocked, so a stale `Source` while the top frame
        // navigates never judges the new page: its own requests come after it commits.
        let kind = kind(context);
        let on = Url::parse(&page).map_or(true, |page| super::super::on(&page));
        if !on || !super::super::blocks(&uri, &page, kind) {
          return Ok(());
        }
        let response = unsafe {
          environment.CreateWebResourceResponse(
            None,
            403,
            &HSTRING::from("Blocked"),
            &HSTRING::new(),
          )?
        };
        unsafe { args.SetResponse(&response)? };
        count(&session);
        Ok(())
      },
    ));
    let mut token = Default::default();
    let _ = unsafe { webview.add_WebResourceRequested(&requested, &mut token) };
  }
}

#[cfg(target_os = "linux")]
mod imp {
  use super::*;
  use std::cell::RefCell;
  use std::ffi::{CStr, CString};
  use tauri::webview::PlatformWebview;
  use tauri::Manager;
  use webkit2gtk::glib::translate::ToGlibPtr;
  use webkit2gtk::glib::{self, WeakRef};
  use webkit2gtk::{ffi, gio, LoadEvent, UserContentManager, WebView, WebViewExt};

  #[derive(Default)]
  struct Lists {
    started: bool,
    store: Option<usize>,
    ready: Option<Vec<usize>>,
    waiting: Vec<WeakRef<WebView>>,
    parts: (usize, Vec<usize>),
    /// To compile the list again when kept parts no longer load.
    app: Option<AppHandle>,
    /// Whether the lists are on, per content manager: a pop-up (a related view) shares its
    /// opener's, so the switch belongs to the manager, not to a page.
    applied: Vec<(WeakRef<UserContentManager>, bool)>,
  }

  impl Lists {
    fn applied(&self, manager: &UserContentManager) -> Option<bool> {
      self
        .applied
        .iter()
        .find(|(known, _)| known.upgrade().is_some_and(|known| known == *manager))
        .map(|(_, on)| *on)
    }

    /// Records `on`; returns the earlier value.
    fn record(&mut self, manager: &UserContentManager, on: bool) -> Option<bool> {
      let was = self.applied(manager);
      self
        .applied
        .retain(|(known, _)| known.upgrade().is_some_and(|known| known != *manager));
      let weak = WeakRef::new();
      weak.set(Some(manager));
      self.applied.push((weak, on));
      was
    }
  }

  thread_local! {
    // GTK calls back on its main thread only.
    static LISTS: RefCell<Lists> = RefCell::new(Lists::default());
  }

  /// WebKitGTK does not say what its filters blocked, so the count stays 0 here (contract).
  pub(super) fn attach(view: &PlatformWebview) {
    let view = view.inner();
    let Some(manager) = view.user_content_manager() else {
      return;
    };
    // `load-changed` is the top frame's only: its lists follow the site's switch as soon as a
    // load starts (or is redirected), before the new page asks for anything.
    view.connect_load_changed(|view, event| {
      if !matches!(event, LoadEvent::Started | LoadEvent::Redirected) {
        return;
      }
      let Some(url) = view
        .uri()
        .and_then(|uri| Url::parse(&uri).ok())
        .filter(crate::catch::state::is_http)
      else {
        return;
      };
      if let Some(manager) = view.user_content_manager() {
        switch(&manager, super::super::on(&url));
      }
    });
    let put = LISTS.with(|lists| {
      let mut lists = lists.borrow_mut();
      if lists.ready.is_none() {
        lists.waiting.retain(|page| page.upgrade().is_some());
        let weak = WeakRef::new();
        weak.set(Some(&view));
        lists.waiting.push(weak);
      }
      // A pop-up with its opener's manager keeps what the opener has.
      let fresh = lists.applied(&manager).is_none();
      if fresh {
        lists.record(&manager, true);
      }
      fresh && lists.ready.is_some()
    });
    if put {
      put_lists(&manager, true);
    }
  }

  /// The page's lists on or off (kept for when they are ready). Takes effect for the loads that
  /// follow.
  pub(super) fn apply(view: &PlatformWebview, on: bool) {
    if let Some(manager) = view.inner().user_content_manager() {
      switch(&manager, on);
    }
  }

  fn switch(manager: &UserContentManager, on: bool) {
    let put = LISTS.with(|lists| {
      let mut lists = lists.borrow_mut();
      lists.record(manager, on) != Some(on) && lists.ready.is_some()
    });
    if put {
      put_lists(manager, on);
    }
  }

  fn put_lists(manager: &UserContentManager, on: bool) {
    LISTS.with(|lists| {
      let lists = lists.borrow();
      let Some(ready) = lists.ready.as_ref() else {
        return;
      };
      let manager: *mut ffi::WebKitUserContentManager = manager.to_glib_none().0;
      for filter in ready {
        let filter = *filter as *mut ffi::WebKitUserContentFilter;
        unsafe {
          ffi::webkit_user_content_manager_remove_filter(manager, filter);
          if on {
            ffi::webkit_user_content_manager_add_filter(manager, filter);
          }
        }
      }
    });
  }

  fn store() -> Option<*mut ffi::WebKitUserContentFilterStore> {
    LISTS.with(|lists| lists.borrow().store.map(|store| store as *mut _))
  }

  /// Makes the lists once per run (main thread).
  pub(super) fn begin(app: &AppHandle) {
    if LISTS.with(|lists| std::mem::replace(&mut lists.borrow_mut().started, true)) {
      return;
    }
    let Some(path) =
      app.path().app_cache_dir().ok().and_then(|dir| {
        CString::new(dir.join("content-filters").to_string_lossy().as_bytes()).ok()
      })
    else {
      return settle(Some("noStore"));
    };
    let store = unsafe { ffi::webkit_user_content_filter_store_new(path.as_ptr()) };
    LISTS.with(|lists| {
      let mut lists = lists.borrow_mut();
      lists.store = Some(store as usize);
      lists.app = Some(app.clone());
    });
    let app = Box::into_raw(Box::new(app.clone()));
    unsafe {
      ffi::webkit_user_content_filter_store_fetch_identifiers(
        store,
        std::ptr::null_mut(),
        Some(fetched),
        app.cast(),
      );
    }
  }

  unsafe extern "C" fn fetched(
    _: *mut glib::gobject_ffi::GObject,
    result: *mut gio::ffi::GAsyncResult,
    app: glib::ffi::gpointer,
  ) {
    let app = unsafe { Box::from_raw(app.cast::<AppHandle>()) };
    let Some(store) = store() else {
      return;
    };
    let mut ids = Vec::new();
    let list =
      unsafe { ffi::webkit_user_content_filter_store_fetch_identifiers_finish(store, result) };
    if !list.is_null() {
      let mut at = list;
      while !unsafe { *at }.is_null() {
        ids.push(
          unsafe { CStr::from_ptr(*at) }
            .to_string_lossy()
            .into_owned(),
        );
        at = unsafe { at.add(1) };
      }
      unsafe { glib::ffi::g_strfreev(list) };
    }
    for id in super::super::old_ids(&ids) {
      if let Ok(id) = CString::new(id) {
        unsafe {
          ffi::webkit_user_content_filter_store_remove(
            store,
            id.as_ptr(),
            std::ptr::null_mut(),
            None,
            std::ptr::null_mut(),
          )
        };
      }
    }
    match super::super::kept_parts(&ids) {
      Some(total) => {
        for part in 0..total {
          let Ok(id) = CString::new(super::super::list_id(part, total)) else {
            continue;
          };
          unsafe {
            ffi::webkit_user_content_filter_store_load(
              store,
              id.as_ptr(),
              std::ptr::null_mut(),
              Some(loaded),
              total as glib::ffi::gpointer,
            )
          };
        }
      }
      None => convert(*app),
    }
  }

  /// Turns the list into WebKit rules off the main thread (a few seconds, once per release), then
  /// saves (compiles) them.
  fn convert(app: AppHandle) {
    std::thread::spawn(move || {
      let parts = match super::super::webkit_rules() {
        Ok(parts) => parts,
        Err(problem) => return settle(Some(problem)),
      };
      if parts.is_empty() {
        return settle(Some("emptyList"));
      }
      if app.run_on_main_thread(move || save(parts)).is_err() {
        settle(Some("mainThread"));
      }
    });
  }

  fn save(parts: Vec<String>) {
    let Some(store) = store() else {
      return;
    };
    let total = parts.len();
    for (part, rules) in parts.into_iter().enumerate() {
      let Ok(id) = CString::new(super::super::list_id(part, total)) else {
        continue;
      };
      let bytes = glib::Bytes::from_owned(rules.into_bytes());
      unsafe {
        ffi::webkit_user_content_filter_store_save(
          store,
          id.as_ptr(),
          bytes.to_glib_none().0,
          std::ptr::null_mut(),
          Some(saved),
          total as glib::ffi::gpointer,
        )
      };
    }
  }

  unsafe extern "C" fn loaded(
    _: *mut glib::gobject_ffi::GObject,
    result: *mut gio::ffi::GAsyncResult,
    total: glib::ffi::gpointer,
  ) {
    let Some(store) = store() else {
      return;
    };
    let filter = unsafe {
      ffi::webkit_user_content_filter_store_load_finish(store, result, std::ptr::null_mut())
    };
    collect(filter, total as usize, true);
  }

  unsafe extern "C" fn saved(
    _: *mut glib::gobject_ffi::GObject,
    result: *mut gio::ffi::GAsyncResult,
    total: glib::ffi::gpointer,
  ) {
    let Some(store) = store() else {
      return;
    };
    let filter = unsafe {
      ffi::webkit_user_content_filter_store_save_finish(store, result, std::ptr::null_mut())
    };
    collect(filter, total as usize, false);
  }

  /// One part is ready (or failed: null). With every part in, waiting pages get them. The
  /// filters are kept for the whole run. Parts loaded from the store (`kept`) that failed (a newer
  /// WebKit) are compiled again.
  fn collect(filter: *mut ffi::WebKitUserContentFilter, total: usize, kept: bool) {
    let done = LISTS.with(|lists| {
      let mut lists = lists.borrow_mut();
      lists.parts.0 += 1;
      if !filter.is_null() {
        lists.parts.1.push(filter as usize);
      }
      if lists.parts.0 < total {
        return None;
      }
      lists.parts.0 = 0;
      let parts = std::mem::take(&mut lists.parts.1);
      if kept && parts.len() < total {
        if let Some(app) = lists.app.clone() {
          // The parts that did load are compiled again with the rest: release them.
          for filter in parts {
            unsafe { ffi::webkit_user_content_filter_unref(filter as *mut _) };
          }
          return Some(Err(app));
        }
      }
      lists.ready = Some(parts);
      Some(Ok(std::mem::take(&mut lists.waiting)))
    });
    let waiting = match done {
      None => return,
      Some(Err(app)) => {
        tracing::warn!(total, "kept ad list unreadable: compiling it again");
        return convert(app);
      }
      Some(Ok(waiting)) => waiting,
    };
    let empty = LISTS.with(|lists| lists.borrow().ready.as_ref().is_none_or(Vec::is_empty));
    for page in waiting {
      if let Some(manager) = page.upgrade().and_then(|view| view.user_content_manager()) {
        let on = LISTS.with(|lists| lists.borrow().applied(&manager).unwrap_or(true));
        put_lists(&manager, on);
      }
    }
    settle(empty.then_some("compile"));
  }
}
