//! The tab table of one Nezzel browser window (no webviews here) and the home page's lists.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::Url;
use tokio::time::Instant;

/// Tabs one window may hold (`catchTabLimit` past it).
pub(super) const MAX_TABS: usize = 6;
/// A tab this long in the background is put to sleep (webview dropped, address and title kept).
pub(super) const SLEEP_AFTER: Duration = Duration::from_secs(10 * 60);
pub(super) const MAX_RECENT_SITES: usize = 6;
pub(super) const MAX_RECENT_SAVES: usize = 3;
const MAX_TITLE_CHARS: usize = 200;

/// Where a tab keeps cookies and site data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Store {
  /// The persistent catch profile every normal tab shares (one sign-in).
  Shared,
  /// A throwaway store of one private tab (and the pop-ups it opened), forgotten on close.
  Private(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Tab {
  pub(super) id: String,
  pub(super) store: Store,
  /// The tab whose page opened this one (a pop-up the user caused).
  pub(super) opener: Option<String>,
  pub(super) asleep: bool,
  /// The last address ("" = the home page) and title: what a sleeping tab shows and reloads.
  pub(super) url: String,
  pub(super) title: Option<String>,
  background_since: Option<Instant>,
}

impl Tab {
  pub(super) fn private(&self) -> bool {
    matches!(self.store, Store::Private(_))
  }

  pub(super) fn home(&self) -> bool {
    self.url.is_empty()
  }
}

#[derive(Default)]
pub(super) struct NewTab {
  pub(super) url: String,
  pub(super) private: bool,
  /// A linked tab: shares this tab's store and sits right after it.
  pub(super) opener: Option<String>,
}

#[derive(Debug)]
pub(super) struct Closed {
  #[allow(dead_code)]
  pub(super) tab: Tab,
  /// The private store nobody uses any more: delete it.
  pub(super) drop_store: Option<String>,
  /// The closed tab was the last one: a fresh home tab took its place.
  #[cfg_attr(not(test), allow(dead_code))]
  pub(super) replaced: bool,
}

/// Where a link opened from outside the window goes (`Tabs::place_link`).
#[derive(Debug, PartialEq, Eq)]
pub(super) enum LinkPlace {
  /// A tab already shows this link: show that tab.
  Open(String),
  /// This home tab loads it.
  Home(String),
  /// A new tab.
  New,
  /// Six tabs and no home tab: ask which tab to close.
  Full,
}

pub(super) struct Tabs {
  tabs: Vec<Tab>,
  active: String,
  next: u32,
}

impl Tabs {
  /// A window starts with one tab (the home page when `url` is empty).
  pub(super) fn new(url: String) -> Self {
    let mut tabs = Self {
      tabs: Vec::new(),
      active: String::new(),
      next: 0,
    };
    let tab = tabs.make(NewTab {
      url,
      ..NewTab::default()
    });
    tabs.active = tab.id.clone();
    tabs.tabs.push(tab);
    tabs
  }

  fn make(&mut self, new: NewTab) -> Tab {
    self.next += 1;
    let id = format!("t{}", self.next);
    let store = match new.opener.as_deref().and_then(|opener| self.get(opener)) {
      Some(opener) => opener.store.clone(),
      None if new.private => Store::Private(id.clone()),
      None => Store::Shared,
    };
    Tab {
      opener: new.opener.filter(|opener| self.get(opener).is_some()),
      id,
      store,
      asleep: false,
      url: new.url,
      title: None,
      background_since: None,
    }
  }

  pub(super) fn iter(&self) -> impl Iterator<Item = &Tab> {
    self.tabs.iter()
  }

  pub(super) fn len(&self) -> usize {
    self.tabs.len()
  }

  pub(super) fn active_id(&self) -> &str {
    &self.active
  }

  pub(super) fn active(&self) -> &Tab {
    self
      .get(&self.active)
      .expect("the active tab is in the table")
  }

  pub(super) fn get(&self, id: &str) -> Option<&Tab> {
    self.tabs.iter().find(|tab| tab.id == id)
  }

  pub(super) fn get_mut(&mut self, id: &str) -> Option<&mut Tab> {
    self.tabs.iter_mut().find(|tab| tab.id == id)
  }

  /// The tab `by` places after the active one in the list, going round at both ends.
  pub(super) fn neighbour(&self, by: isize) -> &str {
    let count = self.tabs.len() as isize;
    let at = self
      .tabs
      .iter()
      .position(|tab| tab.id == self.active)
      .unwrap_or(0) as isize;
    &self.tabs[(at + by).rem_euclid(count) as usize].id
  }

  /// Where a link from the main window goes: the tab that already shows it, else the home tab
  /// in front, else a new tab; at six tabs any home tab, else nowhere. Without a link: a home
  /// tab. A private tab is never picked.
  pub(super) fn place_link(&self, url: Option<&Url>) -> LinkPlace {
    let usable = || self.tabs.iter().filter(|tab| !tab.private());
    let shows = |tab: &&Tab| url.is_some_and(|url| tab.url == url.as_str());
    if let Some(tab) = usable().find(shows) {
      return LinkPlace::Open(tab.id.clone());
    }
    let full = self.tabs.len() >= MAX_TABS;
    let homes = || usable().filter(|tab| tab.home());
    let home = homes()
      .find(|tab| tab.id == self.active)
      .or_else(|| homes().next().filter(|_| full || url.is_none()));
    match home {
      Some(tab) => LinkPlace::Home(tab.id.clone()),
      None if full => LinkPlace::Full,
      None => LinkPlace::New,
    }
  }

  /// Opens a tab and shows it. A linked tab goes after its opener and the tabs it opened before.
  pub(super) fn open(&mut self, new: NewTab, now: Instant) -> Result<String, &'static str> {
    let id = self.insert(new, None)?;
    self.show(&id, now);
    Ok(id)
  }

  /// A modified link leaves the current tab in front.
  #[cfg(target_os = "macos")]
  pub(super) fn open_background(
    &mut self,
    new: NewTab,
    now: Instant,
  ) -> Result<String, &'static str> {
    self.insert(new, Some(now))
  }

  fn insert(
    &mut self,
    new: NewTab,
    background_since: Option<Instant>,
  ) -> Result<String, &'static str> {
    if self.tabs.len() >= MAX_TABS {
      return Err("catchTabLimit");
    }
    let mut tab = self.make(new);
    tab.background_since = background_since;
    let id = tab.id.clone();
    let at = match tab.opener.as_deref() {
      Some(opener) => self
        .tabs
        .iter()
        .rposition(|t| t.id == opener || t.opener.as_deref() == Some(opener))
        .map_or(self.tabs.len(), |at| at + 1),
      None => self.tabs.len(),
    };
    self.tabs.insert(at, tab);
    Ok(id)
  }

  fn show(&mut self, id: &str, now: Instant) {
    if self.active == id {
      return;
    }
    let previous = std::mem::replace(&mut self.active, id.to_owned());
    if let Some(tab) = self.get_mut(&previous) {
      tab.background_since = Some(now);
    }
    if let Some(tab) = self.get_mut(id) {
      tab.background_since = None;
    }
  }

  /// Shows a tab. The caller wakes it when it was asleep (`wake`).
  pub(super) fn select(&mut self, id: &str, now: Instant) -> Result<&Tab, &'static str> {
    if self.get(id).is_none() {
      return Err("catchTabClosed");
    }
    self.show(id, now);
    Ok(self.active())
  }

  /// Closes a tab. The active tab hands over to its opener, else the tab after it, else the one
  /// before; the last tab hands over to a fresh home tab (the window stays).
  pub(super) fn close(&mut self, id: &str) -> Result<Closed, &'static str> {
    let at = self
      .tabs
      .iter()
      .position(|tab| tab.id == id)
      .ok_or("catchTabClosed")?;
    let tab = self.tabs.remove(at);
    for linked in &mut self.tabs {
      if linked.opener.as_deref() == Some(id) {
        linked.opener = None;
      }
    }
    let drop_store = match &tab.store {
      Store::Private(store) if !self.tabs.iter().any(|t| t.store == tab.store) => {
        Some(store.clone())
      }
      _ => None,
    };
    let replaced = self.tabs.is_empty();
    if replaced {
      let home = self.make(NewTab::default());
      self.active = home.id.clone();
      self.tabs.push(home);
    } else if self.active == id {
      let next = tab
        .opener
        .as_deref()
        .filter(|opener| self.get(opener).is_some())
        .map(str::to_owned)
        .unwrap_or_else(|| self.tabs[at.min(self.tabs.len() - 1)].id.clone());
      // The closed tab no longer counts as the previous one.
      self.active = next.clone();
      if let Some(tab) = self.get_mut(&next) {
        tab.background_since = None;
      }
    }
    Ok(Closed {
      tab,
      drop_store,
      replaced,
    })
  }

  /// Background tabs with a page that have waited long enough to be put to sleep.
  pub(super) fn sleepy(&self, now: Instant) -> Vec<String> {
    self
      .tabs
      .iter()
      .filter(|tab| {
        !tab.asleep
          && !tab.home()
          && tab.id != self.active
          && tab
            .background_since
            .is_some_and(|since| now.saturating_duration_since(since) >= SLEEP_AFTER)
      })
      .map(|tab| tab.id.clone())
      .collect()
  }

  /// A busy tab (playing, saving) stays awake: its background clock starts again.
  pub(super) fn keep_awake(&mut self, id: &str, now: Instant) {
    if let Some(tab) = self.get_mut(id) {
      if tab.background_since.is_some() {
        tab.background_since = Some(now);
      }
    }
  }

  pub(super) fn set_asleep(&mut self, id: &str, asleep: bool) {
    if let Some(tab) = self.get_mut(id) {
      tab.asleep = asleep;
    }
  }

  /// The address and title a tab shows (and keeps while asleep).
  pub(super) fn set_page(&mut self, id: &str, url: String, title: Option<String>) {
    if let Some(tab) = self.get_mut(id) {
      tab.url = url;
      tab.title = title;
    }
  }
}

