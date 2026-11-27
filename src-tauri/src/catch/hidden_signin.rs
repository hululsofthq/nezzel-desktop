//! Automatic catch uses the sign-ins the user already has (owner decision 2026-11-25): before a
//! hidden page loads its link, that site's cookies are copied into the page's throwaway store
//! from the Nezzel browser's saved profile and from the user's signed-in browser. The copy is
//! one way (nothing is written back into either), only the link's own site, never from a private
//! tab, never for a site the user switched off. The page's store is still emptied when the page is
//! parked or closed. Cookie names and values are never logged, counted or sent.

use super::{
  native,
  site_tools::{self, SYSTEM_ASKS},
  window,
};
use crate::runners::browser_cookies::{registrable_domain, site_key, CookieContext};
use crate::runners::browsers::{BrowserKind, BrowserSetting, BrowserSource, CookieStore};
use crate::state::config_models::AuthSettings;
use futures::future::BoxFuture;
use std::{collections::HashMap, io::Write, path::Path, sync::Mutex, time::Duration};
use tauri::{
  webview::cookie::{Cookie, Expiration, SameSite},
  AppHandle, Manager, Url, Webview,
};

/// The longest a copy may hold up the hidden page's first load, reads and write together. Each
/// source gets until `READ_SHARE` of it (one slow source never drops the other's cookies); the
/// write gets the rest. Past it the page loads unsigned, or is dropped if a write may still land.
const COPY_LIMIT: Duration = Duration::from_secs(6);
const READ_SHARE: (u32, u32) = (2, 3);
const OFF_FILE: &str = "catch-signin-off.json";
const MAX_OFF: usize = 200;

/// Where the saved catch profile can be read without touching a private store.
#[derive(Debug, PartialEq)]
pub(super) enum ProfileReader<T> {
  /// macOS 14+: the profile's own data store, read with no page.
  Store,
  /// Windows/Linux: an open normal tab's page (its engine is on the profile).
  Tab(T),
  None,
}

/// One tab of the Nezzel browser as the profile reader sees it.
pub(super) struct TabSource<T> {
  pub(super) page: T,
  /// The tab, or the page it holds, keeps its own throwaway store.
  pub(super) private: bool,
  /// A sleeping tab has no engine to ask.
  pub(super) asleep: bool,
}

/// The saved store where the system has one; otherwise the first awake normal tab. A private
/// tab's store is never the profile, so it is never read.
pub(super) fn profile_reader<T>(
  store: bool,
  tabs: impl IntoIterator<Item = TabSource<T>>,
) -> ProfileReader<T> {
  if store {
    return ProfileReader::Store;
  }
  tabs
    .into_iter()
    .find(|tab| !tab.private && !tab.asleep)
    .map_or(ProfileReader::None, |tab| ProfileReader::Tab(tab.page))
}

fn expired(cookie: &Cookie<'_>, now: i64) -> bool {
  matches!(cookie.expires(), Some(Expiration::DateTime(at)) if at.unix_timestamp() <= now)
}

/// The site a copy is for: the registrable domain; a known site's own domains count as one
/// (`twitter.com` is `x.com`), its short links and third-party mirrors never (`fxtwitter.com`,
/// `youtube-nocookie.com` stay themselves).
pub(super) fn copy_site(host: &str) -> Option<String> {
  let domain = registrable_domain(host)?;
  if crate::sites::site_by_domain(&domain).is_some() {
    site_key(&domain)
  } else {
    Some(domain)
  }
}

/// A cookie of `cookie_host` belongs to the link's site. A link at an address takes that
/// address's host-only cookies only.
fn copy_own_site(cookie_host: &str, subdomains: bool, host: &str) -> bool {
  let unbracket = |h: &str| h.trim_start_matches('[').trim_end_matches(']').to_owned();
  match copy_site(host) {
    Some(site) => copy_site(cookie_host).is_some_and(|key| key == site),
    None => {
      let (host, cookie_host) = (unbracket(host), unbracket(cookie_host));
      !subdomains && host.parse::<std::net::IpAddr>().is_ok() && cookie_host == host
    }
  }
}

