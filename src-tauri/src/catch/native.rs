//! Browser engine operations that Tauri's common webview API does not expose.

use super::state::BrowserStatus;
use std::time::Duration;
use tauri::{
  webview::{cookie::Cookie, PlatformWebview},
  Url, Webview,
};
use tokio::sync::oneshot;

const DEEP_SCRIPT: &str =
  "globalThis.__nezzelCatchDeep=true; dispatchEvent(new Event('__nezzelCatchDeep'));";
type NativeResult<T> = Result<T, &'static str>;
type Sender<T> = oneshot::Sender<NativeResult<T>>;

async fn call<T: Send + 'static>(
  page: &Webview,
  operation: impl FnOnce(PlatformWebview, Sender<T>) + Send + 'static,
) -> NativeResult<T> {
  call_within(page, Duration::from_secs(3), operation).await
}

async fn call_within<T: Send + 'static>(
  page: &Webview,
  limit: Duration,
  operation: impl FnOnce(PlatformWebview, Sender<T>) + Send + 'static,
) -> NativeResult<T> {
  let (send, receive) = oneshot::channel();
  page
    .with_webview(move |view| operation(view, send))
    .map_err(|_| "catchWindowClosed")?;
  tokio::time::timeout(limit, receive)
    .await
    .map_err(|_| "catchBrowserTimeout")?
    .map_err(|_| "catchBrowserUnavailable")?
}

/// The page address, or `None` before the first load. Tauri's `url()` aborts the app while the
/// engine has no address yet (a fresh window, or right after its web content process ended).
pub(super) async fn url(page: &Webview) -> Option<Url> {
  call(page, |view, send| {
    let _ = send.send(platform::url(&view).ok_or("catchNoUrl"));
  })
  .await
  .ok()
}

/// Status and address in one main-thread round trip, for the drain loop.
pub(super) async fn inspect(page: &Webview) -> NativeResult<(BrowserStatus, Option<Url>)> {
  call(page, |view, send| {
    let status = platform::status(&view);
    let _ = send.send(status.map(|status| (status, platform::url(&view))));
  })
  .await
}

/// The profile's cookies (Windows and Linux: those the engine sends to `url`). Read with the
/// engine's own callback: Tauri's `cookies()` blocks the main thread in a nested run loop and
/// aborts when the window closes mid-read.
pub(super) async fn cookies(page: &Webview, url: Url) -> NativeResult<Vec<Cookie<'static>>> {
  call(page, move |view, send| platform::cookies(view, url, send)).await
}

/// macOS 14+: every cookie of the persistent catch profile, read from its data store with no
/// page (the caller filters by site). Read only: nothing is ever written to this store here.
/// Elsewhere the profile is read through a tab's page (`cookies`).
#[cfg(target_os = "macos")]
pub(super) async fn profile_cookies(app: &tauri::AppHandle) -> NativeResult<Vec<Cookie<'static>>> {
  let (send, receive) = oneshot::channel();
  app
    .run_on_main_thread(move || platform::profile_cookies(send))
    .map_err(|_| "catchBrowserUnavailable")?;
  tokio::time::timeout(Duration::from_secs(3), receive)
    .await
    .map_err(|_| "catchBrowserTimeout")?
    .map_err(|_| "catchBrowserUnavailable")?
}

#[cfg(not(target_os = "macos"))]
pub(super) async fn profile_cookies(_app: &tauri::AppHandle) -> NativeResult<Vec<Cookie<'static>>> {
  Ok(Vec::new())
}

/// Windows and Linux: run `crashed` on the main thread when the page's renderer process dies.
/// macOS gets the same through the app-wide web content process hook.
#[cfg(not(target_os = "macos"))]
pub(super) fn watch_crash(page: &Webview, crashed: impl Fn() + Send + 'static) {
  let _ = page.with_webview(move |view| platform::watch_crash(view, crashed));
}

#[cfg(not(target_os = "macos"))]
pub(super) fn watch_load_failure(page: &Webview, failed: impl Fn(i64, &str) + Send + 'static) {
  let _ = page.with_webview(move |view| platform::watch_load_failure(view, failed));
}

pub(super) async fn history(page: &Webview, back: bool) -> NativeResult<()> {
  call(page, move |view, send| {
    let _ = send.send(platform::history(view, back));
  })
  .await
}

pub(super) async fn enable_deep(page: &Webview) -> NativeResult<()> {
  call(page, platform::enable_deep).await
}

pub(super) async fn forget(page: &Webview, url: Url) -> NativeResult<()> {
  call(page, move |view, send| platform::forget(view, url, send)).await
}

/// Writes `cookies` into the page's own store with the engine's own cookie manager and answers
/// how many were written, once the store has them. The domain is passed as the jar gave it
/// (`domain_original`): a leading dot is a domain cookie, none is host-only. Tauri's setter drops
/// the dot and, on Linux, does not wait for the write. A whole sign-in is one callback per
/// cookie: it gets a longer limit than a single engine call.
pub(super) async fn set_cookies(
  page: &Webview,
  cookies: Vec<Cookie<'static>>,
) -> NativeResult<usize> {
  call_within(page, Duration::from_secs(15), move |view, send| {
    platform::set_cookies(view, cookies, send)
  })
  .await
}

