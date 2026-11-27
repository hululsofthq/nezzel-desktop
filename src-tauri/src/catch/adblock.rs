//! Ad and tracker blocking for every Nezzel browser page: tabs, private tabs, pop-ups and the
//! hidden automatic-catch page. One bundled list (`filters/network.txt`, refreshed by
//! `scripts/fetch-filters.mjs`): the Rust matcher answers each request on Windows; WebKit (macOS,
//! Linux) gets the same list turned into content-blocker rules. Network rules only (no page
//! hiding). The page's own document and anything a video plays are never blocked.

#[cfg(any(not(windows), test))]
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[cfg(any(not(windows), test))]
use adblock::content_blocking::{CbResourceType, CbRule, CbTrigger, CbType};
use adblock::lists::{FilterSet, ParseOptions};
use adblock::request::Request;
use adblock::Engine;
#[cfg(not(windows))]
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager, Url};

#[path = "adblock_platform.rs"]
mod platform;

pub(super) use platform::{attach, deciding, navigate, navigating, prepare, switched};

const LIST: &str = include_str!("filters/network.txt");
// WebKit (macOS, Linux) items below are gated `cfg(any(not(windows), test))`: Windows uses the
// Rust matcher only.
/// WebKit refuses a content-blocker list with more rules (WebCore `ContentExtensionParser`).
#[cfg(any(not(windows), test))]
const MAX_WEBKIT_RULES: usize = 150_000;
/// Addresses of what a player loads: manifests, segments, media files, captions, keys.
const MEDIA_EXTENSIONS: [&str; 15] = [
  "m3u8", "m3u", "mpd", "m4s", "ts", "mp4", "m4v", "m4a", "webm", "mp3", "aac", "ogg", "vtt",
  "srt", "key",
];
/// The most sites the user may switch blocking off for; the oldest is forgotten first.
const MAX_OFF: usize = 200;
const OFF_FILE: &str = "catch-adblock-off.json";

/// What a request loads, as each engine names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))] // WebView2 names each kind.
pub(super) enum Kind {
  /// A frame's document, the page's or an embedded one: never blocked (as on WebKit, where
  /// the rules never block a document).
  Document,
  Script,
  Image,
  Style,
  Font,
  Media,
  Fetch,
  Other,
}

impl Kind {
  #[cfg_attr(not(windows), allow(dead_code))]
  fn name(self) -> &'static str {
    match self {
      Kind::Document => "document",
      Kind::Script => "script",
      Kind::Image => "image",
      Kind::Style => "stylesheet",
      Kind::Font => "font",
      Kind::Media => "media",
      Kind::Fetch => "xmlhttprequest",
      Kind::Other => "other",
    }
  }
}

/// One matcher for the whole run, built once and never changed: requests only read it.
#[cfg_attr(not(windows), allow(dead_code))] // The Rust matcher answers on Windows only.
static ENGINE: OnceLock<Engine> = OnceLock::new();

/// Builds the matcher (a second or two; off the UI thread) or returns it.
#[cfg_attr(not(windows), allow(dead_code))]
fn engine() -> &'static Engine {
  ENGINE.get_or_init(|| matcher(LIST))
}

#[cfg_attr(not(windows), allow(dead_code))]
fn matcher(list: &str) -> Engine {
  let mut set = FilterSet::new(false);
  set.add_filter_list(list.to_owned(), ParseOptions::default());
  Engine::new_with_filter_set(set)
}