/// Only the link's own site (`copy_own_site`): a domain cookie of the same site or a host-only
/// cookie of one of its hosts. Expired and nameless cookies are dropped; an
/// unknown SameSite becomes Lax.
pub(super) fn site_only(
  cookies: Vec<Cookie<'static>>,
  host: &str,
  now: i64,
) -> Vec<Cookie<'static>> {
  cookies
    .into_iter()
    .filter(|cookie| {
      let Some(domain) = cookie.domain_original().filter(|d| !d.is_empty()) else {
        return false;
      };
      let bare = domain.trim_start_matches('.').to_ascii_lowercase();
      !cookie.name().is_empty()
        && !expired(cookie, now)
        && copy_own_site(&bare, domain.starts_with('.'), host)
    })
    .map(|mut cookie| {
      // Neither the engines' readers nor the jar keep SameSite: a copied cookie the source named
      // no policy for is sent to its own site's pages only (Lax), never along cross-site requests.
      if cookie.same_site().is_none() {
        cookie.set_same_site(SameSite::Lax);
      }
      cookie
    })
    .collect()
}

fn key(cookie: &Cookie<'_>) -> (String, String, String) {
  (
    cookie.name().to_owned(),
    cookie
      .domain_original()
      .unwrap_or_default()
      .to_ascii_lowercase(),
    cookie.path().unwrap_or("/").to_owned(),
  )
}

/// How recent a cookie is, as far as a store tells: neither engine nor jar keeps the time a cookie
/// was set, so the later expiry stands for the later write (sites renew a sign-in with a fixed
/// lifetime). A session cookie ranks below any dated one.
fn freshness(cookie: &Cookie<'_>) -> i64 {
  match cookie.expires() {
    Some(Expiration::DateTime(at)) => at.unix_timestamp(),
    _ => i64::MIN,
  }
}

/// Both sign-ins together: one cookie per name/domain/path, the newer one; on a tie the user's
/// own browser (the one they use every day).
pub(super) fn merge(
  profile: Vec<Cookie<'static>>,
  browser: Vec<Cookie<'static>>,
) -> Vec<Cookie<'static>> {
  let mut merged: HashMap<_, Cookie<'static>> = HashMap::new();
  let mut order = Vec::new();
  for (cookie, from_browser) in profile
    .into_iter()
    .map(|c| (c, false))
    .chain(browser.into_iter().map(|c| (c, true)))
  {
    let key = key(&cookie);
    match merged.get(&key) {
      Some(kept) if freshness(kept) > freshness(&cookie) => {}
      Some(kept) if freshness(kept) == freshness(&cookie) && !from_browser => {}
      Some(_) => {
        merged.insert(key, cookie);
      }
      None => {
        order.push(key.clone());
        merged.insert(key, cookie);
      }
    }
  }
  order
    .into_iter()
    .filter_map(|key| merged.remove(&key))
    .collect()
}

/// Whether reading this browser shows no prompt. Firefox has no key and Safari answers with Full
/// Disk Access or nothing. Where the system may ask (`asks`), a Chromium browser is read only when
/// the main app's browser sign-in already uses it, so the grant the user gave there is reused;
/// nothing else may make a hidden page ask.
pub(super) fn reads_quietly(asks: bool, kind: BrowserKind, auth: &AuthSettings) -> bool {
  if !asks || kind.store() != CookieStore::Chromium {
    return true;
  }
  let fixed = match BrowserSetting::of(auth) {
    BrowserSetting::Fixed(browser) => Some(browser),
    _ => None,
  };
  auth
    .site_browsers
    .values()
    .map(String::as_str)
    .chain(fixed)
    .filter_map(BrowserSource::parse)
    .any(|source| source.kind == kind)
}

/// Sites where the user switched the browser sign-in off (the Nezzel browser's switch). Kept in
/// Rust so a hidden page can read it; the shell's old local list is moved in once.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct OffList {
  sites: Vec<String>,
  /// The file exists: the shell's old list was taken (or there was none).
  saved: bool,
}