/// `set_cookies`, only while `allowed` holds for the page's address read in the same main-thread
/// turn: a page that moved on by itself since the caller's check gets nothing written.
pub(super) async fn set_cookies_if(
  page: &Webview,
  cookies: Vec<Cookie<'static>>,
  allowed: impl FnOnce(Option<&Url>) -> bool + Send + 'static,
) -> NativeResult<usize> {
  call_within(page, Duration::from_secs(15), move |view, send| {
    if allowed(platform::url(&view).as_ref()) {
      platform::set_cookies(view, cookies, send);
    } else {
      let _ = send.send(Err("catchLeftSite"));
    }
  })
  .await
}

/// Reloads only if the address read in that same main-thread turn is allowed.
/// Returns the reloaded address, or `None` if the check rejects it.
pub(super) async fn reload_if(
  page: &Webview,
  allowed: impl FnOnce(&Url) -> bool + Send + 'static,
) -> NativeResult<Option<Url>> {
  call(page, move |view, send| {
    let result = reload_if_at_url(platform::url(&view), allowed, || platform::reload(&view));
    let _ = send.send(result);
  })
  .await
  .map_err(|_| "catchNavigationFailed")
}

pub(super) fn reload_if_at_url(
  current: Option<Url>,
  allowed: impl FnOnce(&Url) -> bool,
  reload: impl FnOnce() -> NativeResult<()>,
) -> NativeResult<Option<Url>> {
  let Some(url) = current.filter(allowed) else {
    return Ok(None);
  };
  reload()?;
  Ok(Some(url))
}

/// Sets the identity the page presents (`None`: the engine's own), for the next load.
pub(super) async fn set_agent(page: &Webview, agent: Option<String>) -> NativeResult<()> {
  #[cfg(windows)]
  let label = page.label().to_owned();
  call(page, move |view, send| {
    #[cfg(windows)]
    let result = platform::set_agent(&view, &label, agent);
    #[cfg(not(windows))]
    let result = platform::set_agent(&view, agent);
    let _ = send.send(result);
  })
  .await
}

