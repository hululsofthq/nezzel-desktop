//! Site tools of the Nezzel browser: reuse this site's sign-in from the user's browser, and
//! reload a page as iPhone Safari. Cookie names and values are never logged, counted or sent.

use super::{
  commands::owns_window,
  hidden_signin::{copy_site, export_matches_copy, same_site, switched_off},
  native,
  state::is_http,
  window,
};
use crate::runners::{
  browser_cookies::{jar_expiry, known_login_host, CookieAccess, CookieContext},
  browser_logins::{self, BrowserCandidate},
  browsers::{BrowserKind, CookieStore},
};
use crate::state::config_models::AuthSettings;
use serde::Serialize;
use std::{
  collections::{HashMap, HashSet},
  future::Future,
  sync::{LazyLock, Mutex},
};
use tauri::{
  webview::cookie::{time::OffsetDateTime, Cookie, Expiration},
  AppHandle, Manager, Url, Webview,
};

const SIGNIN_NONE: &str = "catchSigninNone";
const SIGNIN_DENIED: &str = "catchSigninDenied";
const SIGNIN_UNSUPPORTED: &str = "catchSigninUnsupported";
/// An import is already running for this tab (its answer is on the way).
const SIGNIN_BUSY: &str = "catchSigninBusy";
/// Chromium exports can ask for the macOS Keychain or Linux keyring.
pub(super) const SYSTEM_ASKS: bool = cfg!(any(target_os = "macos", target_os = "linux"));

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SigninImport {
  imported: usize,
  #[serde(skip_serializing_if = "Option::is_none")]
  browser: Option<String>,
}

/// Which signed-in profiles to read for `host`. Known sign-in sites need their session cookie;
/// any other site needs live cookies of its own (there is no site-specific code to know more).
/// A profile whose cookies are bound to its browser (Windows app-bound encryption) cannot be read.
/// Among the signed-in ones the automatic sign-in's browser order applies (Firefox first: it asks
/// for no keychain); the discovery ranking breaks ties.
pub(super) fn pick_profiles(
  mut candidates: Vec<BrowserCandidate>,
  host: &str,
) -> Result<Vec<BrowserCandidate>, &'static str> {
  use crate::runners::login::automatic::source_order;
  let known = known_login_host(host);
  let total = candidates.len();
  // Safari alone shows no sign-in status before it is read.
  let safari_only = total != 0 && candidates.iter().all(|c| c.browser == BrowserKind::Safari);
  candidates.retain(|c| c.signed_in || (!known && c.site_cookies));
  let signed = candidates.len();
  candidates.retain(|c| !c.app_bound);
  candidates.sort_by_key(|c| source_order(c.browser, false, false, false));
  let first = candidates.first();
  let outcome = match first {
    Some(_) => "selected",
    None if signed != 0 || safari_only => SIGNIN_UNSUPPORTED,
    None => SIGNIN_NONE,
  };
  tracing::info!(
    step = "pick",
    candidates = total,
    signed,
    browser = first.map_or("-", |c| c.browser.name()),
    outcome,
    "catch sign-in"
  );
  if first.is_some() {
    Ok(candidates)
  } else {
    Err(outcome)
  }
}

/// Map exporter errors to fixed codes without exposing exporter output.
fn signin_error(code: &str) -> &'static str {
  match code {
    // The user's own "no": the keychain prompt, Full Disk Access for Safari, the setting, a cancel.
    "browserKeychainDenied"
    | "browserSafariAccessDenied"
    | "browserLoginDisabled"
    | "cancelled" => SIGNIN_DENIED,
    "browserProfileMissing" => SIGNIN_NONE,
    // App-bound or DPAPI keys, a locked store, or anything else the exporter could not read:
    // never worked around.
    _ => SIGNIN_UNSUPPORTED,
  }
}

