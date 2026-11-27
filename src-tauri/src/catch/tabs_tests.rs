use super::*;

fn ids(tabs: &Tabs) -> Vec<&str> {
  tabs.iter().map(|tab| tab.id.as_str()).collect()
}

fn page(url: &str) -> NewTab {
  NewTab {
    url: url.into(),
    ..NewTab::default()
  }
}

#[tokio::test(start_paused = true)]
async fn new_select_close_keep_order_and_pick_the_next_active_tab() {
  let now = Instant::now();
  let mut tabs = Tabs::new(String::new());
  assert!(tabs.active().home());
  let b = tabs.open(page("https://b.example/"), now).unwrap();
  let c = tabs.open(page("https://c.example/"), now).unwrap();
  assert_eq!(ids(&tabs), ["t1", "t2", "t3"]);
  assert_eq!(tabs.active_id(), c);
  tabs.select(&b, now).unwrap();
  // The active tab hands over to the tab after it.
  tabs.close(&b).unwrap();
  assert_eq!(tabs.active_id(), c);
  // ... else the one before it.
  tabs.close(&c).unwrap();
  assert_eq!(tabs.active_id(), "t1");
  // A background tab closing leaves the active tab alone.
  let d = tabs.open(page("https://d.example/"), now).unwrap();
  tabs.select("t1", now).unwrap();
  tabs.close(&d).unwrap();
  assert_eq!(tabs.active_id(), "t1");
  assert_eq!(tabs.select("t9", now).unwrap_err(), "catchTabClosed");
  // The last tab hands over to a fresh home tab: the window stays.
  let closed = tabs.close("t1").unwrap();
  assert!(closed.replaced);
  assert_eq!(tabs.len(), 1);
  assert!(tabs.active().home());
  assert_ne!(tabs.active_id(), "t1");
}

#[tokio::test(start_paused = true)]
async fn the_seventh_tab_is_refused() {
  let now = Instant::now();
  let mut tabs = Tabs::new(String::new());
  for _ in 1..MAX_TABS {
    tabs.open(NewTab::default(), now).unwrap();
  }
  assert_eq!(tabs.len(), 6);
  assert_eq!(
    tabs.open(NewTab::default(), now).unwrap_err(),
    "catchTabLimit"
  );
  assert_eq!(tabs.len(), 6);
}

#[tokio::test(start_paused = true)]
async fn a_background_tab_sleeps_after_ten_minutes_and_wakes_on_select() {
  let mut tabs = Tabs::new("https://a.example/".into());
  let b = tabs
    .open(page("https://b.example/"), Instant::now())
    .unwrap();
  let home = tabs.open(NewTab::default(), Instant::now()).unwrap();
  tokio::time::advance(SLEEP_AFTER - Duration::from_secs(1)).await;
  assert!(tabs.sleepy(Instant::now()).is_empty());
  // Playing in the background restarts its clock.
  tabs.keep_awake(&b, Instant::now());
  tokio::time::advance(Duration::from_secs(1)).await;
  assert_eq!(tabs.sleepy(Instant::now()), ["t1"]);
  tokio::time::advance(SLEEP_AFTER).await;
  // The home tab has no page to drop; the active tab never sleeps.
  assert_eq!(tabs.sleepy(Instant::now()), ["t1", b.as_str()]);
  assert!(!tabs.sleepy(Instant::now()).contains(&home));
  tabs.set_asleep("t1", true);
  let shown = tabs.select("t1", Instant::now()).unwrap();
  assert!(shown.asleep, "the caller wakes it");
  assert_eq!(shown.url, "https://a.example/");
  tabs.set_asleep("t1", false);
  assert_eq!(tabs.sleepy(Instant::now()), [b.as_str()]);
}

