use super::*;

#[test]
fn capture_script_passes_document_bound_options_without_globals() {
  for (autoplay, argument) in [(false, "undefined"), (true, "document")] {
    let script = capture_script(autoplay);
    assert!(!script.contains("globalThis.__nezzelCatch"));
    assert!(script.starts_with("(function(relayKey,autoplayDocument){"));
    assert!(script.contains("})({document,key:["));
    assert!(script.ends_with(&format!("n]}},{argument});")));
  }
}

#[test]
fn only_modified_main_frame_web_links_open_background_tabs() {
  for (frame, new_window, link, command, button, address, expected) in [
    (true, false, true, true, 0, "https://example.com", true),
    (true, false, true, false, 4, "http://example.com", true),
    (true, false, true, false, 0, "https://example.com", false),
    (true, false, true, false, 1, "https://example.com", false),
    (false, false, true, true, 0, "https://example.com", false),
    (true, false, false, true, 0, "https://example.com", false),
    (true, false, true, true, 0, "mailto:test@example.com", false),
    // target=_blank (no target frame), also from inside a frame: Cmd / middle go behind.
    (false, true, true, true, 0, "https://example.com", true),
    (false, true, true, false, 4, "https://example.com", true),
    // buttonNumber is a mask: 2 is the right button, not the middle one.
    (false, true, true, false, 2, "https://example.com", false),
    // A page's window.open during a Cmd-click (navigation type Other).
    (false, true, false, true, 0, "https://example.com", true),
    // A plain new-tab click stays in front.
    (false, true, true, false, 0, "https://example.com", false),
    (false, true, true, true, 0, "mailto:test@example.com", false),
  ] {
    assert_eq!(
      opens_background_tab(
        frame,
        new_window,
        link,
        command,
        button,
        &Url::parse(address).unwrap()
      ),
      expected,
      "{frame} {new_window} {link} {command} {button} {address}"
    );
  }
}

#[test]
fn mac_load_failure_ignores_policy_cancels() {
  for (code, expected) in [
    (-999, false),
    (102, false),
    (204, false),  // A direct media link plays in the page.
    (-1003, true), // Host not found.
    (-1004, true), // Connection refused.
    (-1200, true), // TLS failure.
  ] {
    assert_eq!(show_mac_load_failure(code), expected, "error {code}");
  }
}

#[test]
fn windows_load_failure_ignores_downloads_and_cancels() {
  for (code, expected) in [
    (0, false),
    (9, false), // Connection aborted: a navigation that became a download, or window.stop().
    (14, false), // Operation canceled.
    (17, false),
    (18, false),
    (7, true),  // Timeout.
    (12, true), // Cannot connect.
    (13, true), // Host name not resolved.
    (5, true),  // Invalid certificate.
  ] {
    assert_eq!(show_windows_load_failure(code), expected, "status {code}");
  }
}

#[test]
fn browser_window_uses_the_resolved_surface_colour() {
  use tauri::Theme::{Dark, Light};
  for (setting, system, rgba) in [
    ("system", Light, (250, 252, 251, 255)),
    ("system", Dark, (10, 15, 14, 255)),
    ("light", Dark, (250, 252, 251, 255)),
    ("dark", Light, (10, 15, 14, 255)),
  ] {
    let Color(r, g, b, a) = browser_surface(setting, system);
    assert_eq!((r, g, b, a), rgba, "{setting} {system:?}");
  }
}

#[tokio::test]
async fn close_removes_session_aborts_drain_and_discards_page_state() {
  let windows = CatchWindows::default();
  let session = Arc::new(Session::new(
    Url::parse("https://example.com/watch").unwrap(),
  ));
  let drain = tokio::spawn(std::future::pending::<()>());
  {
    let mut state = session.active().unwrap();
    state.drain = Some(drain.abort_handle());
    state.page.document.saved = Some("download-1".into());
  }
  windows
    .sessions
    .lock()
    .unwrap()
    .insert("catch-id".into(), session.clone());
  let removed = windows.remove("catch-id").unwrap();
  assert!(removed.stop().is_empty());
  assert!(windows.session("catch-id").is_err());
  assert!(windows.remove("catch-id").is_none());
  assert!(drain.await.unwrap_err().is_cancelled());
  let state = session.state.lock().unwrap();
  assert!(state.closed);
  assert_eq!(state.page.url, blank());
  assert!(state.page.document.saved.is_none());
}

struct FakeManifest(&'static str);

impl ManifestFetcher for FakeManifest {
  fn fetch(&self, url: Url) -> BoxFuture<'_, Option<(Url, String)>> {
    assert_eq!(url.as_str(), "https://cdn.example.org/stream.m3u8");
    Box::pin(async { Some((url, self.0.to_owned())) })
  }
}

#[tokio::test]
async fn manifest_inspection_uses_the_configured_fetcher() {
  let url = Url::parse("https://cdn.example.org/stream.m3u8").unwrap();
  let manifest = FakeManifest(include_str!("fixtures/short-aes128.m3u8"));
  let info = inspect_manifest(&manifest, url).await.unwrap();
  assert!(info.is_other_video());
  assert!(!info.has_drm && !info.is_live);
}