/// Whether a jar row belongs to the page's own site: the same registrable domain (one sign-in
/// shared by a site's own addresses counts as one site), never another site the sign-in leans on.
/// A page at an IP address takes only that address's host-only cookies. A short link or mirror
/// (`fxtwitter.com`) is its own site, as the off switch sees it (`hidden_signin::copy_site`).
pub(super) fn own_site(cookie_host: &str, subdomains: bool, host: &str) -> bool {
  match copy_site(host) {
    Some(site) => copy_site(cookie_host).is_some_and(|key| key == site),
    None => !subdomains && host.parse::<std::net::IpAddr>().is_ok() && cookie_host == host,
  }
}

/// A failed cookie write as the contract's answer: an engine that did not answer or cannot take
/// the cookies is "cannot be read here"; a closed window stays what it is.
fn write_error(code: &'static str) -> &'static str {
  match code {
    "catchWindowClosed" | "catchLeftSite" => code,
    _ => SIGNIN_UNSUPPORTED,
  }
}

/// A page a sign-in may go to: over https only (cookies sent to a plain-http page travel in the
/// clear, and anyone on the network can put such a page in front of the user).
fn is_https(url: &Url) -> bool {
  is_http(url) && url.scheme() == "https"
}

/// Whether the page is still on the site the import was started for.
fn stayed_on_site(now: Option<&Url>, host: &str) -> bool {
  now
    .filter(|now| is_https(now))
    .and_then(Url::host_str)
    .is_some_and(|now| own_site(now, false, host))
}

/// One Netscape jar row as a cookie for the page's store; `None` for a comment, a malformed or
/// expired row, or a host outside this site. The subdomain column decides between a domain cookie
/// (leading dot) and a host-only one. SameSite is not in the jar: the engine's default applies.
fn jar_cookie(line: &str, host: &str, now: i64, browser: BrowserKind) -> Option<Cookie<'static>> {
  let (http_only, fields) = match line.strip_prefix("#HttpOnly_") {
    Some(rest) => (true, rest),
    None => (false, line),
  };
  if fields.is_empty() || fields.starts_with('#') {
    return None;
  }
  let [domain, subdomains, path, secure, expiry, name, value] =
    <[&str; 7]>::try_from(fields.split('\t').collect::<Vec<_>>()).ok()?;
  let subdomains = match subdomains {
    "TRUE" => true,
    "FALSE" => false,
    _ => return None,
  };
  let expiry: i64 = expiry.parse().ok()?;
  let cookie_host = domain
    .strip_prefix('.')
    .unwrap_or(domain)
    .to_ascii_lowercase();
  if name.is_empty() || !own_site(&cookie_host, subdomains || domain.starts_with('.'), host) {
    return None;
  }
  let expires = match expiry {
    0 => Expiration::Session,
    at => {
      let at = jar_expiry(at, Some(browser.store()))?;
      if at.unix_timestamp() <= now {
        return None;
      }
      Expiration::DateTime(at)
    }
  };
  let domain = if subdomains {
    format!(".{cookie_host}")
  } else {
    cookie_host
  };
  Some(
    Cookie::build((name.to_owned(), value.to_owned()))
      .domain(domain)
      .path(if path.starts_with('/') { path } else { "/" }.to_owned())
      .secure(secure == "TRUE")
      .http_only(http_only)
      .expires(expires)
      .build(),
  )
}

/// Whether an import offered for `offered` may go on with the page now at `live`: the tab still
/// shows that offer (`asked`), over https, still on that site (a redirect or another navigation
/// since the offer ends it), a site whose export reads itself (never a mirror's), and not switched
/// off (`off`: the settings and the native list, read now). Returns the host.
pub(super) fn import_allowed(
  offered: &str,
  asked: bool,
  live: Option<&Url>,
  off: impl FnOnce(&str) -> bool,
) -> Result<String, &'static str> {
  let live = live.filter(|live| asked && is_https(live));
  let host = live.and_then(Url::host_str).ok_or("catchLeftSite")?;
  if !same_site(host, offered) || !export_matches_copy(host) {
    return Err("catchLeftSite");
  }
  if off(host) {
    return Err(SIGNIN_DENIED);
  }
  Ok(host.to_owned())
}