/// A host as the switch names it: lowercase; a name (letters, digits, dots, dashes) or an
/// address (`[v6]` as a link writes it). A port, a scheme or anything else is refused.
fn clean(site: &str) -> Option<String> {
  let site = site.trim().trim_end_matches('.').to_ascii_lowercase();
  if site.is_empty() || site.len() > 253 {
    return None;
  }
  if let Some(v6) = site.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
    return v6.parse::<std::net::Ipv6Addr>().is_ok().then_some(site);
  }
  if site.parse::<std::net::Ipv4Addr>().is_ok() {
    return Some(site);
  }
  let name_chars = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-');
  // Digits and dots only but no address (`7`): a link would read it as an address, not a name.
  let numeric = site.chars().all(|c| c.is_ascii_digit() || c == '.');
  (site.chars().all(name_chars) && !numeric && !site.starts_with(['.', '-'])).then_some(site)
}

/// What the list keeps for a switched-off host: its registrable domain, so every host of the site
/// is one entry; an address or a bare name stays itself.
fn entry(site: &str) -> Option<String> {
  let host = clean(site)?;
  Some(registrable_domain(&host).unwrap_or(host))
}

/// Whether the browser export for `host` reads the same site the copy keeps. A mirror link
/// (`fxtwitter.com`) would export the mirrored site's cookies (the export folds short links),
/// so its browser is not read at all.
pub(super) fn export_matches_copy(host: &str) -> bool {
  copy_site(host) == site_key(host)
}

/// The same site as the copy sees it (`copy_site`): a mirror is its own site.
pub(super) fn same_site(a: &str, b: &str) -> bool {
  match (copy_site(a), copy_site(b)) {
    (Some(a), Some(b)) => a == b,
    _ => a.eq_ignore_ascii_case(b),
  }
}

impl OffList {
  /// `None` for a file that is not a list: every site then counts as off (`load`).
  pub(super) fn from_json(raw: &str) -> Option<Self> {
    let sites: Vec<String> = serde_json::from_str(raw).ok()?;
    let mut list = Self {
      sites: Vec::new(),
      saved: true,
    };
    for site in sites {
      list.set(&site, true);
    }
    Some(list)
  }

  pub(super) fn to_json(&self) -> String {
    serde_json::to_string(&self.sites).unwrap_or_else(|_| "[]".into())
  }

  pub(super) fn sites(&self) -> &[String] {
    &self.sites
  }

  /// Whether the link's site is off: any switched-off host of the same registrable domain.
  pub(super) fn off(&self, host: &str) -> bool {
    self.sites.iter().any(|site| same_site(site, host))
  }

  /// Returns whether the list changed. One entry per site: switching a host on again clears every
  /// entry of its site. The newest choice is last; the oldest go past the cap.
  pub(super) fn set(&mut self, site: &str, off: bool) -> bool {
    let Some(site) = entry(site) else {
      return false;
    };
    let before = self.sites.clone();
    self.sites.retain(|kept| !same_site(kept, &site));
    if off {
      self.sites.push(site);
      if self.sites.len() > MAX_OFF {
        self.sites.drain(..self.sites.len() - MAX_OFF);
      }
    }
    self.sites != before
  }

  /// The shell's old list, taken once: after the first save it is never read again.
  pub(super) fn migrate(&mut self, legacy: &[String]) -> bool {
    if self.saved {
      return false;
    }
    for site in legacy {
      self.set(site, true);
    }
    self.saved = true;
    true
  }
}

static OFF: Mutex<Option<OffList>> = Mutex::new(None);

fn off_file(app: &AppHandle) -> Option<std::path::PathBuf> {
  Some(app.path().app_data_dir().ok()?.join(OFF_FILE))
}

const PREFERENCES_FAILED: &str = "catchPreferencesFailed";

/// The list as read from disk. No file yet is an empty list (the shell's old one still to move
/// in); a file that cannot be read or is not a list is `None`: every site counts as off and the
/// file is never written over.
pub(super) fn load(read: std::io::Result<String>) -> Option<OffList> {
  match read {
    Ok(raw) => OffList::from_json(&raw),
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(OffList::default()),
    Err(_) => None,
  }
}