#[tokio::test]
async fn a_master_playlist_takes_the_length_of_its_first_variant() {
  struct Playlists(&'static str);
  impl ManifestFetcher for Playlists {
    fn fetch(&self, url: Url) -> BoxFuture<'_, Option<(Url, String)>> {
      let text = match url.as_str() {
        "https://cdn.example.org/film/master.m3u8" => Some(include_str!("fixtures/master.m3u8")),
        // The first variant, relative to the master; no other address is asked for.
        "https://cdn.example.org/film/360/playlist.m3u8?token=fake" => Some(self.0),
        other => panic!("unexpected fetch {other}"),
      };
      Box::pin(async move { text.map(|text| (url, text.to_owned())) })
    }
  }
  let master = Url::parse("https://cdn.example.org/film/master.m3u8").unwrap();
  let vod = include_str!("fixtures/short-aes128.m3u8");
  let info = inspect_manifest(&Playlists(vod), master.clone())
    .await
    .unwrap();
  assert_eq!(info.duration_seconds, Some(50.25));
  assert_eq!(info.variants.len(), 2, "the master's own variants stay");
  assert!(!info.has_drm && !info.is_live);
  // A live variant gives no length.
  let live = "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6,\nseg.ts\n";
  let info = inspect_manifest(&Playlists(live), master).await.unwrap();
  assert_eq!(info.duration_seconds, None);
  assert!(info.is_live, "a master whose variant is live is live");
}

#[tokio::test]
async fn a_redirected_master_resolves_its_variants_from_where_it_answered() {
  struct Redirected(std::sync::Mutex<Vec<String>>);
  impl ManifestFetcher for Redirected {
    fn fetch(&self, url: Url) -> BoxFuture<'_, Option<(Url, String)>> {
      self.0.lock().unwrap().push(url.to_string());
      let answer = match url.as_str() {
        // The page's link redirects to the CDN, as `handoff::fetch_manifest` follows it.
        "https://films.example.org/play/master.m3u8" => Some((
          Url::parse("https://cdn.example.net/film/master.m3u8").unwrap(),
          include_str!("fixtures/master.m3u8"),
        )),
        "https://cdn.example.net/film/360/playlist.m3u8?token=fake" => {
          Some((url, include_str!("fixtures/short-aes128.m3u8")))
        }
        // The variant's token never goes to the address that redirected.
        _ => None,
      };
      Box::pin(async move { answer.map(|(from, text)| (from, text.to_owned())) })
    }
  }
  let fetcher = Redirected(Default::default());
  let master = Url::parse("https://films.example.org/play/master.m3u8").unwrap();
  let info = inspect_manifest(&fetcher, master).await.unwrap();
  assert_eq!(
    *fetcher.0.lock().unwrap(),
    [
      "https://films.example.org/play/master.m3u8",
      "https://cdn.example.net/film/360/playlist.m3u8?token=fake",
    ]
  );
  assert_eq!(info.duration_seconds, Some(50.25));
}

#[tokio::test]
async fn manifest_inspection_rejects_oversized_responses() {
  let url = Url::parse("https://cdn.example.org/stream.m3u8").unwrap();
  let text = format!(
    "{}\n#{}",
    include_str!("fixtures/short-aes128.m3u8"),
    "a".repeat(MAX_MANIFEST_BYTES)
  );
  struct Oversized(String);
  impl ManifestFetcher for Oversized {
    fn fetch(&self, url: Url) -> BoxFuture<'_, Option<(Url, String)>> {
      Box::pin(async { Some((url, self.0.clone())) })
    }
  }
  assert!(inspect_manifest(&Oversized(text), url).await.is_none());
}

#[test]
fn pages_fill_the_window_under_the_top_rows_and_beside_the_side_list() {
  let fit = Fit {
    top: 96.,
    side: 240.,
    overlay: false,
    rtl: false,
  };
  assert_eq!(content_rect(1200., 800., fit), (240., 96., 960., 704.));
  let rtl = Fit { rtl: true, ..fit };
  assert_eq!(content_rect(1200., 800., rtl), (0., 96., 960., 704.));
  let overlay = Fit {
    overlay: true,
    ..fit
  };
  assert_eq!(
    content_rect(1200., 800., overlay),
    content_rect(1200., 800., fit),
    "an open menu draws over the page, it does not move it"
  );
  assert_eq!(content_rect(100., 50., fit), (99., 49., 1., 1.));
}

const WINDOW: &str = "catch-0123456789abcdef0123456789abcdef";

fn open_tab(state: &mut BrowserState, url: &str, private: bool) -> String {
  let new = NewTab {
    url: url.into(),
    private,
    opener: None,
  };
  let id = state.tabs.open(new, tokio::time::Instant::now()).unwrap();
  let page = Arc::new(Session::tab(Url::parse(url).unwrap(), private));
  state.pages.insert(id.clone(), page);
  id
}