fn profile_read_allowed(
  session: &window::Session,
  host: &str,
  off: impl FnOnce(&str) -> bool,
) -> Result<(), &'static str> {
  let state = session.active()?;
  if state.private {
    return Err(SIGNIN_UNSUPPORTED);
  }
  import_allowed(host, state.page.signin_hint, Some(&state.page.url), off).map(|_| ())
}

/// Reads the private jar the export wrote and drops it: its directory is removed here, before any
/// cookie reaches the page, on success and on failure alike.
pub(super) fn take_jar(
  access: CookieAccess,
  host: &str,
  now: i64,
  browser: BrowserKind,
) -> Result<Vec<Cookie<'static>>, &'static str> {
  let read = access
    .args
    .windows(2)
    .find(|args| args[0] == "--cookies")
    .and_then(|args| std::fs::read_to_string(&args[1]).ok());
  drop(access);
  Ok(
    read
      .ok_or(SIGNIN_UNSUPPORTED)?
      .lines()
      .filter_map(|line| jar_cookie(line, host, now, browser))
      .collect(),
  )
}

pub(super) fn unix_now() -> i64 {
  OffsetDateTime::now_utc().unix_timestamp()
}

/// A private tab takes nothing from the user's browser: its store is thrown away with it.
pub(super) fn importable(tab: &window::TabPage) -> Result<(), &'static str> {
  if tab.private || tab.session.active()?.private {
    return Err(SIGNIN_UNSUPPORTED);
  }
  Ok(())
}

async fn page_url(page: &Webview) -> Result<Url, &'static str> {
  native::url(page)
    .await
    .filter(is_https)
    .ok_or("catchInvalidUrl")
}

/// Rank the signed-in profiles of the browsers this import is allowed to read.
pub(super) async fn signed_in_profiles(
  host: &str,
  keep: impl Fn(BrowserKind) -> bool + Send + 'static,
) -> Result<Vec<BrowserCandidate>, &'static str> {
  let host = host.to_owned();
  tauri::async_runtime::spawn_blocking(move || {
    let mut candidates = browser_logins::discover(None, &host);
    candidates.retain(|candidate| keep(candidate.browser));
    pick_profiles(candidates, &host)
  })
  .await
  .map_err(|_| SIGNIN_UNSUPPORTED)?
}