/// Drops what `set_agent` kept for a page that is gone (Windows: the engine's own identity).
pub(super) fn forget_agent(label: &str) {
  #[cfg(windows)]
  platform::forget_agent(label);
  #[cfg(not(windows))]
  let _ = label;
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn same_website(name: &str, host: &str) -> bool {
  let name = name.trim_end_matches('.');
  let host = host.trim_end_matches('.');
  name.eq_ignore_ascii_case(host)
    || (host.parse::<std::net::IpAddr>().is_err()
      && psl::domain_str(host).is_some_and(|domain| name.eq_ignore_ascii_case(domain)))
}

#[cfg(target_os = "macos")]
pub(super) fn use_persistent_profile() -> bool {
  objc2_foundation::NSProcessInfo::processInfo()
    .operatingSystemVersion()
    .majorVersion
    >= 14
}

#[cfg(target_os = "macos")]
mod platform {
  use super::*;
  use block2::RcBlock;
  use objc2::{MainThreadMarker, MainThreadOnly};
  use objc2_foundation::{NSArray, NSHTTPCookie, NSString};
  use objc2_web_kit::{
    WKUserScript, WKUserScriptInjectionTime, WKWebView, WKWebsiteDataRecord, WKWebsiteDataStore,
  };
  use std::{ptr::NonNull, sync::Mutex};
  use tauri::webview::cookie::{time::OffsetDateTime, Expiration, SameSite};

  pub(super) fn status(view: &PlatformWebview) -> NativeResult<BrowserStatus> {
    // Tauri supplies a live WKWebView on the main thread.
    let view = unsafe { &*view.inner().cast::<WKWebView>() };
    Ok(unsafe {
      BrowserStatus {
        can_back: view.canGoBack(),
        can_forward: view.canGoForward(),
        loading: view.isLoading(),
      }
    })
  }

  pub(super) fn url(view: &PlatformWebview) -> Option<Url> {
    // Tauri supplies a live WKWebView on the main thread.
    let view = unsafe { &*view.inner().cast::<WKWebView>() };
    let address = unsafe { view.URL()?.absoluteString()? };
    Url::parse(&address.to_string()).ok()
  }

  pub(super) fn reload(view: &PlatformWebview) -> NativeResult<()> {
    // Tauri supplies a live WKWebView on the main thread.
    let view = unsafe { &*view.inner().cast::<WKWebView>() };
    let _ = unsafe { view.reload() };
    Ok(())
  }

  pub(super) fn cookies(view: PlatformWebview, _url: Url, send: Sender<Vec<Cookie<'static>>>) {
    // WKWebView's URL-scoped reader misses domain cookies: read all, the caller filters.
    // Tauri supplies a live WKWebView on the main thread; the block retains what it needs.
    let view = unsafe { &*view.inner().cast::<WKWebView>() };
    let send = Mutex::new(Some(send));
    let found = RcBlock::new(move |cookies: NonNull<NSArray<NSHTTPCookie>>| {
      let cookies = unsafe { cookies.as_ref() }
        .iter()
        .map(|cookie| convert(&cookie))
        .collect();
      if let Some(send) = send.lock().ok().and_then(|mut send| send.take()) {
        let _ = send.send(Ok(cookies));
      }
    });
    unsafe {
      view
        .configuration()
        .websiteDataStore()
        .httpCookieStore()
        .getAllCookies(&found);
    }
  }

  /// The catch profile's store by its identifier (the one its tabs are built with): wry asks
  /// WebKit the same way, so this is the very store, not a copy.
  pub(super) fn profile_cookies(send: Sender<Vec<Cookie<'static>>>) {
    use objc2_foundation::NSUUID;
    let Some(main) = MainThreadMarker::new() else {
      let _ = send.send(Err("catchBrowserUnavailable"));
      return;
    };
    let identifier = NSUUID::from_bytes(crate::catch::window::CATCH_STORE);
    let send = Mutex::new(Some(send));
    let found = RcBlock::new(move |cookies: NonNull<NSArray<NSHTTPCookie>>| {
      let cookies = unsafe { cookies.as_ref() }
        .iter()
        .map(|cookie| convert(&cookie))
        .collect();
      if let Some(send) = send.lock().ok().and_then(|mut send| send.take()) {
        let _ = send.send(Ok(cookies));
      }
    });
    // On the main thread (checked above); the identifier is not zero, and the block retains
    // what it needs.
    unsafe {
      WKWebsiteDataStore::dataStoreForIdentifier(&identifier, main)
        .httpCookieStore()
        .getAllCookies(&found);
    }
  }

  /// Mirrors wry's `cookie_from_wkwebview`, keeping the domain's leading dot.
  fn convert(cookie: &NSHTTPCookie) -> Cookie<'static> {
    let expires = match cookie.expiresDate() {
      Some(date) => OffsetDateTime::from_unix_timestamp(date.timeIntervalSince1970() as i64)
        .map_or(Expiration::Session, Expiration::DateTime),
      None => Expiration::Session,
    };
    let mut built = Cookie::build((cookie.name().to_string(), cookie.value().to_string()))
      .domain(cookie.domain().to_string())
      .path(cookie.path().to_string())
      .http_only(cookie.isHTTPOnly())
      .secure(cookie.isSecure())
      .expires(expires)
      .build();
    // macOS 10.15+; compared as text (wry#1616). Only a strict or lax policy is carried: none or
    // unknown is left unset, so a copy treats it as Lax.
    if objc2::available!(macos = 10.15) {
      match cookie.sameSitePolicy().map(|policy| policy.to_string()) {
        Some(policy) if policy == "strict" => built.set_same_site(SameSite::Strict),
        Some(policy) if policy == "lax" => built.set_same_site(SameSite::Lax),
        _ => {}
      }
    }
    built
  }

  pub(super) fn set_cookies(
    view: PlatformWebview,
    cookies: Vec<Cookie<'static>>,
    send: Sender<usize>,
  ) {
    // Tauri supplies a live WKWebView on the main thread; the block retains what it needs.
    let view = unsafe { &*view.inner().cast::<WKWebView>() };
    let made: Vec<_> = cookies.iter().filter_map(into_native).collect();
    let total = made.len();
    if total == 0 {
      let _ = send.send(Ok(0));
      return;
    }
    let left = std::sync::atomic::AtomicUsize::new(total);
    let send = Mutex::new(Some(send));
    let written = RcBlock::new(move || {
      if left.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
        if let Some(send) = send.lock().ok().and_then(|mut send| send.take()) {
          let _ = send.send(Ok(total));
        }
      }
    });
    unsafe {
      let store = view.configuration().websiteDataStore().httpCookieStore();
      for cookie in made {
        store.setCookie_completionHandler(&cookie, Some(&written));
      }
    }
  }

  /// Mirrors wry's `cookie_into_wkwebview`: string-keyed HttpOnly and SameSite properties.
  fn into_native(cookie: &Cookie<'static>) -> Option<objc2::rc::Retained<NSHTTPCookie>> {
    use objc2::runtime::AnyObject;
    use objc2_foundation::{
      ns_string, NSDate, NSHTTPCookieDomain, NSHTTPCookieExpires, NSHTTPCookieName,
      NSHTTPCookiePath, NSHTTPCookiePropertyKey, NSHTTPCookieSecure, NSHTTPCookieValue,
      NSMutableDictionary,
    };
    let (name, value) = (
      NSString::from_str(cookie.name()),
      NSString::from_str(cookie.value()),
    );
    let domain = NSString::from_str(cookie.domain_original().unwrap_or_default());
    let path = NSString::from_str(cookie.path().unwrap_or("/"));
    // The property keys are WebKit's constant strings.
    unsafe {
      let properties: objc2::rc::Retained<NSMutableDictionary<NSHTTPCookiePropertyKey, AnyObject>> =
        NSMutableDictionary::from_slices(
          &[
            NSHTTPCookieName,
            NSHTTPCookieValue,
            NSHTTPCookiePath,
            NSHTTPCookieDomain,
          ],
          &[&name, &value, &path, &domain],
        );
      if let Some(at) = cookie.expires_datetime() {
        let expires = NSDate::dateWithTimeIntervalSince1970(at.unix_timestamp() as f64);
        properties.insert(NSHTTPCookieExpires, &*expires);
      }
      if cookie.secure() == Some(true) {
        properties.insert(NSHTTPCookieSecure, ns_string!("TRUE"));
      }
      if cookie.http_only() == Some(true) {
        properties.insert(ns_string!("HttpOnly"), ns_string!("TRUE"));
      }
      match cookie.same_site() {
        Some(SameSite::Lax) => properties.insert(ns_string!("SameSite"), ns_string!("lax")),
        Some(SameSite::Strict) => properties.insert(ns_string!("SameSite"), ns_string!("strict")),
        _ => {}
      }
      NSHTTPCookie::cookieWithProperties(&properties)
    }
  }

  pub(super) fn set_agent(view: &PlatformWebview, agent: Option<String>) -> NativeResult<()> {
    // Tauri supplies a live WKWebView on the main thread.
    let view = unsafe { &*view.inner().cast::<WKWebView>() };
    let agent = agent.as_deref().map(NSString::from_str);
    unsafe { view.setCustomUserAgent(agent.as_deref()) };
    Ok(())
  }

  pub(super) fn history(view: PlatformWebview, back: bool) -> NativeResult<()> {
    // Tauri supplies a live WKWebView on the main thread.
    let view = unsafe { &*view.inner().cast::<WKWebView>() };
    unsafe {
      if back {
        view.goBack();
      } else {
        view.goForward();
      }
    }
    Ok(())
  }

  pub(super) fn enable_deep(view: PlatformWebview, send: Sender<()>) {
    let Some(main) = MainThreadMarker::new() else {
      return;
    };
    // Tauri supplies a live WKWebView on the main thread.
    let view = unsafe { &*view.inner().cast::<WKWebView>() };
    unsafe {
      let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
        WKUserScript::alloc(main),
        &NSString::from_str(DEEP_SCRIPT),
        WKUserScriptInjectionTime::AtDocumentStart,
        false,
      );
      view
        .configuration()
        .userContentController()
        .addUserScript(&script);
    }
    let _ = send.send(Ok(()));
  }

  pub(super) fn forget(view: PlatformWebview, url: Url, send: Sender<()>) {
    let Some(main) = MainThreadMarker::new() else {
      return;
    };
    let Some(host) = url.host_str().map(str::to_owned) else {
      return;
    };
    // Tauri supplies a live WKWebView on the main thread. The blocks retain the store
    // and data types until WebKit's asynchronous operations complete.
    let view = unsafe { &*view.inner().cast::<WKWebView>() };
    unsafe {
      let store = view.configuration().websiteDataStore();
      let types = WKWebsiteDataStore::allWebsiteDataTypes(main);
      let (record_store, record_types) = (store.clone(), types.clone());
      let send = Mutex::new(Some(send));
      let found = RcBlock::new(move |records: NonNull<NSArray<WKWebsiteDataRecord>>| {
        let matching: Vec<_> = records
          .as_ref()
          .iter()
          .filter(|record| same_website(&record.displayName().to_string(), &host))
          .collect();
        let Ok(mut sender) = send.lock() else { return };
        let Some(sender) = sender.take() else { return };
        if matching.is_empty() {
          let _ = sender.send(Ok(()));
          return;
        }
        let sender = Mutex::new(Some(sender));
        let removed = RcBlock::new(move || {
          if let Ok(mut sender) = sender.lock() {
            if let Some(sender) = sender.take() {
              let _ = sender.send(Ok(()));
            }
          }
        });
        record_store.removeDataOfTypes_forDataRecords_completionHandler(
          &record_types,
          &NSArray::from_retained_slice(&matching),
          &removed,
        );
      });
      store.fetchDataRecordsOfTypes_completionHandler(&types, &found);
    }
  }
}

