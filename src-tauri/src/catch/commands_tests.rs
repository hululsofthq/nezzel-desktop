use super::*;
use crate::catch::{handoff::FormatLabel, state::BrowserStatus};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

const ID: &str = "catch-2378dd5813974278a475d9f8cae54702";

struct FakeHandoff {
  format_calls: AtomicUsize,
  saving: Notify,
  finish: Notify,
}

impl Default for FakeHandoff {
  fn default() -> Self {
    Self {
      format_calls: AtomicUsize::new(0),
      saving: Notify::new(),
      finish: Notify::new(),
    }
  }
}

impl Handoff for FakeHandoff {
  fn formats(
    &self,
    request: HandoffRequest,
  ) -> BoxFuture<'_, Result<Vec<CatchFormat>, HandoffError>> {
    assert_eq!(request.page_label, ID);
    assert_eq!(
      request.candidate.url().as_str(),
      "https://cdn.example.org/media.mp4"
    );
    self.format_calls.fetch_add(1, Ordering::Relaxed);
    Box::pin(async {
      Ok(vec![CatchFormat {
        id: "best".into(),
        label: FormatLabel::Video,
        height: Some(720),
        ext: "mp4".into(),
        size_bytes: None,
        best: true,
      }])
    })
  }

  fn save(
    &self,
    request: HandoffRequest,
    format_id: Option<String>,
  ) -> BoxFuture<'_, Result<String, HandoffError>> {
    assert_eq!(request.page_url.as_str(), "https://example.com/watch");
    assert_eq!(request.frame_origin.as_deref(), Some("https://example.com"));
    assert_eq!(format_id.as_deref(), Some("best"));
    Box::pin(async {
      self.saving.notify_one();
      self.finish.notified().await;
      Ok("download-1".into())
    })
  }
}

fn session_with_candidate() -> (Session, String) {
  let url = Url::parse("https://example.com/watch").unwrap();
  let session = Session::new(url.clone());
  let id = {
    let mut state = session.active().unwrap();
    let drain = json!([{
      "method": "video.currentSrc", "url": "https://cdn.example.org/media.mp4",
      "frame": url.as_str(), "isTop": true, "durationSec": 85
    }]);
    let snapshot = json!([url.as_str(), "Video", "Test browser", drain]).to_string();
    state
      .page
      .apply_drain(&url, &serde_json::to_string(&snapshot).unwrap());
    let status = state.page.changed(ID, BrowserStatus::default()).unwrap();
    serde_json::to_value(status).unwrap()["candidates"][0]["id"]
      .as_str()
      .unwrap()
      .to_owned()
  };
  (session, id)
}

#[tokio::test]
async fn formats_are_cached_and_closing_keeps_the_in_flight_save() {
  let (session, id) = session_with_candidate();
  let handoff = FakeHandoff::default();
  for _ in 0..2 {
    assert_eq!(
      formats(&session, ID, &id, &handoff).await.unwrap()[0].id,
      "best"
    );
  }
  assert_eq!(handoff.format_calls.load(Ordering::Relaxed), 1);

  let save = save(&session, ID, &id, Some("best".into()), &handoff);
  let close = async {
    handoff.saving.notified().await;
    assert!(session.active().unwrap().page.document.saving);
    session.stop();
    handoff.finish.notify_one();
  };
  let (result, ()) = tokio::join!(save, close);
  assert_eq!(result.unwrap().download_id, "download-1");
  assert!(session.active().is_err());
}