/// Written whole or not at all: a private temporary file, flushed, then renamed over the list.
fn save(file: &Path, list: &OffList) -> std::io::Result<()> {
  let dir = file.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
  std::fs::create_dir_all(dir)?;
  let temp = file.with_extension("json.tmp");
  let mut options = std::fs::OpenOptions::new();
  options.write(true).create(true).truncate(true);
  #[cfg(unix)]
  std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
  let written = (|| {
    let mut out = options.open(&temp)?;
    out.write_all(list.to_json().as_bytes())?;
    out.sync_all()?;
    std::fs::rename(&temp, file)
  })();
  if written.is_err() {
    let _ = std::fs::remove_file(&temp);
  }
  written
}

/// The off list behind `slot` (read from `file` once). `edit` works on a copy and says whether to
/// write it back; the copy is kept only once it is on disk, so memory never says more than the
/// file. A file that cannot be read stays unread (never cached, never written over).
pub(super) fn off_list_at<T>(
  slot: &mut Option<OffList>,
  file: &Path,
  edit: impl FnOnce(&mut OffList) -> (bool, T),
) -> Result<T, &'static str> {
  let mut list = match slot {
    Some(list) => list.clone(),
    None => load(std::fs::read_to_string(file)).ok_or(PREFERENCES_FAILED)?,
  };
  let (changed, out) = edit(&mut list);
  if changed {
    save(file, &list).map_err(|_| PREFERENCES_FAILED)?;
  }
  *slot = Some(list);
  Ok(out)
}

fn off_list<T>(
  app: &AppHandle,
  edit: impl FnOnce(&mut OffList) -> (bool, T),
) -> Result<T, &'static str> {
  let mut slot = OFF.lock().map_err(|_| PREFERENCES_FAILED)?;
  let file = off_file(app).ok_or(PREFERENCES_FAILED)?;
  off_list_at(&mut slot, &file, edit)
}

/// The shell's list of switched-off sites. `legacy`: its old local list, moved in on first use.
#[tauri::command]
pub(crate) fn catch_signin_off_get(
  app: AppHandle,
  legacy: Option<Vec<String>>,
) -> Result<Vec<String>, &'static str> {
  let legacy = legacy.unwrap_or_default();
  off_list(&app, |list| {
    let changed = list.migrate(&legacy[..legacy.len().min(MAX_OFF * 2)]);
    (changed, list.sites().to_vec())
  })
}

#[tauri::command]
pub(crate) fn catch_signin_off_set(
  app: AppHandle,
  site: String,
  off: bool,
) -> Result<Vec<String>, &'static str> {
  off_list(&app, |list| (list.set(&site, off), list.sites().to_vec()))
}

/// A list that cannot be read counts as "off": nothing is copied on a guess.
pub(super) fn off_at(slot: &mut Option<OffList>, file: &Path, host: &str) -> bool {
  off_list_at(slot, file, |list| (false, list.off(host))).unwrap_or(true)
}

pub(super) fn switched_off(app: &AppHandle, host: &str) -> bool {
  let (Ok(mut slot), Some(file)) = (OFF.lock(), off_file(app)) else {
    return true;
  };
  off_at(&mut slot, &file, host)
}

/// (a) The Nezzel browser's saved profile.
async fn profile_cookies(app: &AppHandle, url: &Url) -> Vec<Cookie<'static>> {
  // macOS 14+ reads the saved store itself; the call exists on macOS only.
  #[cfg(target_os = "macos")]
  let store = native::use_persistent_profile();
  #[cfg(not(target_os = "macos"))]
  let store = false;
  let reader = profile_reader(store, window::profile_tabs(app));
  let read = match reader {
    ProfileReader::Store => native::profile_cookies(app).await,
    ProfileReader::Tab(label) => match window::page(app, &label) {
      Ok(page) => native::cookies(&page, url.clone()).await,
      Err(error) => Err(error),
    },
    ProfileReader::None => Ok(Vec::new()),
  };
  read.unwrap_or_default()
}