#[cfg(target_os = "linux")]
mod platform {
  use super::*;
  use gtk::gio;
  use tauri::webview::cookie::{time::OffsetDateTime, Expiration, SameSite};
  use webkit2gtk::{
    CookieManagerExt, UserContentInjectedFrames, UserContentManagerExt, UserScript,
    UserScriptInjectionTime, WebViewExt, WebsiteDataManagerExt, WebsiteDataManagerExtManual,
    WebsiteDataTypes,
  };

  pub(super) fn status(view: &PlatformWebview) -> NativeResult<BrowserStatus> {
    let view = view.inner();
    Ok(BrowserStatus {
      can_back: view.can_go_back(),
      can_forward: view.can_go_forward(),
      loading: view.is_loading(),
    })
  }

  pub(super) fn url(view: &PlatformWebview) -> Option<Url> {
    Url::parse(view.inner().uri()?.as_str()).ok()
  }

  pub(super) fn reload(view: &PlatformWebview) -> NativeResult<()> {
    view.inner().reload();
    Ok(())
  }

  pub(super) fn cookies(view: PlatformWebview, url: Url, send: Sender<Vec<Cookie<'static>>>) {
    let Some(manager) = view
      .inner()
      .website_data_manager()
      .and_then(|manager| manager.cookie_manager())
    else {
      let _ = send.send(Err("catchBrowserUnavailable"));
      return;
    };
    // Mirrors wry's `cookie_from_soup_cookie`, without its nested main loop.
    manager.cookies(url.as_str(), None::<&gio::Cancellable>, move |result| {
      let cookies = result.map_err(|_| "catchCookiesFailed").map(|cookies| {
        cookies
          .into_iter()
          .map(|mut cookie| {
            let expires = cookie
              .expires()
              .and_then(|date| OffsetDateTime::from_unix_timestamp(date.to_unix()).ok())
              .map_or(Expiration::Session, Expiration::DateTime);
            let mut built = Cookie::build((
              cookie.name().map(|n| n.to_string()).unwrap_or_default(),
              cookie.value().map(|v| v.to_string()).unwrap_or_default(),
            ))
            .http_only(cookie.is_http_only())
            .secure(cookie.is_secure())
            .expires(expires);
            if let Some(domain) = cookie.domain() {
              built = built.domain(domain.to_string());
            }
            if let Some(path) = cookie.path() {
              built = built.path(path.to_string());
            }
            // Only a strict or lax policy is carried: none is left unset (a copy makes it Lax).
            match cookie.same_site_policy() {
              soup::SameSitePolicy::Strict => built = built.same_site(SameSite::Strict),
              soup::SameSitePolicy::Lax => built = built.same_site(SameSite::Lax),
              _ => {}
            }
            built.build()
          })
          .collect()
      });
      let _ = send.send(cookies);
    });
  }