#[test]
fn commands_are_limited_to_the_matching_local_bar() {
  let suffix = ID.strip_prefix("catch-").unwrap();
  let bar = format!("catch-bar-{suffix}");
  let popup = format!("catch-popup-{suffix}");
  assert!(crate::window_commands::allowed_for_window(
    &bar,
    "catch_save"
  ));
  assert!(crate::window_commands::allowed_for_window(
    &bar,
    "config_get"
  ));
  // The shell reads and changes its search engine only, never every app setting.
  for (cmd, allowed) in [
    ("catch_search_engine_get", true),
    ("catch_search_engine_set", true),
    // The browser sign-in switch's per-site list (the hidden page reads it too).
    ("catch_signin_off_get", true),
    ("catch_signin_off_set", true),
    // The "Block ads" switch of the side list's "This site" footer.
    ("catch_adblock_set", true),
    ("preferences_get", false),
    ("preferences_set", false),
    ("preferences_reset", false),
  ] {
    assert_eq!(
      crate::window_commands::allowed_for_window(&bar, cmd),
      allowed,
      "{cmd}"
    );
  }
  assert!(!crate::window_commands::allowed_for_window(
    ID,
    "catch_save"
  ));
  assert!(!crate::window_commands::allowed_for_window(
    &popup,
    "catch_save"
  ));
  assert!(owns_window(&bar, ID));
  assert!(!owns_window(&bar, "catch-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
}

#[test]
fn search_choice_changes_only_what_was_sent_and_refuses_unknown_engines() {
  let mut prefs = Preferences::default();
  prefs.paths.video_directory_template = "%(uploader)s".into();
  set_search_choice(&mut prefs, Some(SearchEngine::Brave), None);
  assert_eq!(
    serde_json::to_value(SearchChoice::of(&prefs)).unwrap(),
    json!({ "engine": "brave", "hintShown": false })
  );
  set_search_choice(&mut prefs, None, Some(true));
  assert_eq!(
    serde_json::to_value(SearchChoice::of(&prefs)).unwrap(),
    json!({ "engine": "brave", "hintShown": true })
  );
  assert_eq!(prefs.paths.video_directory_template, "%(uploader)s");
  assert!(serde_json::from_value::<Option<SearchEngine>>(json!("yahoo")).is_err());
  assert!(
    serde_json::from_value::<Option<SearchEngine>>(json!({ "url": "https://x.example" })).is_err()
  );
}

#[test]
fn report_context_contains_only_the_page_shape() {
  let url = Url::parse("https://www.instagram.com/reel/private-id/?token=secret#fragment").unwrap();
  let shape = report_shape(&url).unwrap();
  assert_eq!(shape, "instagram.com/reel/{id}");
  let reduced_again = crate::diagnostics::link::link_for_report(&shape, false).unwrap();
  assert_eq!(reduced_again.link.as_str(), shape);
}

#[test]
fn catch_capability_targets_the_local_bar_only() {
  use tauri::utils::acl::capability::Capability;
  let cap: Capability =
    serde_json::from_str(include_str!("../../capabilities/catch-bar.json")).unwrap();
  assert!(cap.local && cap.remote.is_none() && cap.windows.is_empty());
  assert_eq!(cap.webviews, ["catch-bar-*"]);
  assert!(address("https://example.org/").is_ok());
  let handler = include_str!("../lib.rs");
  let bar = format!("catch-bar-{}", ID.trim_start_matches("catch-"));
  for name in [
    "catch_open",
    "catch_navigate",
    "catch_save",
    "catch_formats",
    "catch_allow_popup_once",
    "catch_forget_site",
    "catch_deep_retry",
    "catch_close",
    "catch_bar_ready",
    "catch_bar_fit",
    "catch_show_download",
    "catch_report",
    "catch_tab_new",
    "catch_tab_select",
    "catch_tab_close",
    "catch_shell_fit",
    "catch_home",
    "catch_clear_recent",
  ] {
    assert!(
      handler.contains(&format!("catch::commands::{name},")),
      "{name}"
    );
    assert!(
      crate::window_commands::allowed_for_window(&bar, name),
      "{name}"
    );
    assert!(
      !crate::window_commands::allowed_for_window(ID, name),
      "{name}"
    );
  }
  // The shell's Paste button reads the clipboard; no other capture command is open to it.
  assert!(crate::window_commands::allowed_for_window(
    &bar,
    "capture_read_clipboard"
  ));
  assert!(!crate::window_commands::allowed_for_window(
    &bar,
    "capture_write_clipboard"
  ));
  assert!(!crate::window_commands::allowed_for_window(
    ID,
    "capture_read_clipboard"
  ));
  for name in ["catch_use_browser_signin", "catch_try_mobile"] {
    assert!(
      handler.contains(&format!("catch::site_tools::{name},")),
      "{name}"
    );
    assert!(
      crate::window_commands::allowed_for_window(&bar, name),
      "{name}"
    );
    assert!(
      !crate::window_commands::allowed_for_window(ID, name),
      "{name}"
    );
  }
  for caller in [
    "main",
    crate::capture::BAR_LABEL,
    crate::capture::CARD_LABEL,
  ] {
    assert!(crate::window_commands::allowed_for_window(
      caller,
      "catch_open"
    ));
  } // The browser sign-in switch's list: the shell only, never a page, a pop-up or a capture window.
  let suffix = ID.trim_start_matches("catch-");
  let popup = format!("catch-popup-{suffix}");
  for name in ["catch_signin_off_get", "catch_signin_off_set"] {
    assert!(
      handler.contains(&format!("catch::hidden_signin::{name},")),
      "{name}"
    );
    assert!(
      crate::window_commands::allowed_for_window(&bar, name),
      "{name}"
    );
    for caller in [
      ID,
      popup.as_str(),
      crate::capture::BAR_LABEL,
      crate::capture::CARD_LABEL,
    ] {
      assert!(
        !crate::window_commands::allowed_for_window(caller, name),
        "{caller} {name}"
      );
    }
  }
}

#[tokio::test]
async fn saving_marks_the_normalized_url_when_seen_after_navigation() {
  let (session, id) = session_with_candidate();
  let handoff = FakeHandoff::default();
  handoff.finish.notify_one();
  save(&session, ID, &id, Some("best".into()), &handoff)
    .await
    .unwrap();

  let id = {
    let mut state = session.active().unwrap();
    let url = Url::parse("https://example.com/next").unwrap();
    state.page.navigated(url.clone());
    let hits = json!([{
      "method": "video.currentSrc", "url": "https://CDN.example.org/media.mp4#position",
      "frame": url.as_str(), "isTop": true, "durationSec": 85
    }]);
    let snapshot = json!([url.as_str(), "Next", "Browser", hits]).to_string();
    state
      .page
      .apply_drain(&url, &serde_json::to_string(&snapshot).unwrap());
    let snapshot =
      serde_json::to_value(state.page.changed(ID, BrowserStatus::default()).unwrap()).unwrap();
    assert_eq!(snapshot["candidates"][0]["alreadySaved"], true);
    assert!(snapshot.get("savedDownloadId").is_none());
    snapshot["candidates"][0]["id"].as_str().unwrap().to_owned()
  };
  assert!(matches!(
    save(&session, ID, &id, Some("best".into()), &handoff).await,
    Err("catchAlreadySaved")
  ));
}

#[test]
fn the_shells_tab_and_fit_payloads_are_read() {
  let fit: ShellFit =
    serde_json::from_value(json!({ "top": 96, "side": 240.5, "overlay": true, "rtl": true }))
      .unwrap();
  assert_eq!(
    (fit.top, fit.side, fit.overlay, fit.rtl),
    (96., 240.5, true, true)
  );
  let fit: ShellFit =
    serde_json::from_value(json!({ "top": 96, "side": 48, "overlay": false })).unwrap();
  assert!(!fit.rtl, "the contract's fit has no direction");

  let opts: TabOptions = serde_json::from_value(json!({})).unwrap();
  assert!(opts.url.is_none() && !opts.private);
  let opts: TabOptions =
    serde_json::from_value(json!({ "url": "https://example.org/", "private": true })).unwrap();
  assert_eq!(opts.url.as_deref(), Some("https://example.org/"));
  assert!(opts.private);

  let home = Home {
    recent_sites: vec![RecentSite {
      host: "example.org".into(),
      title: None,
    }],
    saved: vec![RecentSave {
      download_id: "download-1".into(),
      title: "Video".into(),
      host: "example.org".into(),
      duration_sec: Some(85.),
      size_bytes: None,
      height: Some(720),
    }],
  };
  assert_eq!(
    serde_json::to_value(home).unwrap(),
    json!({
      "recentSites": [{ "host": "example.org" }],
      "saved": [{ "downloadId": "download-1", "title": "Video", "host": "example.org",
        "durationSec": 85.0, "height": 720 }],
    })
  );
}

#[tokio::test]
async fn a_save_gives_the_home_page_its_row_from_what_the_page_already_says() {
  let (session, id) = session_with_candidate();
  let handoff = FakeHandoff::default();
  formats(&session, ID, &id, &handoff).await.unwrap();
  let note = recent_save(&session, &id, Some("best")).unwrap();
  assert_eq!(
    (note.host.as_str(), note.title.as_str()),
    ("example.com", "Video")
  );
  assert_eq!((note.duration_sec, note.height), (Some(85.), Some(720)));
  assert!(recent_save(&session, "missing", None).is_some_and(|note| note.height.is_none()));
}