#[test]
fn the_shell_gets_the_active_tabs_page_and_a_row_for_every_tab() {
  let browser = Browser::new(true);
  let mut state = browser.state.lock().unwrap();
  let now = tokio::time::Instant::now();
  let home = state.tabs.active_id().to_owned();
  let first = open_tab(&mut state, "https://a.example.com/watch", false);
  let second = open_tab(&mut state, "https://b.example.org/", true);
  let sent = serde_json::to_value(state.changed(WINDOW).unwrap()).unwrap();
  assert_eq!(sent["activeTabId"], second.as_str());
  assert_eq!(sent["url"], "https://b.example.org/");
  let rows = sent["tabs"].as_array().unwrap();
  assert_eq!(rows.len(), 3);
  assert_eq!(rows[0]["url"], "", "the home page has no address");
  assert_eq!(rows[1]["host"], "a.example.com");
  assert_eq!(
    (&rows[1]["private"], &rows[2]["private"]),
    (&false.into(), &true.into())
  );
  assert!(state.changed(WINDOW).is_none(), "only differences are sent");

  state.tabs.select(&first, now).unwrap();
  let sent = serde_json::to_value(state.changed(WINDOW).unwrap()).unwrap();
  assert_eq!(sent["activeTabId"], first.as_str());
  assert_eq!(sent["host"], "a.example.com");
  assert_eq!(sent["loading"], true);

  state.tabs.select(&home, now).unwrap();
  let sent = serde_json::to_value(state.changed(WINDOW).unwrap()).unwrap();
  assert_eq!(
    (&sent["url"], &sent["firstRun"]),
    (&"".into(), &true.into())
  );
  assert_eq!(sent["tabs"].as_array().unwrap().len(), 3);
}

#[test]
fn a_window_id_names_the_page_of_its_active_tab() {
  let windows = CatchWindows::default();
  let browser = Arc::new(Browser::new(false));
  windows
    .browsers
    .lock()
    .unwrap()
    .insert(WINDOW.into(), browser.clone());
  assert_eq!(windows.session(WINDOW).err(), Some("catchNoPage"));
  let (first, second) = {
    let mut state = browser.active().unwrap();
    (
      open_tab(&mut state, "https://a.example.com/", false),
      open_tab(&mut state, "https://b.example.org/", false),
    )
  };
  let page = |tab: &str| browser.active().unwrap().pages[tab].clone();
  assert!(Arc::ptr_eq(
    &windows.session(WINDOW).unwrap(),
    &page(&second)
  ));
  let now = tokio::time::Instant::now();
  browser.active().unwrap().tabs.select(&first, now).unwrap();
  assert!(Arc::ptr_eq(
    &windows.session(WINDOW).unwrap(),
    &page(&first)
  ));
  assert!(windows
    .session("catch-ffffffffffffffffffffffffffffffff")
    .is_err());

  // A tab command: the active tab without a tab id, else the named tab.
  let active = windows.tab(WINDOW, None).unwrap();
  assert_eq!(active.label, format!("{WINDOW}-{first}"));
  assert!(Arc::ptr_eq(&active.session, &page(&first)));
  let named = windows.tab(WINDOW, Some(&second)).unwrap();
  assert_eq!(named.label, format!("{WINDOW}-{second}"));
  assert!(Arc::ptr_eq(&named.session, &page(&second)) && !named.private);
  let private = open_tab(
    &mut browser.active().unwrap(),
    "https://c.example.net/",
    true,
  );
  assert!(windows.tab(WINDOW, None).unwrap().private);
  browser.active().unwrap().pages.remove(&private);
  assert_eq!(windows.tab(WINDOW, None).err(), Some("catchNoPage"));
  assert_eq!(
    windows.tab(WINDOW, Some("t99")).err(),
    Some("catchTabClosed")
  );
}

#[test]
fn site_tools_act_on_the_named_tab_and_a_private_tab_takes_no_browser_sign_in() {
  use super::super::site_tools::importable;
  let windows = CatchWindows::default();
  let browser = Arc::new(Browser::new(false));
  windows
    .browsers
    .lock()
    .unwrap()
    .insert(WINDOW.into(), browser.clone());
  let (plain, private) = {
    let mut state = browser.active().unwrap();
    (
      open_tab(&mut state, "https://a.example.com/login", false),
      open_tab(&mut state, "https://b.example.org/login", true),
    )
  };
  let tab = |tab: Option<&str>| windows.tab(WINDOW, tab).unwrap();

  // The private tab is the active one: no import there, with or without its id.
  for refused in [None, Some(private.as_str())] {
    assert_eq!(
      importable(&tab(refused)).err(),
      Some("catchSigninUnsupported")
    );
  }
  assert!(importable(&tab(Some(&plain))).is_ok());
  // The offer: a tab the user sees, never a private one, never a hidden session.
  let offers = |tab: &TabPage| tab.session.active().unwrap().offers_signin();
  assert!(offers(&tab(Some(&plain))) && !offers(&tab(Some(&private))));
  let hidden = Session::new(Url::parse("https://a.example.com/login").unwrap());
  assert!(!hidden.active().unwrap().offers_signin());

  // The iPhone identity is one tab's: only its row says so, and a page that is gone has none.
  let mobile = || {
    let sent = serde_json::to_value(browser.active().unwrap().snapshot(WINDOW)).unwrap();
    let rows = sent["tabs"].as_array().unwrap().clone();
    assert_eq!(rows.len(), 3);
    rows
      .iter()
      .filter(|row| row["mobile"] == true)
      .map(|row| row["id"].as_str().unwrap().to_owned())
      .collect::<Vec<_>>()
  };
  assert!(mobile().is_empty());
  tab(Some(&plain)).session.active().unwrap().mobile = true;
  assert_eq!(mobile(), std::slice::from_ref(&plain));
  browser.active().unwrap().pages.remove(&plain);
  assert!(mobile().is_empty(), "its next page starts as desktop");
}

