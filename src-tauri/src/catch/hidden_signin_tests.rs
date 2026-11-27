use super::*;
use tauri::webview::cookie::time::OffsetDateTime;

const NOW: i64 = 1_800_000_000;

fn cookie(name: &str, value: &str, domain: &str, expires: Option<i64>) -> Cookie<'static> {
  let expires = match expires {
    Some(at) => Expiration::DateTime(OffsetDateTime::from_unix_timestamp(at).unwrap()),
    None => Expiration::Session,
  };
  Cookie::build((name.to_owned(), value.to_owned()))
    .domain(domain.to_owned())
    .path("/")
    .expires(expires)
    .build()
}

fn names(cookies: &[Cookie<'static>]) -> Vec<String> {
  cookies
    .iter()
    .map(|c| format!("{}={}", c.name(), c.value()))
    .collect()
}

#[test]
fn only_the_links_own_site_is_copied() {
  let host = "www.example.com";
  let later = Some(NOW + 600);
  for (domain, name, expires, kept) in [
    (".example.com", "sid", later, true),
    ("www.example.com", "host", None, true),
    // Another host of the same site: one sign-in shared by a site's own addresses.
    ("accounts.example.com", "acct", later, true),
    (".other.org", "foreign", later, false),
    // A look-alike that only ends with the site's name.
    (".notexample.com", "lookalike", later, false),
    (".example.com.evil.net", "suffix", later, false),
    (".example.com", "old", Some(NOW - 1), false),
    (".example.com", "", later, false),
  ] {
    let copied = site_only(vec![cookie(name, "v", domain, expires)], host, NOW);
    assert_eq!(copied.len() == 1, kept, "{domain} {name:?}");
  }
  // A link at an address takes that address's host-only cookies only.
  let ip = site_only(
    vec![
      cookie("a", "v", "93.184.216.34", None),
      cookie("b", "v", ".93.184.216.34", None),
    ],
    "93.184.216.34",
    NOW,
  );
  assert_eq!(names(&ip), ["a=v"]);
  // No policy known: never sent along cross-site requests; a named one is kept.
  let mut strict = cookie("s", "v", ".example.com", None);
  strict.set_same_site(SameSite::Strict);
  let copied = site_only(
    vec![cookie("l", "v", ".example.com", None), strict],
    host,
    NOW,
  );
  let policies: Vec<_> = copied.iter().map(Cookie::same_site).collect();
  assert_eq!(policies, [Some(SameSite::Lax), Some(SameSite::Strict)]);
}

#[test]
fn a_third_party_mirror_never_gets_the_sites_sign_in() {
  for (domain, host, kept) in [
    // A site's own domains are one owner: one sign-in.
    (".x.com", "twitter.com", true),
    (".twitter.com", "x.com", true),
    // Mirrors listed as the site's short links are other owners.
    (".x.com", "fxtwitter.com", false),
    (".x.com", "vxtwitter.com", false),
    (".fxtwitter.com", "x.com", false),
    (".youtube.com", "www.youtube-nocookie.com", false),
    (".youtube.com", "m.youtube.com", true),
  ] {
    let copied = site_only(vec![cookie("sid", "v", domain, None)], host, NOW);
    assert_eq!(copied.len() == 1, kept, "{domain} -> {host}");
  }
  // A link at an IPv6 address (bracketed, as a link writes it).
  let copied = site_only(vec![cookie("a", "v", "::1", None)], "[::1]", NOW);
  assert_eq!(names(&copied), ["a=v"]);
}

#[test]
fn a_private_tab_is_never_the_profile_reader() {
  let tab = |page: &'static str, private: bool, asleep: bool| TabSource {
    page,
    private,
    asleep,
  };
  assert_eq!(
    profile_reader(false, [tab("p", true, false), tab("s", false, true)]),
    ProfileReader::None
  );
  assert_eq!(
    profile_reader(false, [tab("p", true, false), tab("n", false, false)]),
    ProfileReader::Tab("n")
  );
  // macOS 14+ reads the saved store itself, never a tab's page.
  assert_eq!(
    profile_reader(true, [tab("p", true, false)]),
    ProfileReader::Store
  );
}