  pub(super) fn set_cookies(
    view: PlatformWebview,
    cookies: Vec<Cookie<'static>>,
    send: Sender<usize>,
  ) {
    use std::{cell::Cell, rc::Rc};
    let Some(manager) = view
      .inner()
      .website_data_manager()
      .and_then(|manager| manager.cookie_manager())
    else {
      let _ = send.send(Err("catchBrowserUnavailable"));
      return;
    };
    if cookies.is_empty() {
      let _ = send.send(Ok(0));
      return;
    }
    // Answered from the last callback: the store has every cookie before the page reloads.
    let left = Rc::new(Cell::new(cookies.len()));
    let written = Rc::new(Cell::new(0usize));
    let send = Rc::new(Cell::new(Some(send)));
    for cookie in &cookies {
      // Mirrors wry's `cookie_into_soup_cookie`, keeping the domain's leading dot.
      let mut made = soup::Cookie::new(
        cookie.name(),
        cookie.value(),
        cookie.domain_original().unwrap_or_default(),
        cookie.path().unwrap_or("/"),
        -1,
      );
      if let Some(date) = cookie
        .expires_datetime()
        .and_then(|at| gtk::glib::DateTime::from_unix_utc(at.unix_timestamp()).ok())
      {
        made.set_expires(&date);
      }
      made.set_http_only(cookie.http_only() == Some(true));
      made.set_secure(cookie.secure() == Some(true));
      // libsoup's default is no policy (sent cross-site): the copied policy is always set.
      if let Some(policy) = cookie.same_site() {
        made.set_same_site_policy(match policy {
          SameSite::Strict => soup::SameSitePolicy::Strict,
          SameSite::Lax => soup::SameSitePolicy::Lax,
          SameSite::None => soup::SameSitePolicy::None,
        });
      }
      let (left, written, send) = (left.clone(), written.clone(), send.clone());
      manager.add_cookie(&mut made, None::<&gio::Cancellable>, move |result| {
        if result.is_ok() {
          written.set(written.get() + 1);
        }
        left.set(left.get() - 1);
        if left.get() == 0 {
          if let Some(send) = send.take() {
            let _ = send.send(Ok(written.get()));
          }
        }
      });
    }
  }

  pub(super) fn set_agent(view: &PlatformWebview, agent: Option<String>) -> NativeResult<()> {
    use webkit2gtk::SettingsExt;
    let settings = WebViewExt::settings(&view.inner()).ok_or("catchBrowserUnavailable")?;
    settings.set_user_agent(agent.as_deref());
    Ok(())
  }

  pub(super) fn watch_crash(view: PlatformWebview, crashed: impl Fn() + 'static) {
    view
      .inner()
      .connect_web_process_terminated(move |_, _| crashed());
  }

  pub(super) fn watch_load_failure(view: PlatformWebview, failed: impl Fn(i64, &str) + 'static) {
    use gtk::glib::{ffi::GError, translate::ToGlibPtr};
    use webkit2gtk::{NetworkError, PluginError, PolicyError};

    view
      .inner()
      .connect_load_failed(move |_, _, address, error| {
        if !error.matches(NetworkError::Cancelled)
          && !error.matches(PolicyError::FrameLoadInterruptedByPolicyChange)
          && !error.matches(PluginError::WillHandleLoad)
        {
          let raw: *const GError = error.to_glib_none().0;
          // The pointer borrows the live error for this callback.
          failed(i64::from(unsafe { (*raw).code }), address);
        }
        false // Keep WebKitGTK's error page.
      });
  }

  pub(super) fn history(view: PlatformWebview, back: bool) -> NativeResult<()> {
    let view = view.inner();
    if back {
      view.go_back();
    } else {
      view.go_forward();
    }
    Ok(())
  }

  pub(super) fn enable_deep(view: PlatformWebview, send: Sender<()>) {
    let result = view
      .inner()
      .user_content_manager()
      .ok_or("catchBrowserUnavailable")
      .map(|manager| {
        manager.add_script(&UserScript::new(
          DEEP_SCRIPT,
          UserContentInjectedFrames::AllFrames,
          UserScriptInjectionTime::Start,
          &[],
          &[],
        ));
      });
    let _ = send.send(result);
  }

  pub(super) fn forget(view: PlatformWebview, url: Url, send: Sender<()>) {
    let Some(host) = url.host_str().map(str::to_owned) else {
      return;
    };
    let Some(manager) = view.inner().website_data_manager() else {
      return;
    };
    let types = WebsiteDataTypes::all();
    let store = manager.clone();
    manager.fetch(types, None::<&gio::Cancellable>, move |records| {
      let Ok(records) = records else {
        let _ = send.send(Err("catchForgetFailed"));
        return;
      };
      let matching: Vec<_> = records
        .iter()
        .filter(|record| {
          record
            .name()
            .is_some_and(|name| same_website(name.as_str(), &host))
        })
        .collect();
      if matching.is_empty() {
        let _ = send.send(Ok(()));
        return;
      }
      store.remove(types, &matching, None::<&gio::Cancellable>, move |result| {
        let _ = send.send(result.map_err(|_| "catchForgetFailed"));
      });
    });
  }
}