/// Whether `url`, loaded by the page at `page` as `kind`, is an ad or a tracker to block.
/// The page's document and anything a video plays are never blocked. While the matcher is still
/// being built, nothing is (a request never waits for it on the UI thread).
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn blocks(url: &str, page: &str, kind: Kind) -> bool {
  blocks_once_built(&ENGINE, url, page, kind)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn blocks_once_built(engine: &OnceLock<Engine>, url: &str, page: &str, kind: Kind) -> bool {
  engine
    .get()
    .is_some_and(|engine| blocks_with(engine, url, page, kind))
}

#[cfg_attr(not(windows), allow(dead_code))]
fn blocks_with(engine: &Engine, url: &str, page: &str, kind: Kind) -> bool {
  if matches!(kind, Kind::Document | Kind::Media) || plays(url) {
    return false;
  }
  let Ok(request) = Request::new(url, page, kind.name(), "GET") else {
    return false;
  };
  engine.check_network_request(&request).should_block()
}

/// A manifest, segment or media file by its address (the request type is not always known).
#[cfg_attr(not(windows), allow(dead_code))]
fn plays(url: &str) -> bool {
  let Ok(url) = Url::parse(url) else {
    return false;
  };
  let name = url.path().rsplit('/').next().unwrap_or_default();
  name
    .rsplit_once('.')
    .is_some_and(|(_, ext)| MEDIA_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// The list as WebKit content-blocker JSON, in parts WebKit accepts. Each part carries every
/// exception, so an exception still lifts a block that landed in another part.
#[cfg(any(not(windows), test))]
pub(super) fn webkit_rules() -> Result<Vec<String>, &'static str> {
  let rules = webkit_rule_list(LIST)?;
  // The Rust matcher keeps these; WebKit pages go without them.
  tracing::info!(
    dropped = rules.dropped,
    "ad list filters WebKit cannot express"
  );
  webkit_parts(rules.rules)
}

/// What the WebKit form of a list keeps: its rules, and how many of the list's network filters
/// WebKit cannot express (`into_content_blocking` drops them; the Rust matcher still has them).
#[cfg(any(not(windows), test))]
pub(super) struct WebkitRules {
  pub(super) rules: Vec<CbRule>,
  pub(super) dropped: usize,
}

#[cfg(any(not(windows), test))]
pub(super) fn webkit_rule_list(list: &str) -> Result<WebkitRules, &'static str> {
  let mut set = FilterSet::new(true);
  set.add_filter_list(list.to_owned(), ParseOptions::default());
  let (rules, used) = set
    .into_content_blocking()
    .map_err(|()| "catchAdblockList")?;
  let filters = list
    .lines()
    .filter(|line| !line.is_empty() && !line.starts_with('!') && !line.starts_with('['))
    .count();
  Ok(WebkitRules {
    rules: rules.into_iter().filter_map(webkit_rule).collect(),
    dropped: filters.saturating_sub(used.len()),
  })
}

#[cfg(any(not(windows), test))]
fn webkit_parts(rules: Vec<CbRule>) -> Result<Vec<String>, &'static str> {
  let (mut blocks, mut exceptions): (Vec<_>, Vec<_>) = rules
    .into_iter()
    .partition(|rule| rule.action.typ != CbType::IgnorePreviousRules);
  exceptions.extend(never_blocked());
  let room = MAX_WEBKIT_RULES - exceptions.len();
  let mut parts = Vec::new();
  while !blocks.is_empty() {
    let rest = blocks.split_off(room.min(blocks.len()));
    let mut part = std::mem::replace(&mut blocks, rest);
    part.extend(exceptions.iter().cloned());
    parts.push(serde_json::to_string(&part).map_err(|_| "catchAdblockList")?);
  }
  Ok(parts)
}

/// Keeps network blocks and exceptions; a rule that could block the page itself loses that type.
#[cfg(any(not(windows), test))]
fn webkit_rule(mut rule: CbRule) -> Option<CbRule> {
  if !matches!(rule.action.typ, CbType::Block | CbType::IgnorePreviousRules) {
    return None;
  }
  if rule.action.typ == CbType::Block {
    let types = rule.trigger.resource_type.get_or_insert_with(|| {
      HashSet::from([
        CbResourceType::Image,
        CbResourceType::StyleSheet,
        CbResourceType::Script,
        CbResourceType::Font,
        CbResourceType::Raw,
        CbResourceType::SvgDocument,
        CbResourceType::Popup,
      ])
    });
    types.remove(&CbResourceType::Document);
    types.remove(&CbResourceType::Media);
    if types.is_empty() {
      return None;
    }
  }
  Some(rule)
}