#[test]
fn the_newer_cookie_wins_and_a_tie_goes_to_the_users_browser() {
  let profile = vec![
    cookie("sid", "nezzel-new", ".example.com", Some(NOW + 900)),
    cookie("pref", "nezzel", ".example.com", Some(NOW + 60)),
    cookie("tie", "nezzel", ".example.com", Some(NOW + 60)),
    cookie("only", "nezzel", ".example.com", None),
    // Same name, other domain form: a different cookie.
    cookie("sid", "host-only", "www.example.com", None),
  ];
  let browser = vec![
    cookie("sid", "browser-old", ".example.com", Some(NOW + 300)),
    cookie("pref", "browser", ".example.com", Some(NOW + 600)),
    cookie("tie", "browser", ".example.com", Some(NOW + 60)),
    cookie("mine", "browser", ".example.com", None),
  ];
  let mut merged = names(&merge(profile, browser));
  merged.sort();
  assert_eq!(
    merged,
    [
      "mine=browser",
      "only=nezzel",
      "pref=browser",
      "sid=host-only",
      "sid=nezzel-new",
      "tie=browser"
    ]
  );
}

#[test]
fn a_hidden_page_never_makes_the_keychain_ask() {
  let mut auth = AuthSettings::default();
  // macOS (keychain) and Linux (keyring) may ask; Windows never does.
  assert_eq!(
    SYSTEM_ASKS,
    cfg!(any(target_os = "macos", target_os = "linux"))
  );
  // Where nothing asks every browser reads; Firefox and Safari never ask.
  assert!(reads_quietly(false, BrowserKind::Chrome, &auth));
  assert!(reads_quietly(true, BrowserKind::Firefox, &auth));
  assert!(reads_quietly(true, BrowserKind::Safari, &auth));
  // Chrome on macOS: only once the main app's sign-in already reads that browser.
  assert!(!reads_quietly(true, BrowserKind::Chrome, &auth));
  auth
    .site_browsers
    .insert("instagram.com".into(), "brave:Default".into());
  assert!(!reads_quietly(true, BrowserKind::Chrome, &auth));
  assert!(reads_quietly(true, BrowserKind::Brave, &auth));
  auth.cookie_browser = "chrome:Profile 1".into();
  assert!(reads_quietly(true, BrowserKind::Chrome, &auth));
}

#[test]
fn a_browser_that_would_ask_never_hides_a_quiet_one() {
  use crate::runners::browser_logins::BrowserCandidate;
  let candidate = |browser| BrowserCandidate {
    id: format!("{browser:?}"),
    browser,
    profile: "Default".into(),
    modified: std::time::SystemTime::UNIX_EPOCH,
    signed_in: true,
    site_cookies: true,
    app_bound: false,
    active_time: 0.0,
  };
  let auth = AuthSettings {
    cookie_browser: "brave:Default".into(),
    ..AuthSettings::default()
  };
  let all = vec![
    candidate(BrowserKind::Chrome),
    candidate(BrowserKind::Brave),
  ];
  // Unfiltered, the order picks Chrome, which the main sign-in never read.
  let ranked = site_tools::pick_profiles(all.clone(), "example.com").unwrap();
  assert_eq!(ranked[0].browser, BrowserKind::Chrome);
  let quiet: Vec<_> = all
    .into_iter()
    .filter(|c| reads_quietly(true, c.browser, &auth))
    .collect();
  let picked = site_tools::pick_profiles(quiet, "example.com").unwrap();
  assert_eq!(picked[0].browser, BrowserKind::Brave);
}

#[test]
fn switched_off_sites_match_by_site_and_the_list_keeps_the_newest_choices() {
  let mut list = OffList::default();
  assert!(list.set("WWW.Example.com", true));
  assert_eq!(list.sites(), ["example.com"], "one entry per site");
  assert!(!list.set("m.example.com", true), "already off");
  assert!(list.off("m.example.com") && list.off("example.com"));
  assert!(!list.off("example.org"));
  for bad in [
    "javascript:alert(1)",
    "  ",
    "example.com:8443",
    "7",
    "[nope]",
    ".example.com",
  ] {
    assert!(!list.set(bad, true), "{bad}");
  }
  // On again from another host of the site: the site is on.
  assert!(list.set("www.example.com", false));
  assert!(!list.off("example.com") && list.sites().is_empty());
  assert!(list.set("[::1]", true) && list.off("[::1]") && !list.off("[::2]"));
  assert!(list.set("[::1]", false));
  for n in 0..=MAX_OFF {
    list.set(&format!("site{n}.example"), true);
  }
  assert_eq!(list.sites().len(), MAX_OFF);
  assert!(!list.off("site0.example") && list.off(&format!("site{MAX_OFF}.example")));
}

#[test]
fn the_shells_old_list_is_moved_in_once() {
  let legacy = vec!["www.example.com".to_owned(), 7.to_string(), "<bad>".into()];
  let mut first = OffList::default();
  assert!(first.migrate(&legacy));
  assert_eq!(first.sites(), ["example.com"]);
  // Saved: a later old list (another window's) is not taken again.
  let mut saved = OffList::from_json(&first.to_json()).unwrap();
  assert!(!saved.migrate(&["other.org".to_owned()]));
  assert_eq!(saved, first);
}