#[test]
fn a_background_tab_sleeps_after_ten_minutes_unless_it_is_private_or_busy() {
  use super::super::tabs::SLEEP_AFTER;
  let browser = Browser::new(false);
  let mut state = browser.state.lock().unwrap();
  let start = tokio::time::Instant::now();
  let plain = open_tab(&mut state, "https://a.example.com/", false);
  let private = open_tab(&mut state, "https://b.example.com/", true);
  let playing = open_tab(&mut state, "https://c.example.com/", false);
  let shown = open_tab(&mut state, "https://d.example.com/", false);
  state.pages[&playing]
    .active()
    .unwrap()
    .page
    .document
    .playing = true;
  let early = start + SLEEP_AFTER - Duration::from_secs(5);
  assert!(!state.due_sleep(&plain, early));
  let due = start + SLEEP_AFTER + Duration::from_secs(5);
  assert!(state.due_sleep(&plain, due));
  for awake in [&private, &playing, &shown] {
    assert!(!state.due_sleep(awake, due), "{awake}");
  }
  // The wait of a tab that played starts again when it was last seen playing.
  state.pages[&playing]
    .active()
    .unwrap()
    .page
    .document
    .playing = false;
  assert!(!state.due_sleep(&playing, due + Duration::from_secs(60)));
  assert!(state.due_sleep(&playing, due + SLEEP_AFTER));
  // A sleeping page loses `window.opener`: a sign-in pop-up and its opener both stay awake.
  let signin = state
    .tabs
    .open(
      NewTab {
        opener: Some(plain.clone()),
        ..NewTab::default()
      },
      due,
    )
    .unwrap();
  let page = Arc::new(Session::new(blank()));
  state.pages.insert(signin.clone(), page);
  state.tabs.select(&shown, due).unwrap();
  let later = due + SLEEP_AFTER * 2;
  assert!(!state.due_sleep(&signin, later));
  assert!(!state.due_sleep(&plain, later));
  // The pop-up closed: its opener is an ordinary background tab again.
  state.tabs.close(&signin).unwrap();
  assert!(state.due_sleep(&plain, later + SLEEP_AFTER));
}

#[test]
fn a_linked_tab_that_closes_itself_is_closed_and_failed_popups_are_counted() {
  let said = |text: &str| popup::parse(&serde_json::to_string(text).unwrap());
  assert_eq!(
    said("2:0:0"),
    Some(popup::Seen {
      failed: 2,
      closing: false
    })
  );
  // Closed by its own script, or by its opener (the engine's `closed`).
  assert!(said("0:1:0").unwrap().closing);
  assert!(said("0:0:1").unwrap().closing);
  for junk in ["", "1", "1:0", "x:0:0", "1:2:0", "1:0:0:0", "-1:0:0"] {
    assert_eq!(said(junk), None, "{junk}");
  }
  assert_eq!(popup::parse("{}"), None);

  let blocked = |session: &Session| {
    let state = session.active().unwrap();
    serde_json::to_value(state.page.snapshot(WINDOW, state.last_browser_status)).unwrap()
      ["popupsBlocked"]
      .clone()
  };
  // An ordinary tab that calls `window.close()` stays; only its failed pop-ups count.
  let tab = Session::new(Url::parse("https://a.example.com/").unwrap());
  assert!(!popup::apply(&mut tab.active().unwrap(), said("3:1:0")));
  assert_eq!(blocked(&tab), 3);
  assert!(!popup::apply(&mut tab.active().unwrap(), None));
  // A pop-up opened as a tab closes with its page.
  let linked = Session::new(blank());
  linked.active().unwrap().linked = true;
  assert!(!popup::apply(&mut linked.active().unwrap(), said("0:0:0")));
  assert!(popup::apply(&mut linked.active().unwrap(), said("0:1:0")));

  // While one page's "allow once" holds the engine's gate open, no page's request counts as a click.
  let browser = Browser::new(false);
  let mut state = browser.state.lock().unwrap();
  let allowed = open_tab(&mut state, "https://a.example.com/", false);
  open_tab(&mut state, "https://b.example.com/", false);
  assert!(!state.gate_open());
  state.pages[&allowed].active().unwrap().gate_open = true;
  assert!(state.gate_open());
}

#[test]
fn ready_replays_the_latest_state_to_a_new_listener() {
  let session = Session::new(Url::parse("https://example.org/").unwrap());
  let mut state = session.active().unwrap();
  let status = super::super::state::BrowserStatus {
    can_back: true,
    ..Default::default()
  };
  let first = state.page.changed("catch-test", status).unwrap();
  assert!(state.page.changed("catch-test", status).is_none());
  assert_eq!(state.page.replay("catch-test", status), first);
}

#[test]
fn drain_polls_fast_only_while_loading_or_playing() {
  assert_eq!(tick_interval(true, true, false), TICK_BUSY);
  assert_eq!(tick_interval(true, false, true), TICK_BUSY);
  assert_eq!(tick_interval(true, false, false), TICK_QUIET);
  assert_eq!(tick_interval(false, true, true), TICK_BLANK);
  assert_eq!(background_interval(true), TICK_QUIET);
  assert!(background_interval(false) > TICK_QUIET);
  assert!(TICK_QUIET >= Duration::from_secs(2) && TICK_BLANK <= Duration::from_secs(3));
}

