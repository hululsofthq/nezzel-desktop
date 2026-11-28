use super::*;
use tauri::webview::cookie::SameSite;

const NOW: i64 = 1_800_000_000;
const SECRET: &str = "s3cret-value-never-logged";

fn candidate(id: &str, signed_in: bool, site_cookies: bool, app_bound: bool) -> BrowserCandidate {
  let browser = match id.split(':').next() {
    Some("firefox") => BrowserKind::Firefox,
    Some("safari") => BrowserKind::Safari,
    _ => BrowserKind::Chrome,
  };
  BrowserCandidate {
    id: id.into(),
    browser,
    profile: id.into(),
    modified: std::time::SystemTime::UNIX_EPOCH,
    signed_in,
    site_cookies,
    app_bound,
    active_time: 0.0,
  }
}

#[test]
fn jar_rows_map_to_page_cookies_and_foreign_hosts_are_dropped() {
  let host = "www.example.com";
  let cookie = |line: &str| jar_cookie(line, host, NOW, BrowserKind::Firefox);
  let full = cookie(&format!(
    "#HttpOnly_.example.com\tTRUE\t/account\tTRUE\t{}\tsid\t{SECRET}",
    NOW + 60
  ))
  .unwrap();
  assert_eq!(
    (
      full.name(),
      full.value(),
      full.domain_original(),
      full.path()
    ),
    ("sid", SECRET, Some(".example.com"), Some("/account"))
  );
  assert_eq!((full.secure(), full.http_only()), (Some(true), Some(true)));
  assert_eq!(
    full.expires_datetime().map(|at| at.unix_timestamp()),
    Some(NOW + 60)
  );
  // The jar has no SameSite: the engine's default applies.
  assert_eq!(full.same_site(), None::<SameSite>);

  let session = cookie("www.example.com\tFALSE\t/\tFALSE\t0\tpref\tdark").unwrap();
  assert_eq!(session.expires(), Some(Expiration::Session));
  assert_eq!(
    (session.secure(), session.http_only()),
    (Some(false), Some(false))
  );
  assert_eq!(session.domain_original(), Some("www.example.com"));

  for dropped in [
    format!(".evil.com\tTRUE\t/\tTRUE\t0\tsid\t{SECRET}"),
    format!("example.com.evil.com\tFALSE\t/\tTRUE\t0\tsid\t{SECRET}"),
    format!(".example.com\tTRUE\t/\tTRUE\t{}\told\tx", NOW - 1),
    "# Netscape HTTP Cookie File".into(),
    String::new(),
    ".example.com\tTRUE\t/\tTRUE\tnot-a-number\tsid\tx".into(),
    ".example.com\tTRUE\t/\tTRUE\t0\tsid".into(),
    ".example.com\tTRUE\t/\tTRUE\t0\t\tnameless".into(),
  ] {
    assert!(cookie(&dropped).is_none(), "{dropped}");
  }
}

#[test]
fn only_the_pages_own_site_is_imported_and_the_subdomain_column_sets_the_dot() {
  let kept = |domain: &str, subdomains: &str, host: &str| {
    let row = format!("{domain}\t{subdomains}\t/\tTRUE\t0\tsid\t{SECRET}");
    jar_cookie(&row, host, NOW, BrowserKind::Firefox)
      .and_then(|cookie| cookie.domain_original().map(str::to_owned))
  };
  for (domain, subdomains, host, expected) in [
    (
      ".youtube.com",
      "TRUE",
      "www.youtube.com",
      Some(".youtube.com"),
    ),
    // Another site's session is never copied, even one the page's sign-in leans on.
    (".google.com", "TRUE", "www.youtube.com", None),
    ("accounts.google.com", "FALSE", "www.youtube.com", None),
    (".instagram.com", "TRUE", "www.threads.net", None),
    // An address has no subdomains.
    (".127.0.0.1", "TRUE", "127.0.0.1", None),
    ("127.0.0.1", "TRUE", "127.0.0.1", None),
    (".127.0.0.1", "FALSE", "127.0.0.1", None),
    ("127.0.0.1", "FALSE", "127.0.0.1", Some("127.0.0.1")),
    ("127.0.0.1", "FALSE", "127.0.0.2", None),
    // The subdomain column decides between a domain cookie and a host-only one.
    (
      "example.com",
      "TRUE",
      "www.example.com",
      Some(".example.com"),
    ),
    (
      ".example.com",
      "FALSE",
      "www.example.com",
      Some("example.com"),
    ),
    (
      "example.com",
      "FALSE",
      "www.example.com",
      Some("example.com"),
    ),
    ("example.com", "maybe", "www.example.com", None),
  ] {
    assert_eq!(
      kept(domain, subdomains, host).as_deref(),
      expected,
      "{domain} {subdomains} on {host}"
    );
  }
}