/// A scratch folder for one test, removed when it ends.
struct Scratch(std::path::PathBuf);

impl Scratch {
  fn new(name: &str) -> Self {
    let dir = std::env::temp_dir().join(format!("nz-off-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Self(dir)
  }
}

impl Drop for Scratch {
  fn drop(&mut self) {
    let _ = std::fs::remove_dir_all(&self.0);
  }
}

#[test]
fn an_unreadable_off_list_switches_every_site_off_and_is_never_written_over() {
  let scratch = Scratch::new("broken");
  let file = scratch.0.join(OFF_FILE);
  std::fs::write(&file, "{oops").unwrap();
  let mut slot = None;
  assert!(
    off_at(&mut slot, &file, "example.com"),
    "a corrupt file is every site off"
  );
  assert!(slot.is_none(), "never cached");
  let set = off_list_at(&mut slot, &file, |list| (list.set("other.org", true), ()));
  assert!(set.is_err() && slot.is_none());
  assert_eq!(
    std::fs::read_to_string(&file).unwrap(),
    "{oops",
    "never written over"
  );
  // Unreadable (here: a folder where the file should be) is the same; only no file is empty.
  let folder = scratch.0.join("folder.json");
  std::fs::create_dir(&folder).unwrap();
  assert!(off_at(&mut None, &folder, "example.com"));
  assert!(!off_at(
    &mut None,
    &scratch.0.join("none.json"),
    "example.com"
  ));
  assert!(load(Err(std::io::ErrorKind::PermissionDenied.into())).is_none());
}

#[test]
fn a_failed_save_keeps_the_old_list_and_a_retry_still_moves_the_shells_list_in() {
  let scratch = Scratch::new("save");
  // The folder the list goes in is a file: the save fails.
  let blocker = scratch.0.join("blocked");
  std::fs::write(&blocker, "").unwrap();
  let file = blocker.join(OFF_FILE);
  let mut slot = None;
  let legacy = ["www.example.com".to_owned()];
  let moved = off_list_at(&mut slot, &file, |list| (list.migrate(&legacy), ()));
  assert!(moved.is_err());
  assert!(slot.is_none(), "memory never says more than the file");
  // The folder is back: the retry takes the shell's list (it was never saved).
  std::fs::remove_file(&blocker).unwrap();
  let sites = off_list_at(&mut slot, &file, |list| {
    (list.migrate(&legacy), list.sites().to_vec())
  });
  assert_eq!(sites.unwrap(), ["example.com"]);
  // Written whole: the file is the list, no temporary file is left.
  assert_eq!(
    std::fs::read_to_string(&file).unwrap(),
    r#"["example.com"]"#
  );
  assert_eq!(std::fs::read_dir(&blocker).unwrap().count(), 1);
}

/// Read-only sources and the hidden page's store, recorded.
#[derive(Default)]
struct Fake {
  off: bool,
  slow: bool,
  slow_browser: bool,
  slow_write: bool,
  profile: Mutex<Vec<Cookie<'static>>>,
  browser: Vec<Cookie<'static>>,
  reads: Mutex<usize>,
  page: Mutex<Vec<Cookie<'static>>>,
}

impl Sources for Fake {
  fn off(&self, _host: &str) -> bool {
    self.off
  }

  fn profile(&self) -> BoxFuture<'_, Vec<Cookie<'static>>> {
    Box::pin(async move {
      *self.reads.lock().unwrap() += 1;
      if self.slow {
        tokio::time::sleep(Duration::from_secs(60)).await;
      }
      self.profile.lock().unwrap().clone()
    })
  }

  fn browser(&self) -> BoxFuture<'_, Vec<Cookie<'static>>> {
    Box::pin(async move {
      *self.reads.lock().unwrap() += 1;
      if self.slow_browser {
        tokio::time::sleep(Duration::from_secs(60)).await;
      }
      self.browser.clone()
    })
  }

  fn write(&self, cookies: Vec<Cookie<'static>>) -> BoxFuture<'_, ()> {
    Box::pin(async move {
      if self.slow_write {
        tokio::time::sleep(Duration::from_secs(15)).await;
      }
      self.page.lock().unwrap().extend(cookies)
    })
  }
}

fn signed_in() -> Fake {
  Fake {
    profile: Mutex::new(vec![
      cookie("sid", "nezzel", ".example.com", Some(NOW + 60)),
      cookie("ads", "x", ".tracker.net", Some(NOW + 60)),
    ]),
    browser: vec![cookie("pref", "browser", "www.example.com", None)],
    ..Fake::default()
  }
}

#[tokio::test]
async fn the_copy_goes_into_the_hidden_page_only_and_the_profile_is_untouched() {
  let fake = signed_in();
  let before = names(&fake.profile.lock().unwrap());
  assert!(copy_with(&fake, "www.example.com", NOW, COPY_LIMIT).await);
  let mut page = names(&fake.page.lock().unwrap());
  page.sort();
  assert_eq!(page, ["pref=browser", "sid=nezzel"]);
  assert_eq!(names(&fake.profile.lock().unwrap()), before);
}

#[tokio::test]
async fn a_switched_off_site_gets_nothing_and_nothing_is_read() {
  let fake = Fake {
    off: true,
    ..signed_in()
  };
  assert!(copy_with(&fake, "www.example.com", NOW, COPY_LIMIT).await);
  assert!(fake.page.lock().unwrap().is_empty());
  assert_eq!(*fake.reads.lock().unwrap(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_slow_read_loads_the_page_unsigned() {
  let fake = Fake {
    slow: true,
    slow_browser: true,
    ..signed_in()
  };
  assert!(copy_with(&fake, "www.example.com", NOW, COPY_LIMIT).await);
  assert!(fake.page.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn one_slow_source_never_drops_the_others_sign_in() {
  let fake = Fake {
    slow_browser: true,
    ..signed_in()
  };
  let start = tokio::time::Instant::now();
  assert!(copy_with(&fake, "www.example.com", NOW, COPY_LIMIT).await);
  assert_eq!(names(&fake.page.lock().unwrap()), ["sid=nezzel"]);
  assert!(start.elapsed() <= COPY_LIMIT);
}

#[tokio::test(start_paused = true)]
async fn a_slow_write_is_bounded_by_the_same_limit_and_the_page_is_not_used() {
  let fake = Fake {
    slow_write: true,
    ..signed_in()
  };
  let start = tokio::time::Instant::now();
  assert!(!copy_with(&fake, "www.example.com", NOW, COPY_LIMIT).await);
  assert!(start.elapsed() <= COPY_LIMIT, "took {:?}", start.elapsed());
}

#[test]
fn a_mirror_is_its_own_site_in_the_off_list() {
  let mut list = OffList::default();
  assert!(list.set("x.com", true));
  // Switching a mirror on or off never touches the site it mirrors.
  assert!(list.set("fxtwitter.com", true));
  assert!(list.set("fxtwitter.com", false));
  assert!(list.off("x.com") && list.off("twitter.com"));
  assert!(!list.off("fxtwitter.com"));
  // A site's own domains are one site.
  assert!(list.set("twitter.com", false));
  assert!(!list.off("x.com"));
}

#[test]
fn a_mirror_link_never_reads_the_mirrored_sites_cookies() {
  for (host, reads) in [
    ("www.example.com", true),
    ("x.com", true),
    ("twitter.com", true),
    ("fxtwitter.com", false),
    ("www.youtube-nocookie.com", false),
    ("93.184.216.34", true),
  ] {
    assert_eq!(export_matches_copy(host), reads, "{host}");
  }
}

#[test]
fn a_plain_http_link_gets_no_sign_in_copied() {
  let host = |raw: &str| copy_host(&Url::parse(raw).unwrap()).map(str::to_owned);
  assert_eq!(
    host("https://www.example.com/watch"),
    Some("www.example.com".into())
  );
  assert_eq!(host("http://www.example.com/watch"), None);
}

#[tokio::test]
async fn chrome_jar_dated_cookie_reaches_the_hidden_store() {
  use crate::runners::browser_cookies::{CookieAccess, CookieContext};
  use crate::runners::gallery_files::Staging;

  let stage = Staging::new().unwrap();
  let host = "www.example.com";
  let context = CookieContext {
    exporter: Default::default(),
    directory: stage.path.join("scoped"),
  };
  let access = CookieAccess::from_webview(
    &context,
    host,
    &[cookie("sid", "fixture", ".example.com", None)],
    "fixture-agent",
  )
  .unwrap();
  let raw_expiry = (NOW + 60 + 11_644_473_600) * 1_000_000;
  std::fs::write(
    &access.args[1],
    format!(".example.com\tTRUE\t/\tTRUE\t{raw_expiry}\tsid\tfixture\n"),
  )
  .unwrap();
  let fake = Fake {
    browser: site_tools::take_jar(access, host, NOW, BrowserKind::Chrome).unwrap_or_default(),
    ..Fake::default()
  };
  assert!(copy_with(&fake, host, NOW, COPY_LIMIT).await);
  let page = fake.page.lock().unwrap();
  assert_eq!(names(&page), ["sid=fixture"]);
  assert_eq!(
    page[0].expires_datetime().map(|at| at.unix_timestamp()),
    Some(NOW + 60)
  );
}