#[test]
fn a_crash_reloads_once_and_a_repeat_within_a_minute_stops() {
  let mut crash = CrashRecovery::default();
  let start = Instant::now();
  assert_eq!(
    crash.crashed(start),
    Recovery::Reload(Duration::from_secs(1))
  );
  assert_eq!(
    crash.crashed(start + Duration::from_secs(5)),
    Recovery::Stop
  );
  assert_eq!(
    crash.crashed(start + Duration::from_secs(30)),
    Recovery::Stop
  );
  assert_eq!(
    crash.crashed(start + Duration::from_secs(91)),
    Recovery::Reload(Duration::from_secs(1)),
    "a crash long after the last one gets its reload again"
  );
}

#[test]
fn a_reused_hidden_page_starts_fresh_with_a_newer_generation() {
  let session = Session::new(Url::parse("https://example.com/watch").unwrap());
  {
    let mut state = session.active().unwrap();
    state.page.document.crashed = true;
    state.crash.crashed(Instant::now());
  }
  let old = session.state.lock().unwrap().page.generation;
  session.stop();
  let url = Url::parse("https://example.org/next").unwrap();
  session.reopen(url.clone(), false).unwrap();
  let mut state = session.active().unwrap();
  assert_eq!(state.page.url, url);
  assert!(state.page.generation != old);
  assert!(!state.page.document.crashed && state.loading);
  assert_eq!(
    state.crash.crashed(Instant::now()),
    Recovery::Reload(Duration::from_secs(1))
  );
}

#[test]
fn only_a_window_that_never_loaded_a_page_is_kept_and_it_starts_over() {
  let windows = CatchWindows::default();
  let browser = Arc::new(Browser::new(true));
  {
    let mut state = browser.state.lock().unwrap();
    assert!(state.reusable());
    let first = state.tabs.active_id().to_owned();
    state.fit.overlay = true;
    state.closed = true;
    state.reopen(false);
    assert!(!state.closed && !state.fit.overlay && state.reusable());
    assert!(
      state.tabs.active_id() != first,
      "tab ids are never used twice"
    );
    // A page's back/forward list cannot be cleared: Back in the next use would show it.
    state.had_page = true;
    assert!(!state.reusable());
    state.had_page = false;
    open_tab(&mut state, "https://example.com/", false);
    assert!(!state.reusable());
  }
  assert!(windows.park("catch-a", browser.clone()));
  assert!(
    !windows.park("catch-b", browser.clone()),
    "one window is kept"
  );
  windows.forget_idle("catch-b");
  assert_eq!(windows.take_idle().unwrap().0, "catch-a");
  assert!(windows.take_idle().is_none());
  // A parked window that was destroyed leaves the pool.
  assert!(windows.park("catch-a", browser));
  windows.forget_idle("catch-a");
  assert!(windows.take_idle().is_none());
}

#[test]
fn pages_see_the_installed_safari_and_never_a_made_up_version() {
  let agent = safari_agent("27.2").unwrap();
  assert!(agent.starts_with("Mozilla/5.0 (Macintosh;"));
  assert!(agent.ends_with(" Version/27.2 Safari/605.1.15"));
  assert!(!agent.contains("  "));
  for junk in ["", "27.", "27 Safari/1\r\nX: y", "1234567890.12"] {
    assert_eq!(safari_agent(junk), None, "{junk:?}");
  }
}

#[tokio::test(start_paused = true)]
async fn a_close_waits_for_its_own_blank_page() {
  let session = Arc::new(Session::new(blank()));
  session.stop();
  // A blank load of an earlier close finished late: no completion has been registered.
  assert!(session.state.lock().unwrap().blank_loaded.is_none());
  assert!(!session.blanked(|| true).await);
  assert!(
    !session.blanked(|| false).await,
    "the page refused to navigate"
  );
  let loaded = session.clone();
  let finished = session.blanked(move || {
    let send = loaded.state.lock().unwrap().blank_loaded.take().unwrap();
    send.send(()).unwrap();
    true
  });
  assert!(finished.await);
}

#[test]
fn a_crashed_catch_page_or_popup_never_gets_the_default_reload() {
  let id = "0123456789abcdef0123456789abcdef";
  assert_eq!(crashed_kind(&format!("catch-{id}")), Crashed::CatchPage);
  assert_eq!(crashed_kind(&format!("catch-{id}-t12")), Crashed::CatchPage);
  assert_eq!(
    split_label(&format!("catch-{id}-t12")),
    Some((format!("catch-{id}").as_str(), Some("t12")))
  );
  assert_eq!(
    crashed_kind(&format!("catch-popup-{id}")),
    Crashed::CatchPopup
  );
  for other in [
    "main",
    &format!("catch-bar-{id}"),
    "catch-x",
    &format!("catch-{id}-t"),
    &format!("catch-{id}-tx"),
    &format!("catch-{id}x"),
  ] {
    assert_eq!(crashed_kind(other), Crashed::Other, "{other}");
  }
}