#[test]
fn nothing_is_written_once_the_page_left_the_site() {
  let url = |raw: &str| Url::parse(raw).unwrap();
  let host = "www.example.com";
  assert!(stayed_on_site(
    Some(&url("https://www.example.com/feed")),
    host
  ));
  // A redirect inside the site still takes the sign-in.
  assert!(stayed_on_site(Some(&url("https://m.example.com/")), host));
  assert!(!stayed_on_site(Some(&url("https://example.org/")), host));
  assert!(!stayed_on_site(
    Some(&url("https://example.com.evil.test/")),
    host
  ));
  assert!(!stayed_on_site(None, host));
  assert!(stayed_on_site(
    Some(&url("https://127.0.0.1:8080/")),
    "127.0.0.1"
  ));
  assert!(!stayed_on_site(
    Some(&url("https://127.0.0.2/")),
    "127.0.0.1"
  ));
}

#[test]
fn reload_uses_the_address_at_dispatch() {
  let original = Url::parse("https://www.example.com/login").unwrap();
  let on_site = |now: &Url| stayed_on_site(Some(now), "www.example.com");
  for (allowed, destination, should_reload) in [
    (on_site as fn(&Url) -> bool, "https://example.org/", false),
    (on_site, "https://m.example.com/", true),
    (is_http, "about:blank", false),
  ] {
    // The original address was eligible before the cookie write or user-agent switch.
    assert!(allowed(&original));
    let current = Url::parse(destination).unwrap();
    let mut reloaded = false;
    let result = native::reload_if_at_url(Some(current.clone()), allowed, || {
      reloaded = true;
      Ok(())
    });
    assert_eq!(reloaded, should_reload, "{destination}");
    assert_eq!(
      result,
      Ok(should_reload.then_some(current)),
      "{destination}"
    );
  }
}

#[test]
fn profile_pick_follows_the_site_sign_in() {
  let pick_profile = |candidates: &[BrowserCandidate], host: &str| {
    pick_profiles(candidates.to_vec(), host).map(|mut profiles| profiles.remove(0))
  };
  let none: [BrowserCandidate; 0] = [];
  assert_eq!(pick_profile(&none, "example.com").err(), Some(SIGNIN_NONE));
  // A known sign-in site needs its session cookie; site cookies alone are not a sign-in.
  let visitor = [candidate("chrome:a", false, true, false)];
  assert_eq!(
    pick_profile(&visitor, "www.instagram.com").err(),
    Some(SIGNIN_NONE)
  );
  // Any other site: live cookies of its own are the only signal.
  assert_eq!(
    pick_profile(&visitor, "example.com").unwrap().id,
    "chrome:a"
  );
  // Ranked order is kept; a profile locked to its browser is skipped, and reported when alone.
  let ranked = [
    candidate("chrome:bound", true, true, true),
    candidate("chrome:b", true, true, false),
  ];
  assert_eq!(
    pick_profile(&ranked, "www.instagram.com").unwrap().id,
    "chrome:b"
  );
  assert_eq!(
    pick_profile(&ranked[..1], "www.instagram.com").err(),
    Some(SIGNIN_UNSUPPORTED)
  );
  // Firefox needs no keychain: it goes first where both are signed in; ties keep the ranking.
  let both = [
    candidate("chrome:a", true, true, false),
    candidate("firefox:a", true, true, false),
    candidate("firefox:b", true, true, false),
  ];
  assert_eq!(
    pick_profile(&both, "www.instagram.com").unwrap().id,
    "firefox:a"
  );
  // Safari tells nothing about a sign-in before it is read: alone, it is not a "none".
  let safari = [
    candidate("safari", false, false, false),
    candidate("chrome:a", false, false, false),
  ];
  assert_eq!(
    pick_profile(&safari[..1], "example.com").err(),
    Some(SIGNIN_UNSUPPORTED)
  );
  assert_eq!(
    pick_profile(&safari, "example.com").err(),
    Some(SIGNIN_NONE)
  );
}