#[tokio::test(start_paused = true)]
async fn a_linked_tab_sits_after_its_opener_and_hands_back_to_it_on_close() {
  let now = Instant::now();
  let mut tabs = Tabs::new("https://site.example/".into());
  let other = tabs.open(page("https://other.example/"), now).unwrap();
  tabs.select("t1", now).unwrap();
  let signin = tabs
    .open(
      NewTab {
        url: "https://accounts.example/".into(),
        opener: Some("t1".into()),
        ..NewTab::default()
      },
      now,
    )
    .unwrap();
  assert_eq!(ids(&tabs), ["t1", signin.as_str(), other.as_str()]);
  assert_eq!(tabs.get(&signin).unwrap().opener.as_deref(), Some("t1"));
  tabs.close(&signin).unwrap();
  assert_eq!(tabs.active_id(), "t1");
}

#[tokio::test(start_paused = true)]
async fn private_tabs_get_their_own_store_which_is_dropped_with_its_last_tab() {
  let now = Instant::now();
  let mut tabs = Tabs::new(String::new());
  let private = |opener: Option<String>| NewTab {
    url: "https://a.example/".into(),
    private: true,
    opener,
  };
  let one = tabs.open(private(None), now).unwrap();
  let two = tabs.open(private(None), now).unwrap();
  let store = |tabs: &Tabs, id: &str| tabs.get(id).unwrap().store.clone();
  assert_eq!(store(&tabs, "t1"), Store::Shared);
  assert!(matches!(store(&tabs, &one), Store::Private(_)));
  assert_ne!(store(&tabs, &one), store(&tabs, &two));
  // A pop-up from a private tab shares its opener's store (window.opener needs it).
  let popup = tabs.open(private(Some(one.clone())), now).unwrap();
  assert_eq!(store(&tabs, &popup), store(&tabs, &one));
  assert_eq!(tabs.close(&one).unwrap().drop_store, None);
  let Store::Private(kept) = store(&tabs, &popup) else {
    panic!("private")
  };
  assert_eq!(tabs.close(&popup).unwrap().drop_store, Some(kept));
  assert_eq!(tabs.close("t1").unwrap().drop_store, None);
}