/// (b) The user's signed-in browser, as the Nezzel browser's sign-in switch reads it; only when
/// the main app's browser sign-in is on and reading shows no prompt.
async fn browser_cookies(app: &AppHandle, url: &Url, host: &str) -> Vec<Cookie<'static>> {
  if !crate::runners::login::automatic::enabled(app) || !export_matches_copy(host) {
    return Vec::new();
  }
  let settings = app.state::<crate::SharedConfig>().load().auth.clone();
  // Only browsers that read without a prompt are candidates, before one is picked.
  let auth = settings.clone();
  let quiet = move |kind| reads_quietly(SYSTEM_ASKS, kind, &auth);
  let Ok(profiles) = site_tools::signed_in_profiles(host, quiet).await else {
    return Vec::new();
  };
  let Ok(context) = CookieContext::for_app(app) else {
    return Vec::new();
  };
  site_tools::read_signin_profiles(profiles, &settings, url, &context, || {
    if !crate::runners::login::automatic::enabled(app) || switched_off(app, host) {
      Err("browserLoginDisabled")
    } else {
      Ok(())
    }
  })
  .await
  .map(|(_, cookies)| cookies)
  .unwrap_or_default()
}

/// What a copy reads and where it writes. The only write goes to the hidden page's own store:
/// the sources are read-only.
pub(super) trait Sources: Sync {
  fn off(&self, host: &str) -> bool;
  /// (a) The Nezzel browser's saved profile.
  fn profile(&self) -> BoxFuture<'_, Vec<Cookie<'static>>>;
  /// (b) The user's signed-in browser.
  fn browser(&self) -> BoxFuture<'_, Vec<Cookie<'static>>>;
  /// The hidden page's throwaway store.
  fn write(&self, cookies: Vec<Cookie<'static>>) -> BoxFuture<'_, ()>;
}

/// Reads both sign-ins of the link's site, merges them and writes them into the hidden page, all
/// within `limit`. A source that is missing, slow or refused adds nothing. Returns `false` only
/// when the write did not finish in time: cookies may still land, so the page is not used.
pub(super) async fn copy_with(
  sources: &impl Sources,
  host: &str,
  now: i64,
  limit: Duration,
) -> bool {
  if sources.off(host) {
    return true;
  }
  let start = tokio::time::Instant::now();
  let read_by = start + limit * READ_SHARE.0 / READ_SHARE.1;
  let (profile, browser) = tokio::join!(
    tokio::time::timeout_at(read_by, sources.profile()),
    tokio::time::timeout_at(read_by, sources.browser()),
  );
  let cookies = merge(
    site_only(profile.unwrap_or_default(), host, now),
    site_only(browser.unwrap_or_default(), host, now),
  );
  if cookies.is_empty() {
    return true;
  }
  tokio::time::timeout_at(start + limit, sources.write(cookies))
    .await
    .is_ok()
}

struct Live<'a> {
  app: &'a AppHandle,
  page: &'a Webview,
  url: &'a Url,
  host: &'a str,
}

impl Sources for Live<'_> {
  fn off(&self, host: &str) -> bool {
    switched_off(self.app, host)
  }

  fn profile(&self) -> BoxFuture<'_, Vec<Cookie<'static>>> {
    Box::pin(profile_cookies(self.app, self.url))
  }

  fn browser(&self) -> BoxFuture<'_, Vec<Cookie<'static>>> {
    Box::pin(browser_cookies(self.app, self.url, self.host))
  }

  fn write(&self, cookies: Vec<Cookie<'static>>) -> BoxFuture<'_, ()> {
    Box::pin(async move {
      let _ = native::set_cookies(self.page, cookies).await;
    })
  }
}

/// The host a copy is made for: none for a plain-http link, whose cookies would travel in the clear.
fn copy_host(url: &Url) -> Option<&str> {
  url.host_str().filter(|_| url.scheme() == "https")
}

/// Copies the link's site cookies into the hidden page's throwaway store, before its first load.
/// `false`: a write may still be landing, the page must not be used.
pub(super) async fn copy(app: &AppHandle, page: &Webview, url: &Url) -> bool {
  let Some(host) = copy_host(url) else {
    return true;
  };
  let live = Live {
    app,
    page,
    url,
    host,
  };
  copy_with(&live, host, site_tools::unix_now(), COPY_LIMIT).await
}

#[cfg(test)]
#[path = "hidden_signin_tests.rs"]
mod tests;