/// Exceptions after every block: anything a video plays loads.
#[cfg(any(not(windows), test))]
fn never_blocked() -> Vec<CbRule> {
  let ignore = |trigger: CbTrigger| CbRule {
    action: adblock::content_blocking::CbAction {
      typ: CbType::IgnorePreviousRules,
      selector: None,
    },
    trigger,
  };
  let media = ignore(CbTrigger {
    url_filter: ".*".to_owned(),
    resource_type: Some(HashSet::from([CbResourceType::Media])),
    ..Default::default()
  });
  let files = MEDIA_EXTENSIONS.iter().map(|ext| {
    ignore(CbTrigger {
      url_filter: format!(r"^[^?#]*\.{ext}([?#].*)?$"),
      ..Default::default()
    })
  });
  std::iter::once(media).chain(files).collect()
}

#[cfg(not(windows))]
fn list_hash() -> String {
  let digest = Sha256::digest(LIST.as_bytes());
  digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(not(windows))]
const ID_PREFIX: &str = "nezzel-ads-";

/// The name WebKit keeps a compiled part under (`part` of `total`): a new list gets new names.
#[cfg(not(windows))]
pub(super) fn list_id(part: usize, total: usize) -> String {
  format!("{ID_PREFIX}{}-{part}-{total}", list_hash())
}

/// How many parts of this list WebKit already keeps compiled, when it keeps all of them.
#[cfg(not(windows))]
pub(super) fn kept_parts(ids: &[String]) -> Option<usize> {
  let ours = format!("{ID_PREFIX}{}-", list_hash());
  let total = ids
    .iter()
    .filter_map(|id| {
      id.strip_prefix(&ours)?
        .rsplit_once('-')?
        .1
        .parse::<usize>()
        .ok()
    })
    .next()?;
  (total > 0 && (0..total).all(|part| ids.contains(&list_id(part, total)))).then_some(total)
}

/// Compiled lists of an earlier list (an older release): WebKit's store drops them.
#[cfg(not(windows))]
pub(super) fn old_ids(ids: &[String]) -> Vec<String> {
  let ours = format!("{ID_PREFIX}{}-", list_hash());
  ids
    .iter()
    .filter(|id| id.starts_with(ID_PREFIX) && !id.starts_with(&ours))
    .cloned()
    .collect()
}

/// Sites the user switched blocking off for (registrable domains), kept on this device only.
#[derive(Clone, Default)]
pub(super) struct OffSites {
  sites: Vec<String>,
}

impl OffSites {
  pub(super) fn from_json(text: &str) -> Self {
    let mut sites: Vec<String> = serde_json::from_str(text).unwrap_or_default();
    sites.retain(|site| !site.is_empty());
    let extra = sites.len().saturating_sub(MAX_OFF);
    sites.drain(..extra);
    Self { sites }
  }

  pub(super) fn to_json(&self) -> String {
    serde_json::to_string(&self.sites).unwrap_or_else(|_| "[]".to_owned())
  }

  /// Blocking is on for every site the user did not switch off (and for pages without a site).
  pub(super) fn on(&self, url: &Url) -> bool {
    super::auto::site_of(url).is_none_or(|site| !self.sites.contains(&site))
  }

  /// Returns whether anything changed.
  pub(super) fn set(&mut self, url: &Url, on: bool) -> bool {
    let Some(site) = super::auto::site_of(url) else {
      return false;
    };
    let had = self.sites.contains(&site);
    if on == !had {
      return false;
    }
    self.sites.retain(|known| *known != site);
    if !on {
      self.sites.push(site);
      let extra = self.sites.len().saturating_sub(MAX_OFF);
      self.sites.drain(..extra);
    }
    true
  }
}

fn off_sites() -> &'static Mutex<Option<OffSites>> {
  static OFF: Mutex<Option<OffSites>> = Mutex::new(None);
  &OFF
}

fn off_path(app: &AppHandle) -> Option<PathBuf> {
  app.path().app_data_dir().ok().map(|dir| dir.join(OFF_FILE))
}

/// Reads the switched-off sites once (the first page of the run). A file that cannot be read
/// (other than missing) is not taken as empty: it is tried again, and nothing overwrites it.
pub(super) fn load(app: &AppHandle) {
  load_with(off_sites(), || {
    off_path(app).and_then(|path| read_off(&path))
  });
}