#[test]
fn export_failures_map_to_the_contract_codes() {
  for (code, expected) in [
    ("browserKeychainDenied", SIGNIN_DENIED),
    ("browserLoginDisabled", SIGNIN_DENIED),
    ("cancelled", SIGNIN_DENIED),
    ("browserProfileMissing", SIGNIN_NONE),
    ("browserAppBound", SIGNIN_UNSUPPORTED),
    ("browserDpapiFailed", SIGNIN_UNSUPPORTED),
    // Full Disk Access refused: the user's own "no", like the keychain.
    ("browserSafariAccessDenied", SIGNIN_DENIED),
    ("browserDatabaseLocked", SIGNIN_UNSUPPORTED),
    ("browserCookiesUnreadable", SIGNIN_UNSUPPORTED),
    ("fetchTimeout", SIGNIN_UNSUPPORTED),
  ] {
    assert_eq!(signin_error(code), expected, "{code}");
  }
  // The page's own store did not take the cookies.
  for (code, expected) in [
    ("catchBrowserTimeout", SIGNIN_UNSUPPORTED),
    ("catchBrowserUnavailable", SIGNIN_UNSUPPORTED),
    ("catchWindowClosed", "catchWindowClosed"),
  ] {
    assert_eq!(write_error(code), expected, "{code}");
  }
}

fn jar_root() -> (std::path::PathBuf, CookieContext) {
  let root = std::env::temp_dir().join(format!("nezzel-signin-{}", uuid::Uuid::new_v4()));
  let context = CookieContext {
    exporter: std::path::PathBuf::new(),
    directory: root.join("browser-cookies"),
  };
  (root, context)
}

fn exported(context: &CookieContext, domain: &str) -> CookieAccess {
  let cookie = Cookie::build(("sid", SECRET))
    .domain(domain.to_owned())
    .path("/")
    .secure(true)
    .build();
  CookieAccess::from_webview(context, domain.trim_start_matches('.'), &[cookie], "agent").unwrap()
}

fn attempts(context: &CookieContext) -> usize {
  std::fs::read_dir(&context.directory).unwrap().count()
}