#[test]
fn a_catch_page_is_refused_every_permission_but_autoplay() {
  use tauri::webview::{PermissionKind as Kind, PermissionResponse as Answer};
  for kind in [
    Kind::Microphone,
    Kind::Camera,
    Kind::Geolocation,
    Kind::Notifications,
    Kind::ClipboardRead,
    Kind::DisplayCapture,
    Kind::Midi,
    Kind::Sensors,
    Kind::MediaKeySystemAccess,
    Kind::LocalFonts,
    Kind::WindowManagement,
    Kind::PointerLock,
    Kind::AutomaticDownloads,
    Kind::FileSystemAccess,
    Kind::Other,
  ] {
    assert_eq!(permission(kind), Answer::Deny, "{kind:?}");
  }
  assert_eq!(permission(Kind::Autoplay), Answer::Default);
}

#[test]
fn a_link_opens_in_the_window_on_screen_and_only_without_one_is_a_window_made() {
  let windows = CatchWindows::default();
  // No window: `open` makes one (or takes the parked one).
  assert!(windows.shown().is_none());
  let browser = Arc::new(Browser::new(false));
  windows
    .browsers
    .lock()
    .unwrap()
    .insert(WINDOW.into(), browser.clone());
  let (id, shown) = windows
    .shown()
    .expect("the window on screen takes the link");
  assert_eq!(id, WINDOW);
  let link = Url::parse("https://example.com/watch").unwrap();
  {
    let mut state = shown.active().unwrap();
    let home = state.tabs.active_id().to_owned();
    assert_eq!(state.tabs.place_link(Some(&link)), LinkPlace::Home(home));
    let tab = open_tab(&mut state, link.as_str(), false);
    open_tab(&mut state, "https://example.org/", false);
    assert_eq!(state.tabs.place_link(Some(&link)), LinkPlace::Open(tab));
  }
  // A closing window is no place for a link, and a parked one is not on screen.
  browser.state.lock().unwrap().closed = true;
  assert!(windows.shown().is_none());
  let parked = windows.remove_browser(WINDOW).unwrap();
  assert!(windows.park(WINDOW, parked));
  assert!(windows.shown().is_none());
}

#[test]
fn a_popup_is_handed_to_the_engine_only_with_its_page_still_in_its_holder() {
  use tauri::{test::mock_app, WebviewBuilder, WindowBuilder};
  let app = mock_app();
  let handle = app.handle();
  let catch = WindowBuilder::new(handle, "catch-1").build().unwrap();
  // Built as `popup::build` does: a hidden holder window whose only webview is the page.
  let holder = WindowBuilder::new(handle, "catch-1-tab-2").build().unwrap();
  holder
    .add_child(
      WebviewBuilder::new("catch-1-tab-2", WebviewUrl::default()),
      LogicalPosition::new(0., 0.),
      LogicalSize::new(100., 100.),
    )
    .unwrap();
  let window = handle.get_webview_window("catch-1-tab-2").unwrap();
  assert!(matches!(
    popup::answer(Some(window.clone())),
    NewWindowResponse::Create { .. }
  ));
  // The page left its holder before the engine read it (the 0.1.6 abort): refused instead.
  let page = handle.get_webview("catch-1-tab-2").unwrap();
  page.reparent(&catch).unwrap();
  assert!(matches!(
    popup::answer(Some(window)),
    NewWindowResponse::Deny
  ));
  // No window could be built.
  assert!(matches!(
    popup::answer::<tauri::test::MockRuntime>(None),
    NewWindowResponse::Deny
  ));
}

#[test]
fn one_build_holds_a_tabs_page_slot_and_a_second_command_waits_for_it() {
  let browser = Browser::new(false);
  let mut state = browser.active().unwrap();
  let tab = state.tabs.active_id().to_owned();
  assert!(state.claim_page(&tab));
  // `load_tab` and `tab_select` racing the first build: neither builds a second webview.
  assert!(!state.claim_page(&tab));
  assert!(state.has_page(&tab));
  state.building.remove(&tab);
  state
    .pages
    .insert(tab.clone(), Arc::new(Session::tab(blank(), false)));
  assert!(
    !state.claim_page(&tab),
    "a tab with a page is not built again"
  );
}

#[test]
fn a_closed_window_forgets_every_private_store_and_cancels_pending_popups() {
  let browser = Browser::new(false);
  let (private, linked) = {
    let mut state = browser.active().unwrap();
    open_tab(&mut state, "https://example.com/", false);
    let private = open_tab(&mut state, "https://example.org/", true);
    // A pop-up of the private tab, between the engine's request and its move into the tab.
    let linked = state
      .tabs
      .open(
        NewTab {
          url: "https://example.org/signin".into(),
          private: false,
          opener: Some(private.clone()),
        },
        tokio::time::Instant::now(),
      )
      .unwrap();
    assert!(state.may_adopt(&linked));
    (private, linked)
  };
  let shut = browser.state.lock().unwrap().shut();
  assert_eq!(shut.pages.len(), 2);
  // The linked tab shares its opener's store: deleted once.
  let Store::Private(store) = browser
    .state
    .lock()
    .unwrap()
    .tabs
    .get(&private)
    .unwrap()
    .store
    .clone()
  else {
    panic!("a private tab has its own store");
  };
  assert_eq!(shut.stores, vec![store]);
  // The adoption that runs after the close finds the window closed and moves nothing.
  assert!(browser.active().is_err());
  assert!(browser.state.lock().unwrap().tabs.get(&linked).is_some());
}