/// Read the first usable jar. A missing profile is detected before requesting a key, so it
/// leaves the prompt available. After starting a potentially prompting export, try quiet
/// browsers only: granting "Allow once" cannot authorize another Keychain question.
async fn try_profiles<F>(
  profiles: Vec<BrowserCandidate>,
  system_asks: bool,
  allowed: impl Fn() -> Result<(), &'static str> + Send,
  mut read: impl FnMut(BrowserCandidate) -> F + Send,
) -> Result<(BrowserKind, Vec<Cookie<'static>>), &'static str>
where
  F: Future<Output = Result<Vec<Cookie<'static>>, &'static str>> + Send,
{
  let mut prompt_used = false;
  let mut exhausted = SIGNIN_NONE;
  for profile in profiles {
    let browser = profile.browser;
    let may_ask = system_asks && browser.store() == CookieStore::Chromium;
    if may_ask && prompt_used {
      exhausted = SIGNIN_UNSUPPORTED;
      tracing::info!(
        step = "read",
        browser = browser.name(),
        outcome = "promptUsed",
        "catch sign-in"
      );
      continue;
    }
    allowed()?;
    let result = read(profile).await;
    let outcome = match &result {
      Ok(cookies) if cookies.is_empty() => "empty",
      Ok(_) => "ready",
      Err(code) if *code == "browserProfileMissing" => "profileMissing",
      Err(code) => signin_error(code),
    };
    tracing::info!(
      step = "read",
      browser = browser.name(),
      outcome,
      "catch sign-in"
    );
    match result {
      Ok(cookies) if !cookies.is_empty() => return Ok((browser, cookies)),
      Ok(_) => prompt_used |= may_ask,
      Err("browserProfileMissing") => {}
      Err(_) if outcome == SIGNIN_DENIED => return Err(outcome),
      Err(_) => {
        prompt_used |= may_ask;
        exhausted = outcome;
      }
    }
  }
  Err(exhausted)
}

pub(super) async fn read_signin_profiles(
  profiles: Vec<BrowserCandidate>,
  settings: &AuthSettings,
  url: &Url,
  context: &CookieContext,
  allowed: impl Fn() -> Result<(), &'static str> + Send,
) -> Result<(BrowserKind, Vec<Cookie<'static>>), &'static str> {
  let host = url.host_str().ok_or(SIGNIN_UNSUPPORTED)?;
  // Dropping the sender (for example, at the hidden copy's time limit) cancels the export.
  let (_running, cancel) = tokio::sync::watch::channel(true);
  try_profiles(profiles, SYSTEM_ASKS, allowed, move |profile| {
    let cancel = cancel.clone();
    async move {
      let access =
        CookieAccess::prepare(settings, Some(&profile.id), url.as_str(), context, cancel).await?;
      take_jar(access, host, unix_now(), profile.browser)
    }
  })
  .await
}

/// One INFO line per import or sign-in check: the registrable domain and a fixed outcome code
/// only (never cookies, profile names, paths or the address).
fn log_signin(step: &'static str, host: &str, outcome: &str) {
  let site = copy_site(host).unwrap_or_else(|| "-".to_owned());
  tracing::info!(step, site = %site, outcome, "catch sign-in");
}

#[tauri::command]
pub(crate) async fn catch_use_browser_signin(
  app: AppHandle,
  caller: Webview,
  window_id: String,
  tab_id: Option<String>,
  site: String,
) -> Result<SigninImport, &'static str> {
  if !owns_window(caller.label(), &window_id) {
    return Err("catchWrongWindow");
  }
  let mut host = String::new();
  let result = import_signin(&app, &window_id, tab_id.as_deref(), &site, &mut host).await;
  if !host.is_empty() {
    let outcome = match &result {
      Ok(_) => "imported",
      Err(code) => code,
    };
    log_signin("import", &host, outcome);
  }
  result
}

/// The import itself, for the site the offer was raised for (`offered`); `host` is filled once
/// the page's address is known (for the log line).
async fn import_signin(
  app: &AppHandle,
  window_id: &str,
  tab_id: Option<&str>,
  offered: &str,
  host_out: &mut String,
) -> Result<SigninImport, &'static str> {
  let tab = app.state::<window::CatchWindows>().tab(window_id, tab_id)?;
  importable(&tab)?;
  let (page, session) = (window::page(app, &tab.label)?, tab.session);
  // One export at a time per tab: a second click never asks the keychain again.
  let Ok(_importing) = session.signin.try_lock() else {
    return Err(SIGNIN_BUSY);
  };
  let url = page_url(&page).await?;
  host_out.push_str(url.host_str().ok_or("catchInvalidUrl")?);
  // Before any browser is read: the tab shows the offer, the page is still on the offered site,
  // and both switches (the setting, this site) are on.
  let off = |host: &str| !crate::runners::login::automatic::enabled(app) || switched_off(app, host);
  let asked = || session.active().map(|state| state.page.signin_hint);
  let host = import_allowed(offered, asked()?, Some(&url), off)?;
  let profiles = signed_in_profiles(&host, |_| true).await?;
  let context = CookieContext::for_app(app).map_err(|_| SIGNIN_UNSUPPORTED)?;
  let settings = app.state::<crate::SharedConfig>().load().auth.clone();
  let (browser, cookies) = read_signin_profiles(profiles, &settings, &url, &context, || {
    profile_read_allowed(&session, &host, off)
  })
  .await?;
  // The export may have taken a while (a keychain prompt): no navigation of ours from here on,
  // and nothing is written when the page has left the site or the site was switched off meanwhile.
  let _action = session.action.lock().await;
  import_allowed(&host, asked()?, page_url(&page).await.ok().as_ref(), off)?;
  if session.active()?.private {
    return Err(SIGNIN_UNSUPPORTED);
  }
  // The page can still move by itself: its address is read again in the turn that writes.
  let on_site = host.clone();
  let imported = native::set_cookies_if(&page, cookies, move |now| stayed_on_site(now, &on_site))
    .await
    .map_err(write_error)?;
  if imported == 0 {
    // The engine took none of them: no reload, nothing counted.
    return Err(SIGNIN_UNSUPPORTED);
  }
  // The page itself may have moved on during the write: another site's page is not reloaded.
  native::reload_if(&page, move |now| stayed_on_site(Some(now), &host))
    .await?
    .ok_or("catchLeftSite")?;
  crate::usage::count_catch(app, crate::usage::keys::CATCH_SIGNIN_IMPORTED, &url);
  Ok(SigninImport {
    imported,
    browser: Some(browser.name().to_owned()),
  })
}