/// A site on the home page: registrable domain and the last page title seen there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct RecentSite {
  pub(super) host: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub(super) title: Option<String>,
}

fn clean_title(title: Option<&str>) -> Option<String> {
  let title: String = title?
    .chars()
    .filter(|c| !c.is_control())
    .take(MAX_TITLE_CHARS)
    .collect();
  let title = title.trim();
  (!title.is_empty()).then(|| title.to_owned())
}

/// The sites the home page offers again, newest first. Private tabs are never recorded.
#[derive(Default, Debug)]
pub(super) struct RecentSites {
  sites: Vec<RecentSite>,
}

impl RecentSites {
  pub(super) fn list(&self) -> &[RecentSite] {
    &self.sites
  }

  /// Records a visited page; returns whether the list changed (and should be written).
  pub(super) fn record(&mut self, url: &Url, title: Option<&str>, private: bool) -> bool {
    if private {
      return false;
    }
    let Some(host) = crate::usage::keys::site_of(url.as_str()) else {
      return false;
    };
    let site = RecentSite {
      host,
      title: clean_title(title),
    };
    if self.sites.first() == Some(&site) {
      return false;
    }
    self.sites.retain(|known| known.host != site.host);
    self.sites.insert(0, site);
    self.sites.truncate(MAX_RECENT_SITES);
    true
  }