#[test]
fn a_popup_waits_for_allow_once_only_when_no_click_caused_it() {
  let session = Session::tab(blank(), false);
  let mut state = session.active().unwrap();
  let (ad, signin) = (
    Url::parse("https://ads.example/").unwrap(),
    Url::parse("https://example.org/signin").unwrap(),
  );
  // Windows: no engine gate; WebView2's word for each request, matched by its address.
  popup::clicked(&mut state, "https://ads.example", false);
  popup::clicked(&mut state, "https://example.org/signin", true);
  assert!(popup::gesture(&mut state, &signin, false, false));
  assert!(
    !popup::gesture(&mut state, &signin, false, false),
    "one word, one request"
  );
  assert!(!popup::gesture(&mut state, &ad, false, false));
  // A request with no word (or after an overflow) is not a click.
  popup::clicked(&mut state, "https://example.org/signin", true);
  for _ in 0..8 {
    popup::clicked(&mut state, "https://ads.example/", false);
  }
  assert!(state.clicks.len() <= 8);
  assert!(!popup::gesture(&mut state, &signin, false, false));
  // macOS/Linux: the engine's gate, unless "allow once" holds it open; recorded words are ignored.
  popup::clicked(&mut state, "https://ads.example/", false);
  assert!(popup::gesture(&mut state, &ad, true, false));
  assert!(!popup::gesture(&mut state, &ad, true, true));
}

#[test]
fn a_page_that_leaves_for_another_site_right_after_its_popup_is_refused() {
  let url = |text: &str| Url::parse(text).unwrap();
  let (page, popup) = (
    url("https://movies.example/watch"),
    url("https://player.example/v/1"),
  );
  let at = std::time::Instant::now();
  let opened = Some((at, popup.clone()));
  let soon = at + std::time::Duration::from_millis(300);
  let tab_under = |to: &str, now| popup::tab_under(opened.as_ref(), true, &page, &url(to), now);
  // Tab-under: the page goes to a third site within a second of its pop-up.
  assert!(tab_under("https://ads.example/landing", soon));
  // Its own site, the pop-up's site, later, or no pop-up at all: a normal navigation.
  assert!(!tab_under("https://www.movies.example/next", soon));
  assert!(!tab_under("https://cdn.player.example/v/1", soon));
  assert!(!tab_under(
    "https://ads.example/landing",
    at + std::time::Duration::from_millis(1500)
  ));
  assert!(!tab_under("about:blank", soon));
  assert!(!popup::tab_under(
    None,
    true,
    &page,
    &url("https://ads.example/"),
    soon
  ));
}

/// The browser window opens plain-http sites (a link to one did nothing: macOS refuses every
/// http load in a web view unless the app allows web content), while the app's own requests keep
/// the secure-connection rule.
#[cfg(target_os = "macos")]
#[test]
fn macos_lets_web_pages_load_plain_http_but_not_the_apps_own_requests() {
  let info: plist::Dictionary =
    plist::from_file(concat!(env!("CARGO_MANIFEST_DIR"), "/Info.plist")).unwrap();
  let ats = info
    .get("NSAppTransportSecurity")
    .and_then(plist::Value::as_dictionary)
    .expect("NSAppTransportSecurity");
  // The one exception: no other key may widen it (arbitrary loads, media, local, domains).
  assert_eq!(ats.len(), 1, "{ats:?}");
  assert_eq!(
    ats
      .get("NSAllowsArbitraryLoadsInWebContent")
      .and_then(plist::Value::as_boolean),
    Some(true)
  );
}

/// The browser's tabs may load plain http, the app's own pages may not: their policy allows no
/// plain-http image (only the local asset host Windows serves files from).
#[test]
fn the_apps_own_pages_load_no_plain_http_images() {
  let conf: serde_json::Value =
    serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
  let images = conf["app"]["security"]["csp"]["img-src"]
    .as_str()
    .expect("img-src");
  let plain: Vec<_> = images
    .split_whitespace()
    .filter(|source| source.starts_with("http:") && *source != "http://asset.localhost")
    .collect();
  assert!(plain.is_empty(), "{plain:?}");
}

#[test]
fn a_file_download_goes_to_the_list_only_from_a_plain_shown_tab_once_in_5s() {
  let url = Url::parse("https://example.com/files/film%20archive.zip?key=123#section").unwrap();
  let now = Instant::now();
  let tab = Session::tab(url.clone(), false);
  let mut state = tab.active().unwrap();
  assert_eq!(
    serde_json::to_value(state.hands_off_download(&url, now)).unwrap(),
    serde_json::json!({"kind": "saving", "name": "film archive.zip"})
  );
  assert_eq!(
    state.hands_off_download(&url, now + Duration::from_secs(4)),
    None
  );
  let unnamed = Url::parse("https://example.com/files/?key=123").unwrap();
  assert_eq!(
    serde_json::to_value(state.hands_off_download(&unnamed, now + Duration::from_secs(5))).unwrap(),
    serde_json::json!({"kind": "saving", "name": null})
  );
  state.background = true;
  assert_eq!(
    state.hands_off_download(&url, now + Duration::from_secs(12)),
    None
  );
  drop(state);
  let private = Session::tab(url.clone(), true);
  assert_eq!(
    private.active().unwrap().hands_off_download(&url, now),
    Some(FileDownloadNotice::Private)
  );
  let proxied = Session::tab(url.clone(), false);
  proxied.active().unwrap().proxied = true;
  assert_eq!(
    proxied.active().unwrap().hands_off_download(&url, now),
    Some(FileDownloadNotice::Proxied)
  );
  let hidden = Session::new(url.clone());
  assert_eq!(hidden.active().unwrap().hands_off_download(&url, now), None);
}