/// Fills `off` from `read` if it is empty. The file is read without holding the lock: requests
/// ask the list on the UI thread (Windows), and a page is made on the main thread.
pub(super) fn load_with(off: &Mutex<Option<OffSites>>, read: impl FnOnce() -> Option<OffSites>) {
  if off.lock().map_or(true, |off| off.is_some()) {
    return;
  }
  let Some(loaded) = read() else {
    return;
  };
  if let Ok(mut off) = off.lock() {
    off.get_or_insert(loaded);
  }
}

/// The kept list; `None` when the file is there but cannot be read.
pub(super) fn read_off(path: &std::path::Path) -> Option<OffSites> {
  match std::fs::read_to_string(path) {
    Ok(text) => Some(OffSites::from_json(&text)),
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(OffSites::default()),
    Err(error) => {
      tracing::warn!(kind = ?error.kind(), "ad-block site list unreadable");
      None
    }
  }
}

/// Replaces the file whole (a sibling file renamed over it), so a crash never leaves half a list.
pub(super) fn write_off(path: &std::path::Path, off: &OffSites) -> std::io::Result<()> {
  if let Some(dir) = path.parent() {
    std::fs::create_dir_all(dir)?;
  }
  let temporary = path.with_extension("json.tmp");
  let written =
    std::fs::write(&temporary, off.to_json()).and_then(|()| std::fs::rename(&temporary, path));
  if written.is_err() {
    let _ = std::fs::remove_file(&temporary);
  }
  written
}

/// Whether blocking is on for this page's site (on until the user switches the site off).
pub(super) fn on(url: &Url) -> bool {
  off_sites()
    .lock()
    .ok()
    .and_then(|off| off.as_ref().map(|off| off.on(url)))
    .unwrap_or(true)
}

/// Switches blocking for the page's site and keeps the choice. Returns whether it changed; the
/// switch changes only once the choice is on disk.
pub(super) fn set(app: &AppHandle, url: &Url, on: bool) -> Result<bool, &'static str> {
  static SAVING: Mutex<()> = Mutex::new(());
  let _saving = SAVING.lock().map_err(|_| "catchAdblockSave")?;
  load(app);
  // Requests read the list on the UI thread: it is not held while the file is written.
  let mut next = off_sites()
    .lock()
    .ok()
    .and_then(|off| off.clone())
    .ok_or("catchAdblockSave")?;
  if !next.set(url, on) {
    return Ok(false);
  }
  let path = off_path(app).ok_or("catchAdblockSave")?;
  write_off(&path, &next).map_err(|error| {
    tracing::warn!(kind = ?error.kind(), "ad-block site list not saved");
    "catchAdblockSave"
  })?;
  *off_sites().lock().map_err(|_| "catchAdblockSave")? = Some(next);
  Ok(true)
}

/// The "Block ads" switch of the side list's "This site" footer: on or off for the tab's site,
/// kept on this device, and the tab loads again with it. Not for a private tab (it would keep the site).
#[tauri::command]
pub(crate) async fn catch_adblock_set(
  app: AppHandle,
  caller: tauri::Webview,
  window_id: String,
  on: bool,
  tab_id: Option<String>,
) -> Result<(), &'static str> {
  if !super::commands::owns_window(caller.label(), &window_id) {
    return Err("catchWrongWindow");
  }
  let tab = app
    .state::<super::window::CatchWindows>()
    .tab(&window_id, tab_id.as_deref())?;
  // A private tab leaves nothing behind: no site kept on disk, nothing counted.
  if tab.private {
    return Err("catchAdblockPrivate");
  }
  let page = super::window::page(&app, &tab.label)?;
  let Some(url) = super::native::url(&page)
    .await
    .filter(super::state::is_http)
  else {
    return Ok(());
  };
  if !set(&app, &url, on)? {
    return Ok(());
  }
  if !on {
    crate::usage::count_catch(&app, crate::usage::keys::CATCH_ADBLOCK_OFF, &url);
  }
  switched(&page, on).await;
  super::window::notify(&app, &tab.label);
  page.reload().map_err(|_| "catchNavigationFailed")
}

#[cfg(test)]
#[path = "adblock_tests.rs"]
mod tests;