/// iPhone Safari as `version` names it (the installed Safari's version where known).
pub(super) fn iphone_agent(version: &str) -> Option<String> {
  window::safari_agent(version)?; // the same version check as the desktop identity
  let ios = version.split('.').take(2).collect::<Vec<_>>().join("_");
  Some(format!(
    "Mozilla/5.0 (iPhone; CPU iPhone OS {ios} like Mac OS X) AppleWebKit/605.1.15 (KHTML, like \
     Gecko) Version/{version} Mobile/15E148 Safari/604.1"
  ))
}

/// The iPhone identity for this system: the installed Safari's version on macOS, a current one elsewhere.
pub(super) fn mobile_agent() -> String {
  const FALLBACK: &str = "18.5";
  window::browser_version()
    .and_then(iphone_agent)
    .or_else(|| iphone_agent(FALLBACK))
    .expect("constant version")
}

#[tauri::command]
pub(crate) async fn catch_try_mobile(
  app: AppHandle,
  caller: Webview,
  window_id: String,
  tab_id: Option<String>,
  on: bool,
) -> Result<(), &'static str> {
  if !owns_window(caller.label(), &window_id) {
    return Err("catchWrongWindow");
  }
  let tab = app
    .state::<window::CatchWindows>()
    .tab(&window_id, tab_id.as_deref())?;
  let window::TabPage { label, session, .. } = tab;
  if let Some(url) = switch_identity(&app, &label, &session, on).await? {
    if on {
      crate::usage::count_catch(&app, crate::usage::keys::CATCH_MOBILE_RETRY, &url);
    }
  }
  Ok(())
}

/// Switches the page between the Mobile version and its own identity and reloads a web page;
/// returns the reloaded address. Also the retry ladder's mobile step and its restore.
pub(super) async fn switch_identity(
  app: &AppHandle,
  label: &str,
  session: &window::Session,
  on: bool,
) -> Result<Option<Url>, &'static str> {
  let page = window::page(app, label)?;
  // Off: the desktop identity the page was built with (`None`: the engine's own).
  let agent = if on {
    // Marked before the switch: a window that may present iPhone is never kept for reuse.
    session.active()?.mobile = true;
    Some(mobile_agent())
  } else {
    window::browser_agent().map(str::to_owned)
  };
  let switched = native::set_agent(&page, agent).await;
  if !on && switched.is_ok() {
    session.active()?.mobile = false;
  }
  // The tab's row says which identity it has now, without waiting for the next read.
  window::notify(app, label);
  switched?;
  native::reload_if(&page, is_http).await
}