#[test]
fn a_blob_or_data_download_says_it_cannot_be_saved() {
  let file = Url::parse("https://example.com/manual.pdf").unwrap();
  let now = Instant::now();
  for unsupported in ["blob:https://example.com/uuid", "data:text/plain,hi"] {
    let tab = Session::tab(file.clone(), false);
    let mut state = tab.active().unwrap();
    assert_eq!(
      state.hands_off_download(&Url::parse(unsupported).unwrap(), now),
      Some(FileDownloadNotice::Unsupported)
    );
    assert_eq!(
      state.hands_off_download(&file, now),
      Some(FileDownloadNotice::Saving {
        name: Some("manual.pdf".into())
      })
    );
  }
}

#[test]
fn only_a_refused_address_falls_back_to_the_browser() {
  use handoff::HandoffError::{Cancelled, Forbidden, InvalidContext, Refused, Runner};

  let name = Some("hint.pdf".into());
  assert_eq!(
    after_page_file(Ok("a.pdf".into()), name.clone()),
    Ok(FileDownloadNotice::Saving {
      name: Some("a.pdf".into())
    })
  );
  assert_eq!(
    after_page_file(Err(Refused), name.clone()),
    Err(name.clone())
  );
  for error in [Runner, InvalidContext, Forbidden, Cancelled] {
    assert_eq!(
      after_page_file(Err(error), name.clone()),
      Ok(FileDownloadNotice::Failed { name: name.clone() }),
      "{error:?}"
    );
  }
}

#[test]
fn a_page_file_only_reuses_site_cookies_on_the_original_download_origin_and_cookie_path() {
  use reqwest::header::{COOKIE, REFERER};
  use tauri::webview::Cookie;

  let page = Url::parse("https://www.example.co.uk/library").unwrap();
  let file = Url::parse("https://files.example.co.uk/signed/manual.pdf").unwrap();
  let cookie = Cookie::build(("session", "fixture"))
    .domain(".example.co.uk")
    .path("/signed")
    .secure(true)
    .build();
  let request =
    handoff::PageFileRequest::new("1".into(), file.clone(), &page, None, vec![cookie.clone()]);
  let headers = request.headers_for(&file);
  assert_eq!(headers[COOKIE], "session=fixture");
  assert!(headers[COOKIE].is_sensitive());
  assert_eq!(headers[REFERER], "https://www.example.co.uk/");

  let other_path = file.join("/public/manual.pdf").unwrap();
  assert!(!request.headers_for(&other_path).contains_key(COOKIE));
  for redirect in [
    "https://www.example.co.uk/signed/manual.pdf",
    "https://files.example.co.uk:444/signed/manual.pdf",
    "https://unrelated.co.uk/signed/manual.pdf",
  ] {
    let headers = request.headers_for(&Url::parse(redirect).unwrap());
    assert!(!headers.contains_key(COOKIE));
    assert!(!headers.contains_key(REFERER));
  }
  let other_site = Url::parse("https://unrelated.co.uk/library").unwrap();
  let request =
    handoff::PageFileRequest::new("2".into(), file.clone(), &other_site, None, vec![cookie]);
  assert!(!request.headers_for(&file).contains_key(REFERER));
  assert!(!request.headers_for(&file).contains_key(COOKIE));
}

#[tokio::test]
async fn a_page_file_uses_the_file_engine_for_raw_audio_and_refuses_pages_and_manifests() {
  use crate::files::{engine::Engine, probe::Kind, Error};
  use reqwest::ResponseBuilderExt;

  let url = Url::parse("https://example.org/download").unwrap();
  let request = Arc::new(handoff::PageFileRequest::new(
    "1".into(),
    url.clone(),
    &url,
    None,
    vec![],
  ));
  for (mime, downloadable) in [
    ("audio/mpeg", true),
    ("text/html", false),
    ("application/vnd.apple.mpegurl", false),
  ] {
    let engine = Engine::scripted(Arc::new(move |url, _| {
      let mut response = http::Response::builder()
        .status(200)
        .url(Url::parse(url).unwrap())
        .header("content-type", mime);
      if downloadable {
        response = response.header("Content-Disposition", "attachment; filename=song.mp3");
      }
      Ok(response.body(vec![0u8; 3]).unwrap().into())
    }))
    .with_page_file(Some(request.clone()));
    let (_active, cancel) = tokio::sync::watch::channel(true);
    let probe = engine.probe(url.as_str(), cancel).await;
    if downloadable {
      let probe = probe.unwrap();
      assert_eq!(probe.kind, Kind::File);
      assert_eq!(probe.name, "song.mp3");
    } else {
      assert!(matches!(probe, Err(Error::NotFile)));
    }
  }
}