#[test]
fn recent_sites_cap_dedupe_by_domain_skip_private_and_clear() {
  let mut recent = RecentSites::default();
  let url = |raw: &str| Url::parse(raw).unwrap();
  assert!(recent.record(&url("https://www.a.com/x?id=1"), Some("A one"), false));
  assert!(recent.record(&url("https://b.com/"), None, false));
  // Same registrable domain: one entry, moved to the front with the newer title.
  assert!(recent.record(&url("https://m.a.com/y"), Some("A two\n"), false));
  assert_eq!(
    recent.list(),
    [
      RecentSite {
        host: "a.com".into(),
        title: Some("A two".into())
      },
      RecentSite {
        host: "b.com".into(),
        title: None
      },
    ]
  );
  assert!(!recent.record(&url("https://m.a.com/z"), Some("A two"), false));
  assert!(!recent.record(&url("https://private.com/"), None, true));
  for n in 0..10 {
    recent.record(&url(&format!("https://s{n}.com/")), None, false);
  }
  assert_eq!(recent.list().len(), MAX_RECENT_SITES);
  assert_eq!(recent.list()[0].host, "s9.com");
  // The stored file round-trips; anything that is not a plain domain is dropped.
  let stored = RecentSites::from_json(&recent.to_json());
  assert_eq!(stored.list(), recent.list());
  let odd = RecentSites::from_json(r#"[{"host":"https://x.com/path"},{"host":"ok.com"}]"#);
  assert_eq!(odd.list().len(), 1);
  assert!(recent.forget(&url("https://www.s9.com/a")));
  assert_eq!(recent.list().len(), MAX_RECENT_SITES - 1);
  recent.clear();
  assert!(recent.list().is_empty());
}

#[test]
fn recent_saves_keep_the_last_three_outside_private_tabs() {
  let save = |id: &str| RecentSave {
    download_id: id.into(),
    title: "T".into(),
    host: "a.com".into(),
    duration_sec: None,
    size_bytes: None,
    height: None,
  };
  let mut saves = RecentSaves::default();
  for id in ["1", "2", "3", "4"] {
    saves.record(save(id), false);
  }
  saves.record(save("5"), true);
  let ids: Vec<_> = saves
    .list()
    .iter()
    .map(|s| s.download_id.as_str())
    .collect();
  assert_eq!(ids, ["4", "3", "2"]);
}

#[test]
fn the_last_three_saves_are_kept_and_a_private_tabs_save_never_is() {
  let save = |id: &str, title: &str| RecentSave {
    download_id: id.into(),
    title: title.into(),
    host: "a.com".into(),
    duration_sec: None,
    size_bytes: None,
    height: None,
  };
  let mut saves = RecentSaves::default();
  saves.record(save("secret", "Private"), true);
  assert!(saves.list().is_empty());
  for n in 1..=4 {
    saves.record(save(&format!("d{n}"), "A\u{7}title\n"), false);
  }
  saves.record(save("d3", ""), false);
  let ids: Vec<_> = saves
    .list()
    .iter()
    .map(|save| save.download_id.as_str())
    .collect();
  assert_eq!(ids, ["d3", "d4", "d2"]);
  assert_eq!(
    saves.list()[0].title,
    "a.com",
    "no title: the site names it"
  );
  assert_eq!(saves.list()[1].title, "Atitle");
}

#[test]
fn the_next_and_previous_tab_go_round_the_list() {
  let now = Instant::now();
  let mut tabs = Tabs::new(String::new());
  let first = tabs.active_id().to_owned();
  assert_eq!(tabs.neighbour(1), first, "one tab: itself");
  let second = tabs.open(NewTab::default(), now).unwrap();
  let third = tabs.open(NewTab::default(), now).unwrap();
  assert_eq!((tabs.neighbour(0), tabs.neighbour(1)), (&*third, &*first));
  assert_eq!(tabs.neighbour(-1), second);
  tabs.select(&first, now).unwrap();
  assert_eq!(tabs.neighbour(-1), third);
}

#[tokio::test]
async fn a_link_from_outside_goes_to_its_own_tab_a_home_tab_or_a_new_tab_never_a_private_one() {
  let now = Instant::now();
  let link = |url: &str| Url::parse(url).unwrap();
  let private = |url: &str| NewTab {
    private: true,
    ..page(url)
  };
  // A window showing only the home page loads the link there.
  let mut tabs = Tabs::new(String::new());
  let home = tabs.active_id().to_owned();
  let a = link("https://a.test/watch");
  assert_eq!(tabs.place_link(Some(&a)), LinkPlace::Home(home.clone()));
  assert_eq!(tabs.place_link(None), LinkPlace::Home(home.clone()));

  // A home tab behind another tab is left alone while there is room.
  let shown = tabs.open(page(a.as_str()), now).unwrap();
  let b = link("https://b.test/");
  assert_eq!(tabs.place_link(Some(&b)), LinkPlace::New);
  assert_eq!(tabs.place_link(None), LinkPlace::Home(home.clone()));
  // The link a tab already shows selects that tab, even from the background.
  tabs.open(page(b.as_str()), now).unwrap();
  assert_eq!(tabs.place_link(Some(&a)), LinkPlace::Open(shown));

  // Private tabs: neither the one showing the link nor a private home tab is picked.
  let c = link("https://c.test/");
  tabs.open(private(c.as_str()), now).unwrap();
  tabs.open(private(""), now).unwrap();
  assert_eq!(tabs.place_link(Some(&c)), LinkPlace::New);

  // Six tabs: the home tab takes the link; without one the caller asks which tab to close.
  tabs.open(page("https://d.test/"), now).unwrap();
  assert_eq!(tabs.len(), MAX_TABS);
  let e = link("https://e.test/");
  assert_eq!(tabs.place_link(Some(&e)), LinkPlace::Home(home.clone()));
  tabs.set_page(&home, "https://f.test/".into(), None);
  assert_eq!(tabs.place_link(Some(&e)), LinkPlace::Full);
  assert_eq!(tabs.place_link(None), LinkPlace::Full);
  assert!(matches!(tabs.place_link(Some(&c)), LinkPlace::Full));
}