#[test]
fn the_private_jar_is_removed_after_import_and_after_a_failure() {
  let (root, context) = jar_root();
  let access = exported(&context, ".example.com");
  assert_eq!(attempts(&context), 1);
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    let jar = &access.args[1];
    let mode = std::fs::metadata(jar).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
  }
  let cookies = take_jar(access, "www.example.com", NOW, BrowserKind::Firefox).unwrap();
  assert_eq!(cookies.len(), 1);
  assert_eq!(attempts(&context), 0);

  // Nothing of this site in the jar: the caller can try another profile after cleanup.
  let access = exported(&context, ".other.org");
  let empty = take_jar(access, "www.example.com", NOW, BrowserKind::Firefox).unwrap();
  assert!(empty.is_empty());
  assert_eq!(attempts(&context), 0);
  std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_import_answer_carries_no_cookie() {
  let answer = serde_json::to_string(&SigninImport {
    imported: 3,
    browser: Some("chrome".into()),
  })
  .unwrap();
  assert_eq!(answer, r#"{"imported":3,"browser":"chrome"}"#);
}

#[test]
fn sign_in_walls_are_found_by_address_or_password_field() {
  let url = |raw: &str| Url::parse(raw).unwrap();
  for (raw, candidate, password, expected) in [
    ("https://example.com/login?next=/v/1", false, false, true),
    ("https://example.com/users/sign-in", false, false, true),
    ("https://example.com/signin", false, false, true),
    (
      "https://accounts.example.com/v3/challenge",
      false,
      false,
      true,
    ),
    ("https://auth.example.com/", false, false, true),
    ("https://sso.example.org/start", false, false, true),
    ("https://example.com/account/videos", false, false, true),
    ("https://example.com/watch/1", false, true, true),
    // A video is offered: never a wall.
    ("https://example.com/login", true, true, false),
    ("https://example.com/watch/1", false, false, false),
    ("https://example.com/authors/jane", false, false, false),
    ("https://blogin.example.com/post", false, false, false),
    ("https://example.com/?next=login", false, false, false),
    ("about:blank", false, true, false),
  ] {
    assert_eq!(
      signin_wall(&url(raw), candidate, password),
      expected,
      "{raw}"
    );
  }
}

#[test]
fn iphone_and_desktop_identities() {
  assert_eq!(
    iphone_agent("18.5").as_deref(),
    Some(
      "Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like \
       Gecko) Version/18.5 Mobile/15E148 Safari/604.1"
    )
  );
  assert!(iphone_agent("26.1.2")
    .unwrap()
    .contains("iPhone OS 26_1 like"));
  assert_eq!(iphone_agent("18.5; evil"), None);
  assert_eq!(iphone_agent(""), None);
  let agent = mobile_agent();
  assert!(agent.contains("iPhone") && agent.contains("Mobile/"));
  // The desktop identity stays the Safari of this Mac (or the engine's own elsewhere).
  if let Some(desktop) = window::browser_agent() {
    assert!(desktop.contains("Macintosh") && !desktop.contains("Mobile"));
  }
}

#[test]
fn a_site_is_offered_once_per_window() {
  let wall = super::super::state::PageState::new(Url::parse("https://example.com/login").unwrap());
  let first_check = |window| claim(window, "example.com", &wall, false, false, || false).is_some();
  assert!(first_check("catch-a"));
  assert!(!first_check("catch-a"));
  assert!(first_check("catch-b"));
  forget_window("catch-a");
  assert!(first_check("catch-a"));
  forget_window("catch-a");
  forget_window("catch-b");
  forget_run(&["example.com"]);
}

fn page_at(raw: &str, password: bool) -> super::super::state::PageState {
  let mut page = super::super::state::PageState::new(Url::parse(raw).unwrap());
  page.password_visible = password;
  page
}

#[test]
fn a_login_form_on_the_top_page_asks_for_the_sign_in() {
  // Facebook's landing page: no login word in the address, its form is the wall.
  assert_eq!(
    look_for(&page_at("https://www.facebook.com/", false), false),
    None
  );
  assert_eq!(
    look_for(&page_at("https://www.facebook.com/", true), false),
    Some(Look::Wall)
  );
  assert_eq!(
    look_for(&page_at("https://example.com/login", false), false),
    Some(Look::Wall)
  );
  // Already offered on this page, or not a web page: no second look.
  let mut offered = page_at("https://www.facebook.com/", true);
  offered.signin_hint = true;
  assert_eq!(look_for(&offered, true), None);
  assert_eq!(look_for(&page_at("about:blank", true), true), None);
  // The first-visit variant looks on a known sign-in site only; any other site waits for a wall.
  assert_eq!(
    look_for(&page_at("https://www.youtube.com/", false), true),
    Some(Look::FirstVisit)
  );
  assert_eq!(
    look_for(&page_at("https://news.example.org/", false), true),
    None
  );
}

#[test]
fn a_first_visit_looks_once_per_site_per_run_never_for_an_off_site_or_a_private_tab() {
  let (site, page) = ("youtube.com", page_at("https://www.youtube.com/", false));
  let on = || false;
  // Private tab and switched-off site: nothing, and nothing is marked.
  assert_eq!(claim("fv-a", site, &page, true, true, on), None);
  assert_eq!(claim("fv-a", site, &page, false, true, || true), None);
  // Variant off: a plain visit never looks.
  assert_eq!(claim("fv-a", site, &page, false, false, on), None);
  // First visit looks once; the reload, another visit or another window does not.
  assert_eq!(
    claim("fv-a", site, &page, false, true, on),
    Some(Look::FirstVisit)
  );
  assert_eq!(claim("fv-a", site, &page, false, true, on), None);
  assert_eq!(claim("fv-b", site, &page, false, true, on), None);
  // Its window does not look again at a wall either; a new window's wall still does.
  let wall = page_at("https://www.youtube.com/", true);
  assert_eq!(claim("fv-a", site, &wall, false, true, on), None);
  assert_eq!(
    claim("fv-c", site, &wall, false, true, on),
    Some(Look::Wall)
  );
  // The off list is read only when a look is due.
  let mut read = false;
  let plain = page_at("https://example.net/watch", false);
  assert_eq!(
    claim("fv-a", "example.net", &plain, false, true, || {
      read = true;
      false
    }),
    None
  );
  assert!(!read);
  for id in ["fv-a", "fv-b", "fv-c"] {
    forget_window(id);
  }
  forget_run(&[site]);
}

#[test]
fn a_first_visit_whose_page_moved_on_leaves_the_look_to_the_landed_page() {
  let (site, page) = ("facebook.com", page_at("https://www.facebook.com/", false));
  let on = || false;
  let first = |window| claim(window, site, &page, false, true, on);
  // A signed-in browser was found, but the page moved on before the offer: the next page looks.
  assert_eq!(first("fv-m"), Some(Look::FirstVisit));
  look_ended("fv-m", site, Look::FirstVisit, true, false);
  assert_eq!(first("fv-m"), Some(Look::FirstVisit));
  // No signed-in browser: the run keeps its mark, no rescan on every page.
  look_ended("fv-m", site, Look::FirstVisit, false, false);
  assert_eq!(first("fv-m"), None);
  assert_eq!(first("fv-n"), None);
  forget_window("fv-m");
  forget_window("fv-n");
  forget_run(&[site]);
}

#[test]
fn a_mirror_or_short_link_domain_never_takes_the_mirrored_sites_sign_in() {
  // The export folds these into the mirrored site (and would skip that site's off switch).
  for mirror in ["fxtwitter.com", "vxtwitter.com", "www.youtube-nocookie.com"] {
    let wall = page_at(&format!("https://{mirror}/login"), true);
    assert_eq!(look_for(&wall, true), None, "{mirror}");
  }
  assert!(!own_site("x.com", true, "fxtwitter.com"));
  assert!(!own_site("youtube.com", true, "youtube-nocookie.com"));
  // A site's own domains stay one site.
  assert!(own_site("twitter.com", true, "x.com"));
  let row = format!(".x.com\tTRUE\t/\tTRUE\t0\tauth_token\t{SECRET}");
  assert!(jar_cookie(&row, "fxtwitter.com", NOW, BrowserKind::Firefox).is_none());
  assert!(jar_cookie(&row, "x.com", NOW, BrowserKind::Firefox).is_some());
}

#[test]
fn a_page_that_already_found_a_video_is_not_a_sign_in_wall() {
  let mut page = page_at("https://example.com/watch", false);
  let hit = serde_json::json!({
    "method": "video.currentSrc", "url": "https://cdn.example.org/video.mp4",
    "frame": "https://example.com/watch", "isTop": true, "durationSec": 600
  });
  let parts = serde_json::json!([
    page.url.as_str(),
    "Video",
    "Agent",
    [hit],
    ["", ""],
    [0, ""],
    true
  ]);
  let url = page.url.clone();
  page.apply_drain(&url, &serde_json::to_string(&parts.to_string()).unwrap());
  assert!(page.password_visible);
  assert_eq!(look_for(&page, false), None);
}

#[test]
fn a_first_visit_look_that_finds_nothing_leaves_a_later_wall_its_offer() {
  let (site, page) = (
    "g-first.example",
    page_at("https://www.youtube.com/", false),
  );
  let wall = page_at("https://www.youtube.com/", true);
  assert_eq!(
    claim("g-a", site, &page, false, true, || false),
    Some(Look::FirstVisit)
  );
  // While that look runs, a wall in the same window does not start a second one.
  assert_eq!(claim("g-a", site, &wall, false, true, || false), None);
  look_ended("g-a", site, Look::FirstVisit, false, false);
  assert_eq!(
    claim("g-a", site, &wall, false, true, || false),
    Some(Look::Wall)
  );
  // An offer that was raised is the window's one offer for the site.
  let other = "g-offered.example";
  assert_eq!(
    claim("g-a", other, &page, false, true, || false),
    Some(Look::FirstVisit)
  );
  look_ended("g-a", other, Look::FirstVisit, true, true);
  assert_eq!(claim("g-a", other, &wall, false, true, || false), None);
  forget_window("g-a");
  forget_run(&[site, other]);
}

#[test]
fn two_tabs_draining_at_once_start_one_look_for_a_site() {
  let first = page_at("https://www.youtube.com/", false);
  let wall = page_at("https://www.youtube.com/", true);
  for round in 0..2000 {
    let site = format!("race-{round}.example");
    let start = std::sync::Barrier::new(2);
    let won = std::thread::scope(|scope| {
      let look = |page| {
        let (site, start) = (&site, &start);
        scope.spawn(move || {
          start.wait();
          claim("race", site, page, false, true, || false).is_some()
        })
      };
      let (a, b) = (look(&first), look(&wall));
      u8::from(a.join().unwrap()) + u8::from(b.join().unwrap())
    });
    assert_eq!(won, 1, "round {round}");
    forget_run(&[&site]);
  }
  forget_window("race");
}

#[test]
fn an_import_goes_on_only_for_the_offered_site_while_its_switch_is_on() {
  let at = |raw: &str| Url::parse(raw).unwrap();
  let on = |_: &str| false;
  assert_eq!(
    import_allowed(
      "example.com",
      true,
      Some(&at("https://www.example.com/login")),
      on
    ),
    Ok("www.example.com".to_owned())
  );
  // The page moved to another site between the offer and the import (or during the export).
  assert_eq!(
    import_allowed(
      "example.com",
      true,
      Some(&at("https://target.example.org/")),
      on
    ),
    Err("catchLeftSite")
  );
  assert_eq!(
    import_allowed("example.com", true, None, on),
    Err("catchLeftSite")
  );
  // Switched off (also while the export ran): read natively, for the page's own host.
  let mut asked = Vec::new();
  assert_eq!(
    import_allowed(
      "example.com",
      true,
      Some(&at("https://www.example.com/")),
      |host| {
        asked.push(host.to_owned());
        true
      }
    ),
    Err(SIGNIN_DENIED)
  );
  assert_eq!(asked, ["www.example.com"]);
  // The tab no longer shows the offer (or never did): the command alone imports nothing.
  assert_eq!(
    import_allowed(
      "example.com",
      false,
      Some(&at("https://www.example.com/login")),
      on
    ),
    Err("catchLeftSite")
  );
  // A mirror's export would read the mirrored site: never started.
  assert_eq!(
    import_allowed(
      "fxtwitter.com",
      true,
      Some(&at("https://fxtwitter.com/")),
      on
    ),
    Err("catchLeftSite")
  );
}

#[test]
fn a_look_that_raised_no_offer_leaves_the_window_free_to_look_again() {
  let wall = page_at("https://example.com/login", false);
  let look = |site| claim("r2-a", site, &wall, false, false, || false);
  // A signed-in browser was found, but the page had moved on (a login redirect): no offer shown.
  assert_eq!(look("r2-moved.example"), Some(Look::Wall));
  look_ended("r2-a", "r2-moved.example", Look::Wall, true, false);
  assert_eq!(look("r2-moved.example"), Some(Look::Wall));
  // No browser is signed in: the same wall is not scanned for again; an offer is the one offer.
  look_ended("r2-a", "r2-moved.example", Look::Wall, false, false);
  assert_eq!(look("r2-moved.example"), None);
  assert_eq!(look("r2-shown.example"), Some(Look::Wall));
  look_ended("r2-a", "r2-shown.example", Look::Wall, true, true);
  assert_eq!(look("r2-shown.example"), None);
  forget_window("r2-a");
  forget_run(&["r2-moved.example", "r2-shown.example"]);
}

#[test]
fn a_short_clip_beside_a_login_form_is_still_a_sign_in_wall() {
  // A decorative loop or a short preview: nothing the page offers to save.
  let mut page = page_at("https://example.com/", false);
  let hit = serde_json::json!({
    "method": "video.currentSrc", "url": "https://cdn.example.org/loop.mp4",
    "frame": "https://example.com/", "isTop": true, "durationSec": 12
  });
  let parts = serde_json::json!([
    page.url.as_str(),
    "Welcome",
    "Agent",
    [hit],
    ["", ""],
    [0, ""],
    true
  ]);
  let url = page.url.clone();
  page.apply_drain(&url, &serde_json::to_string(&parts.to_string()).unwrap());
  assert_eq!(look_for(&page, false), Some(Look::Wall));
}

#[test]
fn a_plain_http_page_never_takes_a_browser_sign_in() {
  let plain = Url::parse("http://www.example.com/login").unwrap();
  assert!(!signin_wall(&plain, false, true));
  assert_eq!(look_for(&page_at(plain.as_str(), true), true), None);
  assert_eq!(
    look_for(&page_at("http://www.youtube.com/", false), true),
    None
  );
  assert!(!stayed_on_site(Some(&plain), "www.example.com"));
  assert_eq!(
    import_allowed("example.com", true, Some(&plain), |_| false),
    Err("catchLeftSite")
  );
}

#[cfg(unix)]
#[tokio::test]
async fn chrome_default_signin_reaches_the_page_with_its_own_live_cookies() {
  use crate::runners::gallery_files::Staging;
  use std::os::unix::fs::PermissionsExt;

  let fixture = Staging::new().unwrap();
  let root = fixture.path.join("Library/Application Support");
  let chrome = root.join("Google/Chrome");
  let now = unix_now();
  let raw = |seconds: i64| (seconds + 11_644_473_600) * 1_000_000;
  let expiry = now + 3600;
  for name in ["Default", "Profile 1", "Profile 4", "Profile 5"] {
    let profile = chrome.join(name);
    std::fs::create_dir_all(&profile).unwrap();
    let db = rusqlite::Connection::open(profile.join("Cookies")).unwrap();
    db.execute_batch("CREATE TABLE cookies (host_key TEXT, name TEXT, expires_utc INTEGER, last_access_utc INTEGER)").unwrap();
    let rows = [
      (".facebook.com", "visitor"),
      (".facebook.com", "c_user"),
      (".facebook.com", "xs"),
      (".youtube.com", "LOGIN_INFO"),
      (".google.com", "SID"),
    ];
    for (domain, cookie) in if name == "Default" {
      &rows[..]
    } else {
      &rows[..1]
    } {
      db.execute(
        "INSERT INTO cookies VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![domain, cookie, raw(expiry), raw(now)],
      )
      .unwrap();
    }
  }
  let firefox = root.join("Firefox/Profiles/fixture.default");
  std::fs::create_dir_all(&firefox).unwrap();
  rusqlite::Connection::open(firefox.join("cookies.sqlite")).unwrap()
    .execute_batch("CREATE TABLE moz_cookies (host TEXT, name TEXT, expiry INTEGER, lastAccessed INTEGER); INSERT INTO moz_cookies VALUES ('.facebook.com', 'visitor', 0, 1)").unwrap();

  let exporter = fixture.path.join("fixture-exporter");
  std::fs::copy(
    chrome.join("Default/Cookies"),
    fixture.path.join("expected.db"),
  )
  .unwrap();
  std::fs::write(&exporter, format!(r#"#!/bin/sh
set -eu
while [ "$#" -gt 0 ]; do
  case "$1" in
    --cookies-from-browser) profile="${{2#chrome:}}"; shift ;;
    --cookies) jar="$2"; shift ;;
  esac
  shift
done
/usr/bin/cmp -s "$profile/Cookies" "${{0%/*}}/expected.db"
printf '%s\n' '.facebook.com	TRUE	/	TRUE	{dated}	c_user	fixture' '.facebook.com	TRUE	/	TRUE	{dated}	xs	fixture' '.youtube.com	TRUE	/	TRUE	{dated}	LOGIN_INFO	fixture' '.google.com	TRUE	/	TRUE	{dated}	SID	fixture' '.facebook.com	TRUE	/	TRUE	{expired}	expired_fixture	fixture' > "$jar"
"#, dated = raw(expiry), expired = raw(now - 3600))).unwrap();
  std::fs::set_permissions(&exporter, std::fs::Permissions::from_mode(0o700)).unwrap();
  let context = CookieContext {
    exporter,
    directory: fixture.path.join("scoped"),
  };

  for (host, domain, expected) in [
    ("www.facebook.com", ".facebook.com", &["c_user", "xs"][..]),
    ("www.youtube.com", ".youtube.com", &["LOGIN_INFO"][..]),
  ] {
    let candidates = browser_logins::discover_mac_fixture(&fixture.path, host);
    let profiles = pick_profiles(candidates, host).unwrap();
    assert_eq!(profiles.len(), 1);
    assert!(profiles[0].id.ends_with("/Default"));
    let (browser, cookies) = read_signin_profiles(
      profiles,
      &crate::state::config_models::AuthSettings::default(),
      &Url::parse(&format!("https://{host}/")).unwrap(),
      &context,
      || Ok(()),
    )
    .await
    .unwrap();
    assert_eq!(browser, BrowserKind::Chrome);
    let written: Vec<_> = cookies
      .iter()
      .map(|cookie| {
        (
          cookie.name(),
          cookie.domain_original(),
          cookie.expires_datetime().map(|at| at.unix_timestamp()),
        )
      })
      .collect();
    let expected: Vec<_> = expected
      .iter()
      .map(|name| (*name, Some(domain), Some(expiry)))
      .collect();
    assert_eq!(written, expected, "{host}");
  }
}

#[test]
fn jar_expiry_accepts_seconds_and_chromium_microseconds() {
  let live = NOW + 60;
  let raw = |seconds: i64| (seconds + 11_644_473_600) * 1_000_000;
  for (browser, expiry, expected) in [
    (BrowserKind::Chrome, live, Some(Some(live))),
    (BrowserKind::Chrome, raw(live), Some(Some(live))),
    (BrowserKind::Chrome, 0, Some(None)),
    (BrowserKind::Chrome, raw(NOW - 1), None),
    (BrowserKind::Chrome, i64::MAX, None),
    (BrowserKind::Firefox, live, Some(Some(live))),
    (BrowserKind::Firefox, raw(live), None),
  ] {
    let row = format!(".example.com\tTRUE\t/\tTRUE\t{expiry}\tsid\tfixture");
    let expiry_in_store = jar_cookie(&row, "www.example.com", NOW, browser)
      .map(|cookie| cookie.expires_datetime().map(|at| at.unix_timestamp()));
    assert_eq!(expiry_in_store, expected, "{} {expiry}", browser.name());
  }
}

#[tokio::test]
async fn signin_falls_back_to_quiet_profiles_with_one_prompt_budget() {
  let cookie = Cookie::build(("sid", "fixture"))
    .domain(".example.com")
    .build();
  let missing = "browserProfileMissing";
  for (outcomes, expected_reads, expected) in [
    (
      &[
        ("firefox:gone", Err(missing)),
        ("firefox:empty", Ok(false)),
        ("chrome:a", Ok(true)),
      ][..],
      &["firefox:gone", "firefox:empty", "chrome:a"][..],
      Ok(BrowserKind::Chrome),
    ),
    (
      &[("chrome:gone", Err(missing)), ("chrome:b", Ok(true))][..],
      &["chrome:gone", "chrome:b"][..],
      Ok(BrowserKind::Chrome),
    ),
    (
      &[
        ("firefox:a", Err("browserCookiesUnreadable")),
        ("chrome:b", Ok(true)),
      ][..],
      &["firefox:a", "chrome:b"][..],
      Ok(BrowserKind::Chrome),
    ),
    (
      &[("chrome:empty", Ok(false)), ("chrome:b", Ok(true))][..],
      &["chrome:empty"][..],
      Err(SIGNIN_UNSUPPORTED),
    ),
    (
      &[
        ("chrome:a", Err("browserCookiesUnreadable")),
        ("firefox:b", Ok(true)),
      ][..],
      &["chrome:a", "firefox:b"][..],
      Ok(BrowserKind::Firefox),
    ),
    (
      &[
        ("chrome:a", Err("browserKeychainDenied")),
        ("firefox:b", Ok(true)),
      ][..],
      &["chrome:a"][..],
      Err(SIGNIN_DENIED),
    ),
    (
      &[
        ("chrome:a", Err("browserCookiesUnreadable")),
        ("chrome:b", Ok(true)),
        ("firefox:c", Ok(false)),
      ][..],
      &["chrome:a", "firefox:c"][..],
      Err(SIGNIN_UNSUPPORTED),
    ),
  ] {
    let profiles = outcomes
      .iter()
      .map(|(id, _)| candidate(id, true, true, false))
      .collect();
    let mut reads = Vec::new();
    let result = try_profiles(
      profiles,
      true,
      || Ok(()),
      |profile| {
        reads.push(profile.id.clone());
        let (_, outcome) = outcomes.iter().find(|(id, _)| *id == profile.id).unwrap();
        std::future::ready(outcome.map(|has_cookie| {
          if has_cookie {
            vec![cookie.clone()]
          } else {
            vec![]
          }
        }))
      },
    )
    .await;
    assert_eq!(reads, expected_reads);
    assert_eq!(
      result,
      expected.map(|browser| (browser, vec![cookie.clone()]))
    );
  }
}

#[tokio::test]
async fn signin_exhaustion_distinguishes_no_cookies_from_unreadable_profiles() {
  for (outcomes, expected) in [
    (
      [Err("browserProfileMissing"), Err("browserProfileMissing")],
      SIGNIN_NONE,
    ),
    ([Err("browserProfileMissing"), Ok(())], SIGNIN_NONE),
    (
      [Err("browserCookiesUnreadable"), Ok(())],
      SIGNIN_UNSUPPORTED,
    ),
  ] {
    let profiles = vec![
      candidate("firefox:a", true, true, false),
      candidate("chrome:b", true, true, false),
    ];
    let mut outcomes = outcomes.into_iter();
    let result = try_profiles(
      profiles,
      true,
      || Ok(()),
      |_| std::future::ready(outcomes.next().unwrap().map(|()| Vec::new())),
    )
    .await;
    assert_eq!(result.err(), Some(expected));
    assert!(outcomes.next().is_none());
  }
}

#[tokio::test]
async fn signin_stops_reading_profiles_when_the_offer_or_site_changes() {
  for (destination, sign_in_wall) in [
    ("https://example.com/redirect", false),
    ("https://example.org/login", true),
  ] {
    let session = window::Session::new(Url::parse("https://example.com/login").unwrap());
    session.active().unwrap().page.signin_hint = true;
    let mut reads = 0;
    let result = try_profiles(
      vec![
        candidate("firefox:a", true, true, false),
        candidate("chrome:b", true, true, false),
      ],
      true,
      || profile_read_allowed(&session, "example.com", |_| false),
      |_| {
        reads += 1;
        let mut state = session.active().unwrap();
        state.page.navigated(Url::parse(destination).unwrap());
        if sign_in_wall {
          state.page.signin_hint = true;
        }
        std::future::ready(Ok(Vec::new()))
      },
    )
    .await;
    assert_eq!(reads, 1, "{destination}");
    assert_eq!(result.err(), Some("catchLeftSite"));
  }
}

#[tokio::test]
async fn signin_stops_after_denial_or_a_revoked_guard() {
  use std::sync::atomic::{AtomicUsize, Ordering};

  for first_read in [Err("browserKeychainDenied"), Ok(Vec::new())] {
    let reads = AtomicUsize::new(0);
    let result = try_profiles(
      vec![
        candidate("firefox:a", true, true, false),
        candidate("chrome:b", true, true, false),
      ],
      true,
      || {
        if first_read.is_ok() && reads.load(Ordering::Relaxed) > 0 {
          Err(SIGNIN_DENIED)
        } else {
          Ok(())
        }
      },
      |profile| {
        reads.fetch_add(1, Ordering::Relaxed);
        std::future::ready(if profile.browser == BrowserKind::Firefox {
          first_read.clone()
        } else {
          Ok(vec![Cookie::new("sid", "fixture")])
        })
      },
    )
    .await;
    assert_eq!(reads.load(Ordering::Relaxed), 1);
    assert_eq!(result.err(), Some(SIGNIN_DENIED));
  }
}