  /// Forgets one site (by its registrable domain); returns whether it was listed.
  pub(super) fn forget(&mut self, url: &Url) -> bool {
    let Some(host) = crate::usage::keys::site_of(url.as_str()) else {
      return false;
    };
    let before = self.sites.len();
    self.sites.retain(|known| known.host != host);
    self.sites.len() != before
  }

  pub(super) fn clear(&mut self) {
    self.sites.clear();
  }

  /// Reads the stored list; anything malformed is dropped, never trusted.
  pub(super) fn from_json(raw: &str) -> Self {
    let sites = serde_json::from_str::<Vec<RecentSite>>(raw).unwrap_or_default();
    let mut list = Self::default();
    for site in sites.into_iter().rev() {
      let host = site.host.to_ascii_lowercase();
      if crate::usage::keys::site_of(&format!("https://{host}/")).as_deref() == Some(host.as_str())
      {
        list.sites.retain(|known| known.host != host);
        list.sites.insert(
          0,
          RecentSite {
            host,
            title: clean_title(site.title.as_deref()),
          },
        );
      }
    }
    list.sites.truncate(MAX_RECENT_SITES);
    list
  }

  pub(super) fn to_json(&self) -> String {
    serde_json::to_string(&self.sites).unwrap_or_else(|_| "[]".into())
  }
}

/// A video saved from the Nezzel browser in this run of the app (the home page's last saves).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RecentSave {
  pub(super) download_id: String,
  pub(super) title: String,
  pub(super) host: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub(super) duration_sec: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub(super) size_bytes: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub(super) height: Option<u32>,
}

#[derive(Default, Debug)]
pub(super) struct RecentSaves {
  saves: Vec<RecentSave>,
}

impl RecentSaves {
  pub(super) fn record(&mut self, mut save: RecentSave, private: bool) {
    if private {
      return;
    }
    // A page names itself: keep it short and printable.
    save.title = clean_title(Some(&save.title)).unwrap_or_else(|| save.host.clone());
    self
      .saves
      .retain(|known| known.download_id != save.download_id);
    self.saves.insert(0, save);
    self.saves.truncate(MAX_RECENT_SAVES);
  }

  pub(super) fn list(&self) -> &[RecentSave] {
    &self.saves
  }
}

#[cfg(test)]
#[path = "tabs_tests.rs"]
mod tests;