#[cfg(windows)]
mod platform {
  use super::*;
  use tauri::webview::cookie::{time::OffsetDateTime, Expiration, SameSite};
  use webview2_com::{
    take_pwstr, AddScriptToExecuteOnDocumentCreatedCompletedHandler,
    CallDevToolsProtocolMethodCompletedHandler, GetCookiesCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
      ICoreWebView2, ICoreWebView2Cookie, ICoreWebView2_2, COREWEBVIEW2_COOKIE_SAME_SITE_KIND_LAX,
      COREWEBVIEW2_COOKIE_SAME_SITE_KIND_NONE, COREWEBVIEW2_COOKIE_SAME_SITE_KIND_STRICT,
      COREWEBVIEW2_PROCESS_FAILED_KIND, COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
      COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE,
    },
    ProcessFailedEventHandler,
  };
  use windows::core::{w, Interface, BOOL, HSTRING, PCWSTR, PWSTR};

  fn core(view: &PlatformWebview) -> NativeResult<ICoreWebView2> {
    // Tauri supplies the live WebView2 controller on its owning COM apartment.
    unsafe { view.controller().CoreWebView2() }.map_err(|_| "catchBrowserUnavailable")
  }

  pub(super) fn status(view: &PlatformWebview) -> NativeResult<BrowserStatus> {
    let view = core(view)?;
    let (mut back, mut forward) = (BOOL::default(), BOOL::default());
    // Both out pointers are valid for the duration of the calls.
    unsafe {
      view
        .CanGoBack(&mut back)
        .and_then(|()| view.CanGoForward(&mut forward))
    }
    .map_err(|_| "catchBrowserUnavailable")?;
    Ok(BrowserStatus {
      can_back: back.as_bool(),
      can_forward: forward.as_bool(),
      loading: false,
    })
  }

  pub(super) fn url(view: &PlatformWebview) -> Option<Url> {
    let view = core(view).ok()?;
    let mut source = PWSTR::null();
    // The out pointer is valid for the call; WebView2 allocates the string.
    unsafe { view.Source(&mut source) }.ok()?;
    Url::parse(&take_pwstr(source)).ok()
  }

  pub(super) fn reload(view: &PlatformWebview) -> NativeResult<()> {
    let view = core(view)?;
    unsafe { view.Reload() }.map_err(|_| "catchNavigationFailed")
  }

  pub(super) fn cookies(view: PlatformWebview, url: Url, send: Sender<Vec<Cookie<'static>>>) {
    let Ok(view) = core(&view) else {
      let _ = send.send(Err("catchBrowserUnavailable"));
      return;
    };
    let Ok(manager) = (unsafe {
      view
        .cast::<ICoreWebView2_2>()
        .and_then(|view| view.CookieManager())
    }) else {
      let _ = send.send(Err("catchBrowserUnavailable"));
      return;
    };
    let send = std::sync::Mutex::new(Some(send));
    let reply = move |result: NativeResult<Vec<Cookie<'static>>>| {
      if let Some(send) = send.lock().ok().and_then(|mut send| send.take()) {
        let _ = send.send(result);
      }
    };
    let reply = std::rc::Rc::new(reply);
    let done = reply.clone();
    let callback = GetCookiesCompletedHandler::create(Box::new(move |result, list| {
      let cookies = (|| unsafe {
        result?;
        let mut out = Vec::new();
        if let Some(list) = list {
          let mut count = 0;
          list.Count(&mut count)?;
          for index in 0..count {
            if let Ok(cookie) = convert(list.GetValueAtIndex(index)?) {
              out.push(cookie);
            }
          }
        }
        Ok::<_, windows::core::Error>(out)
      })();
      done(cookies.map_err(|_| "catchCookiesFailed"));
      Ok(())
    }));
    let uri = HSTRING::from(url.as_str());
    // WebView2 copies the URI and retains the callback.
    if unsafe { manager.GetCookies(&uri, &callback) }.is_err() {
      reply(Err("catchCookiesFailed"));
    }
  }

  /// Mirrors wry's `cookie_from_win32`, keeping the domain's leading dot.
  unsafe fn convert(cookie: ICoreWebView2Cookie) -> windows::core::Result<Cookie<'static>> {
    // Every out pointer is valid for its call; WebView2 allocates the strings.
    unsafe {
      let (mut name, mut value, mut domain, mut path) =
        (PWSTR::null(), PWSTR::null(), PWSTR::null(), PWSTR::null());
      cookie.Name(&mut name)?;
      cookie.Value(&mut value)?;
      cookie.Domain(&mut domain)?;
      cookie.Path(&mut path)?;
      let (mut http_only, mut secure, mut session) =
        (BOOL::default(), BOOL::default(), BOOL::default());
      cookie.IsHttpOnly(&mut http_only)?;
      cookie.IsSecure(&mut secure)?;
      cookie.IsSession(&mut session)?;
      let mut expires = 0.0;
      cookie.Expires(&mut expires)?;
      let mut policy = COREWEBVIEW2_COOKIE_SAME_SITE_KIND_NONE;
      cookie.SameSite(&mut policy)?;
      let expires = if session.as_bool() || expires == -1.0 {
        Expiration::Session
      } else {
        OffsetDateTime::from_unix_timestamp(expires as i64)
          .map_or(Expiration::Session, Expiration::DateTime)
      };
      let mut built = Cookie::build((take_pwstr(name), take_pwstr(value)))
        .domain(take_pwstr(domain))
        .path(take_pwstr(path))
        .http_only(http_only.as_bool())
        .secure(secure.as_bool())
        .expires(expires)
        .build();
      // Only a strict or lax policy is carried: none is left unset (a copy makes it Lax).
      if policy == COREWEBVIEW2_COOKIE_SAME_SITE_KIND_STRICT {
        built.set_same_site(SameSite::Strict);
      } else if policy == COREWEBVIEW2_COOKIE_SAME_SITE_KIND_LAX {
        built.set_same_site(SameSite::Lax);
      }
      Ok(built)
    }
  }

  /// Mirrors wry's `cookie_into_win32`, keeping the domain's leading dot. `AddOrUpdateCookie`
  /// writes synchronously: the store has every cookie when this returns.
  pub(super) fn set_cookies(
    view: PlatformWebview,
    cookies: Vec<Cookie<'static>>,
    send: Sender<usize>,
  ) {
    let written: NativeResult<usize> = (|| {
      // Tauri supplies the live WebView2 on its COM apartment; WebView2 copies every string.
      let manager = unsafe {
        core(&view)?
          .cast::<ICoreWebView2_2>()
          .and_then(|view| view.CookieManager())
      }
      .map_err(|_| "catchBrowserUnavailable")?;
      let mut written = 0;
      for cookie in &cookies {
        let added: windows::core::Result<()> = (|| unsafe {
          let made = manager.CreateCookie(
            &HSTRING::from(cookie.name()),
            &HSTRING::from(cookie.value()),
            &HSTRING::from(cookie.domain_original().unwrap_or_default()),
            &HSTRING::from(cookie.path().unwrap_or("/")),
          )?;
          if let Some(at) = cookie.expires_datetime() {
            made.SetExpires(at.unix_timestamp() as f64)?;
          }
          made.SetIsHttpOnly(cookie.http_only() == Some(true))?;
          made.SetIsSecure(cookie.secure() == Some(true))?;
          if let Some(policy) = cookie.same_site() {
            made.SetSameSite(match policy {
              SameSite::Strict => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_STRICT,
              SameSite::Lax => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_LAX,
              SameSite::None => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_NONE,
            })?;
          }
          manager.AddOrUpdateCookie(&made)
        })();
        // A cookie the engine refuses (an odd name or value) is skipped, as on macOS.
        if added.is_ok() {
          written += 1;
        }
      }
      Ok(written)
    })();
    let _ = send.send(written);
  }

  /// The engine's own identity per page, kept while the page presents another one.
  static ENGINE_AGENTS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, String>>,
  > = std::sync::LazyLock::new(Default::default);

  pub(super) fn set_agent(
    view: &PlatformWebview,
    label: &str,
    agent: Option<String>,
  ) -> NativeResult<()> {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings2;
    let view = core(view)?;
    // Tauri supplies the live WebView2 on its COM apartment; WebView2 allocates the read string.
    let settings = unsafe {
      view
        .Settings()
        .and_then(|s| s.cast::<ICoreWebView2Settings2>())
    }
    .map_err(|_| "catchBrowserUnavailable")?;
    let mut engine = ENGINE_AGENTS
      .lock()
      .map_err(|_| "catchBrowserUnavailable")?;
    let restore = agent.is_none();
    let agent = match agent {
      Some(agent) => {
        if !engine.contains_key(label) {
          let mut current = PWSTR::null();
          unsafe { settings.UserAgent(&mut current) }.map_err(|_| "catchBrowserUnavailable")?;
          engine.insert(label.to_owned(), take_pwstr(current));
        }
        agent
      }
      None => match engine.get(label) {
        Some(agent) => agent.clone(),
        None => return Ok(()),
      },
    };
    unsafe { settings.SetUserAgent(&HSTRING::from(agent)) }
      .map_err(|_| "catchBrowserUnavailable")?;
    // Forgotten only once the engine's own identity is really back.
    if restore {
      engine.remove(label);
    }
    Ok(())
  }

  pub(super) fn forget_agent(label: &str) {
    if let Ok(mut engine) = ENGINE_AGENTS.lock() {
      engine.remove(label);
    }
  }

  pub(super) fn watch_crash(view: PlatformWebview, crashed: impl Fn() + 'static) {
    let Ok(view) = core(&view) else { return };
    let handler = ProcessFailedEventHandler::create(Box::new(move |_, args| {
      let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
      if let Some(args) = args {
        // The out pointer is valid for the call.
        unsafe { args.ProcessFailedKind(&mut kind) }?;
      }
      if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
        || kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE
      {
        crashed();
      }
      Ok(())
    }));
    let mut token = 0i64;
    // WebView2 retains the handler for the webview's lifetime.
    let _ = unsafe { view.add_ProcessFailed(&handler, &mut token) };
  }

  pub(super) fn watch_load_failure(view: PlatformWebview, failed: impl Fn(i64, &str) + 'static) {
    use std::{cell::RefCell, rc::Rc};
    use webview2_com::{
      Microsoft::Web::WebView2::Win32::COREWEBVIEW2_WEB_ERROR_STATUS,
      NavigationCompletedEventHandler, NavigationStartingEventHandler,
    };

    let Ok(view) = core(&view) else { return };
    let current = Rc::new(RefCell::new(None));
    let started = current.clone();
    let starting = NavigationStartingEventHandler::create(Box::new(move |_, args| {
      let Some(args) = args else { return Ok(()) };
      let (mut id, mut uri) = (0, PWSTR::null());
      // Out pointers are valid and take_pwstr releases the allocated URI.
      unsafe {
        args.NavigationId(&mut id)?;
        args.Uri(&mut uri)?;
      }
      started.replace(Some((id, take_pwstr(uri))));
      Ok(())
    }));
    let completed = NavigationCompletedEventHandler::create(Box::new(move |_, args| {
      let Some(args) = args else { return Ok(()) };
      let (mut id, mut success, mut code) =
        (0, BOOL::default(), COREWEBVIEW2_WEB_ERROR_STATUS::default());
      // Out pointers are valid for the duration of each call.
      unsafe {
        args.NavigationId(&mut id)?;
        args.IsSuccess(&mut success)?;
        args.WebErrorStatus(&mut code)?;
      }
      let mut current = current.borrow_mut();
      if current.as_ref().is_some_and(|(started, _)| *started == id) {
        if let Some((_, url)) = current
          .take()
          .filter(|_| !success.as_bool() && crate::catch::window::show_windows_load_failure(code.0))
        {
          failed(i64::from(code.0), &url);
        }
      }
      Ok(())
    }));
    let (mut start_token, mut end_token) = (0, 0);
    // WebView2 retains the handlers for this webview's lifetime.
    unsafe {
      if view
        .add_NavigationStarting(&starting, &mut start_token)
        .is_ok()
      {
        let _ = view.add_NavigationCompleted(&completed, &mut end_token);
      }
    }
  }

  pub(super) fn history(view: PlatformWebview, back: bool) -> NativeResult<()> {
    let view = core(&view)?;
    unsafe {
      if back {
        view.GoBack()
      } else {
        view.GoForward()
      }
    }
    .map_err(|_| "catchNavigationFailed")
  }

  pub(super) fn enable_deep(view: PlatformWebview, send: Sender<()>) {
    let Ok(view) = core(&view) else { return };
    let callback =
      AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(move |result, _| {
        let _ = send.send(result.map_err(|_| "catchBrowserUnavailable"));
        Ok(())
      }));
    let script = HSTRING::from(DEEP_SCRIPT);
    // WebView2 copies the script and retains the callback.
    let _ = unsafe { view.AddScriptToExecuteOnDocumentCreated(&script, &callback) };
  }

  pub(super) fn forget(view: PlatformWebview, url: Url, send: Sender<()>) {
    let Ok(view) = core(&view) else { return };
    let Some(host) = url.host_str().map(str::to_owned) else {
      return;
    };
    let result = clear_cookies(view.clone(), host, move |result| {
      if result.is_err() {
        let _ = send.send(result);
        return;
      }
      let mut origins = vec![url.origin().ascii_serialization()];
      let mut other = url.clone();
      let _ = other.set_scheme(if url.scheme() == "http" {
        "https"
      } else {
        "http"
      });
      origins.push(other.origin().ascii_serialization());
      clear_origins(view, origins, send);
    });
    let _ = result;
  }

  fn clear_cookies(
    view: ICoreWebView2,
    host: String,
    done: impl FnOnce(NativeResult<()>) + 'static,
  ) -> windows::core::Result<()> {
    // The cookie manager uses this WebView2 profile, and returns allocated domain strings.
    let manager = unsafe { view.cast::<ICoreWebView2_2>()?.CookieManager()? };
    let cookie_manager = manager.clone();
    let callback = GetCookiesCompletedHandler::create(Box::new(move |result, list| {
      let result = (|| unsafe {
        result?;
        let list = list.ok_or(windows::core::Error::from_hresult(
          windows::Win32::Foundation::E_FAIL,
        ))?;
        let mut count = 0;
        list.Count(&mut count)?;
        for index in 0..count {
          let cookie = list.GetValueAtIndex(index)?;
          let mut domain = PWSTR::null();
          cookie.Domain(&mut domain)?;
          let domain = take_pwstr(domain);
          if domain.trim_start_matches('.').eq_ignore_ascii_case(&host)
            || (domain.starts_with('.') && host.ends_with(&domain))
          {
            cookie_manager.DeleteCookie(&cookie)?;
          }
        }
        Ok::<_, windows::core::Error>(())
      })();
      done(result.map_err(|_| "catchForgetFailed"));
      Ok(())
    }));
    unsafe { manager.GetCookies(PCWSTR::null(), &callback) }
  }

  fn clear_origins(view: ICoreWebView2, mut origins: Vec<String>, send: Sender<()>) {
    let Some(origin) = origins.pop() else {
      let _ = send.send(Ok(()));
      return;
    };
    let params = HSTRING::from(
      serde_json::json!({
        "origin": origin,
          "storageTypes": "all"
      })
      .to_string(),
    );
    let next_view = view.clone();
    let callback =
      CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, _| {
        if result.is_ok() {
          clear_origins(next_view, origins, send);
        } else {
          let _ = send.send(Err("catchForgetFailed"));
        }
        Ok(())
      }));
    // The method deletes data only for the supplied page origin, inside this profile.
    let _ = unsafe {
      view.CallDevToolsProtocolMethod(w!("Storage.clearDataForOrigin"), &params, &callback)
    };
  }
}