/// Login words a sign-in wall's address carries, as a path segment or host label part.
const LOGIN_WORDS: [&str; 6] = ["login", "signin", "sign-in", "account", "auth", "sso"];

/// Whether the page looks like a sign-in wall: nothing to save yet, and a visible password field
/// or a top-frame address naming a sign-in step.
pub(super) fn signin_wall(url: &Url, has_candidate: bool, password_visible: bool) -> bool {
  if has_candidate || !is_https(url) {
    return false;
  }
  if password_visible {
    return true;
  }
  let words = |part: &str| {
    part
      .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
      .any(|word| {
        let word = word.to_ascii_lowercase();
        LOGIN_WORDS.iter().any(|login| {
          word == *login
            || word
              .strip_prefix(login)
              .is_some_and(|rest| rest.is_empty() || rest.starts_with('-') || rest == "s")
        })
      })
  };
  url
    .host_str()
    .is_some_and(|host| host.split('.').any(words))
    || url
      .path_segments()
      .is_some_and(|mut parts| parts.any(words))
}

/// First-visit import, on (DECISIONS.md 2026-11-28: the owner, signed in to Chrome, expects the
/// Nezzel browser to open YouTube signed in; replaces 2026-11-25 "only when the page asks").
/// A known sign-in site (YouTube, Facebook…) takes its sign-in on its first visit in this
/// app run when the switch is on, without waiting for a sign-in wall. The switch, private tabs and
/// "a browser is signed in" still apply; a site without a known session cookie waits for a wall.
pub(super) const IMPORT_ON_FIRST_VISIT: bool = true;

/// Why a drained page looks for a signed-in browser profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Look {
  /// A sign-in wall: by address, or a visible password field on the top page.
  Wall,
  /// The first visit of a known sign-in site in this app run (`IMPORT_ON_FIRST_VISIT`).
  FirstVisit,
}

/// Whether this drained page should look for a signed-in browser profile, and why.
pub(super) fn look_for(page: &super::state::PageState, first_visit: bool) -> Option<Look> {
  if page.signin_hint || !is_https(&page.url) {
    return None;
  }
  // A mirror's export would read the mirrored site's browser sign-in: never looked for.
  let host = page
    .url
    .host_str()
    .filter(|host| export_matches_copy(host))?;
  if signin_wall(&page.url, page.has_candidate(), page.password_visible) {
    return Some(Look::Wall);
  }
  let known = known_login_host(host);
  (first_visit && known).then_some(Look::FirstVisit)
}

/// Which sites were looked at, behind one lock so two tabs draining at once start one look.
#[derive(Default)]
struct Looked {
  /// Per window: sites it already offered (or is looking at). At most one offer per site per window.
  windows: HashMap<String, HashSet<String>>,
  /// Sites checked in this app run: a first visit looks once per site per run, in any window.
  run: HashSet<String>,
}

static LOOKED: LazyLock<Mutex<Looked>> = LazyLock::new(Default::default);

/// Whether to look now. A wall looks once per site per window; a first visit once per site per
/// run. Either marks both, so the reload after an import never looks again (`look_ended` frees the
/// window's mark of a first visit that offered nothing). Never for a private tab or a site the
/// user switched off (`off` is read only when a look is due); an off site marks nothing, so
/// switching it on looks at the next read.
pub(super) fn claim(
  window: &str,
  site: &str,
  page: &super::state::PageState,
  private: bool,
  first_visit: bool,
  off: impl FnOnce() -> bool,
) -> Option<Look> {
  if private {
    return None;
  }
  let look = look_for(page, first_visit)?;
  // A known site's later pages are first visits too: skip the settings read once it looked.
  if look == Look::FirstVisit && LOOKED.lock().ok()?.run.contains(site) {
    return None;
  }
  if off() {
    return None;
  }
  let mut looked = LOOKED.lock().ok()?;
  let seen = looked.windows.get(window).is_some_and(|s| s.contains(site));
  if seen || (look == Look::FirstVisit && looked.run.contains(site)) {
    return None;
  }
  let sites = looked.windows.entry(window.to_owned()).or_default();
  sites.insert(site.to_owned());
  looked.run.insert(site.to_owned());
  Some(look)
}

/// A look is over. Without an offer shown it was not the window's one offer for the site: a first
/// visit leaves a later real wall its look, and a look whose page moved on before the offer (a
/// login redirect) leaves it to the next page. A wall with no signed-in browser (`found` false)
/// keeps its mark: the same wall is not scanned for again on every read. A first visit that found
/// a signed-in browser but whose page moved on (YouTube's consent or redirect page) also frees
/// the run's mark, so the page it landed on gets the first visit; with none found it stays marked.
fn look_ended(window: &str, site: &str, look: Look, found: bool, offered: bool) {
  if !offered && (found || look == Look::FirstVisit) {
    if let Ok(mut looked) = LOOKED.lock() {
      if let Some(sites) = looked.windows.get_mut(window) {
        sites.remove(site);
      }
      if look == Look::FirstVisit && found {
        looked.run.remove(site);
      }
    }
  }
}

#[cfg(test)]
fn forget_run(sites: &[&str]) {
  let mut looked = LOOKED.lock().unwrap();
  for site in sites {
    looked.run.remove(*site);
  }
}

/// Called with each drained tab page (`id`: its label): on a sign-in wall of a site not checked
/// yet in this window (or a known sign-in site's first visit, `IMPORT_ON_FIRST_VISIT`), looks
/// (off the main thread) for a browser profile signed in to it and raises the offer on that tab;
/// the shell imports when the site's switch is on.
pub(super) fn observe(app: &AppHandle, id: &str, page: &super::state::PageState) {
  let Some(site) = page.url.host_str().and_then(copy_site) else {
    return;
  };
  let host = page.url.host_str().unwrap_or_default();
  let Some(look) = claim(
    window::window_of(id),
    &site,
    page,
    false, // `offers_signin` already left out private tabs
    IMPORT_ON_FIRST_VISIT,
    // Off in Settings counts like this site's own switch: nothing is marked, so switching it on
    // looks at the next read.
    || !crate::runners::login::automatic::enabled(app) || switched_off(app, host),
  ) else {
    return;
  };
  let step = match look {
    Look::Wall => "wall",
    Look::FirstVisit => "firstVisit",
  };
  let (app, id, generation, host) = (
    app.clone(),
    id.to_owned(),
    page.generation,
    page.url.host_str().unwrap_or_default().to_owned(),
  );
  tauri::async_runtime::spawn(async move {
    let found = signed_in_profiles(&host, |_| true).await;
    // Shown only on the page that was read: a page that moved on meanwhile gets its own look.
    let offered = found.is_ok()
      && app
        .state::<window::CatchWindows>()
        .session(&id)
        .ok()
        .and_then(|session| {
          let mut state = session.active().ok()?;
          (state.page.generation == generation).then(|| state.page.signin_hint = true)
        })
        .is_some();
    look_ended(window::window_of(&id), &site, look, found.is_ok(), offered);
    let outcome = match found {
      Err(code) => code,
      Ok(_) if offered => "offer",
      Ok(_) => "pageMoved",
    };
    log_signin(step, &host, outcome);
    if offered {
      window::notify(&app, &id);
    }
  });
}

/// A closed window forgets which sites it offered (each page's kept identity goes with the page).
pub(super) fn forget_window(id: &str) {
  if let Ok(mut looked) = LOOKED.lock() {
    looked.windows.remove(id);
  }
}

#[cfg(test)]
#[path = "site_tools_tests.rs"]
mod tests;
