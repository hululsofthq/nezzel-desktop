use super::*;
use adblock::content_blocking::CbResourceType;

const PAGE: &str = "https://videos.example.org/watch/42";

#[test]
fn blocks_ads_and_trackers_but_never_the_page_or_what_it_plays() {
  engine();
  let cases = [
    // Ads and trackers.
    (
      "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js",
      Kind::Script,
      true,
    ),
    (
      "https://securepubads.g.doubleclick.net/tag/js/gpt.js",
      Kind::Script,
      true,
    ),
    ("https://ib.adnxs.com/ut/v3/prebid", Kind::Fetch, true),
    (
      "https://www.google-analytics.com/analytics.js",
      Kind::Script,
      true,
    ),
    (
      "https://imasdk.googleapis.com/js/sdkloader/ima3.js",
      Kind::Script,
      true,
    ),
    // The page itself, even on an ad domain, and any frame's document (as on WebKit: an
    // embedded player is a frame).
    ("https://ib.adnxs.com/landing", Kind::Document, false),
    (
      "https://securepubads.g.doubleclick.net/tag/js/gpt.js",
      Kind::Document,
      false,
    ),
    // What a video plays, even from an ad host.
    (
      "https://g.doubleclick.net/pagead/video/preroll.mp4",
      Kind::Media,
      false,
    ),
    (
      "https://ib.adnxs.com/media/master.m3u8?token=1",
      Kind::Fetch,
      false,
    ),
    (
      "https://ib.adnxs.com/media/seg-00001.ts",
      Kind::Other,
      false,
    ),
    (
      "https://ib.adnxs.com/media/manifest.mpd",
      Kind::Fetch,
      false,
    ),
    // The site's own player and files.
    (
      "https://videos.example.org/static/player.js",
      Kind::Script,
      false,
    ),
    (
      "https://cdn.jsdelivr.net/npm/hls.js@1/dist/hls.min.js",
      Kind::Script,
      false,
    ),
    (
      "https://vjs.zencdn.net/8.10.0/video.min.js",
      Kind::Script,
      false,
    ),
    (
      "https://cdn.example.net/hls/720p/index.m3u8",
      Kind::Fetch,
      false,
    ),
    (
      "https://videos.example.org/api/stream?id=42",
      Kind::Fetch,
      false,
    ),
    // Not a web address.
    ("not a url", Kind::Script, false),
  ];
  for (url, kind, blocked) in cases {
    assert_eq!(blocks(url, PAGE, kind), blocked, "{url} as {kind:?}");
  }
}

/// YouTube's player loads with blocking on: its pieces come by fetch from googlevideo (no media
/// type, no media file name), its script and API from youtube.com. Its ad calls stay blocked.
#[test]
fn youtube_player_requests_load_while_its_ad_calls_are_blocked() {
  engine();
  let page = "https://www.youtube.com/shorts/ynQcf3PSsyI";
  let cases = [
    (
      "https://rr3---sn-4g5e6nzz.googlevideo.com/videoplayback?expire=1&itag=243&range=0-9999",
      Kind::Fetch,
      false,
    ),
    (
      "https://rr3---sn-4g5e6nzz.googlevideo.com/videoplayback?expire=1&itag=251&sabr=1",
      Kind::Other,
      false,
    ),
    (
      "https://www.youtube.com/youtubei/v1/player?prettyPrint=false",
      Kind::Fetch,
      false,
    ),
    (
      "https://www.youtube.com/s/player/abc123/player_ias.vflset/en_US/base.js",
      Kind::Script,
      false,
    ),
    (
      "https://i.ytimg.com/vi/ynQcf3PSsyI/hqdefault.jpg",
      Kind::Image,
      false,
    ),
    (
      "https://www.youtube.com/pagead/viewthroughconversion/1",
      Kind::Fetch,
      true,
    ),
    (
      "https://www.youtube.com/api/stats/ads?ver=2",
      Kind::Fetch,
      true,
    ),
  ];
  for (url, kind, blocked) in cases {
    assert_eq!(blocks(url, page, kind), blocked, "{url} as {kind:?}");
  }
  // macOS and Linux block with WebKit's own rule lists, made from the same list.
  // Only the filters that can match one of these addresses: every literal piece of a filter's
  // pattern is in the address it matches, and compiling the whole list takes minutes.
  let addresses = cases.map(|(url, ..)| url.to_ascii_lowercase());
  let list: Vec<&str> = LIST
    .lines()
    .filter(|line| {
      let pattern = line
        .trim_start_matches("@@")
        .split('$')
        .next()
        .unwrap_or("");
      let pattern = pattern.to_ascii_lowercase();
      (pattern.len() > 1 && pattern.starts_with('/') && pattern.ends_with('/'))
        || addresses.iter().any(|url| {
          pattern
            .split(['*', '^', '|'])
            .all(|piece| url.contains(piece))
        })
    })
    .collect();
  assert!(list.len() > cases.len(), "{} filters", list.len());
  // The order WebKit gets them in: every block, then the exceptions (`webkit_parts`).
  let (mut all, mut exceptions): (Vec<_>, Vec<_>) = webkit_rule_list(&list.join("\n"))
    .unwrap()
    .rules
    .into_iter()
    .partition(|rule| rule.action.typ != CbType::IgnorePreviousRules);
  all.append(&mut exceptions);
  all.extend(never_blocked());
  let rules = webkit_compiled(&all);
  for (url, kind, blocked) in cases {
    let webkit = match kind {
      Kind::Script => CbResourceType::Script,
      Kind::Image => CbResourceType::Image,
      _ => CbResourceType::Raw,
    };
    assert_eq!(
      webkit_verdict(&rules, url, page, webkit),
      blocked,
      "WebKit: {url} as {kind:?}"
    );
  }
}

/// A request never waits for the matcher on the UI thread: until it is built, nothing is blocked.
#[test]
fn nothing_is_blocked_while_the_matcher_is_being_built() {
  let ad = "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js";
  let cell = OnceLock::new();
  assert!(!blocks_once_built(&cell, ad, PAGE, Kind::Script));
  assert!(cell.get().is_none(), "the request built the matcher");
  assert!(cell.set(matcher("||googlesyndication.com^")).is_ok());
  assert!(blocks_once_built(&cell, ad, PAGE, Kind::Script));
}

#[test]
fn catch_fixture_media_is_never_blocked() {
  let link = regex::Regex::new(r#"https?://[^"'\s<)]+"#).unwrap();
  let fixtures = [
    include_str!("fixtures/master.m3u8"),
    include_str!("fixtures/short-aes128.m3u8"),
    include_str!("fixtures/static.mpd"),
    include_str!("fixtures/protected.mpd"),
  ];
  let urls: Vec<_> = fixtures
    .iter()
    .flat_map(|text| link.find_iter(text))
    .collect();
  assert!(!urls.is_empty());
  engine();
  for url in urls {
    assert!(!blocks(url.as_str(), PAGE, Kind::Fetch), "{}", url.as_str());
  }
}

#[test]
fn webkit_rules_never_block_the_page_or_media_and_fit_webkit() {
  let parts = webkit_rules().expect("list converts");
  assert!(!parts.is_empty());
  for part in &parts {
    let rules: Vec<serde_json::Value> = serde_json::from_str(part).expect("json");
    assert!(rules.len() <= MAX_WEBKIT_RULES);
    let types = |rule: &serde_json::Value| rule["trigger"]["resource-type"].as_array().cloned();
    for rule in &rules {
      if rule["action"]["type"] == "block" {
        let types = types(rule).expect("every block names its types");
        assert!(!types.contains(&"document".into()) && !types.contains(&"media".into()));
      }
    }
    // The never-blocked exceptions come after every block in each part.
    let last_block = rules
      .iter()
      .rposition(|rule| rule["action"]["type"] == "block");
    let media = rules.iter().rposition(|rule| {
      rule["action"]["type"] == "ignore-previous-rules"
        && types(rule).is_some_and(|t| t == vec![serde_json::Value::from("media")])
    });
    assert!(media > last_block);
  }
}

#[test]
fn switched_off_sites_are_remembered_by_registrable_domain() {
  let page = |url: &str| Url::parse(url).unwrap();
  let mut off = OffSites::default();
  assert!(off.on(&page("https://www.example.co.uk/a")));
  assert!(off.set(&page("https://www.example.co.uk/a"), false));
  assert!(!off.set(&page("https://m.example.co.uk/b"), false));
  assert!(!off.on(&page("https://video.example.co.uk/")));
  assert!(off.on(&page("https://other.example/")));

  let kept = OffSites::from_json(&off.to_json());
  assert!(!kept.on(&page("https://example.co.uk/")));

  assert!(off.set(&page("https://example.co.uk/"), true));
  assert!(off.on(&page("https://www.example.co.uk/a")));
  assert!(OffSites::from_json("not json").on(&page("https://example.co.uk/")));
}

#[test]
fn switched_off_sites_keep_the_newest_two_hundred() {
  let mut off = OffSites::default();
  for n in 0..=MAX_OFF {
    off.set(
      &Url::parse(&format!("https://site{n}.example/")).unwrap(),
      false,
    );
  }
  assert!(off.on(&Url::parse("https://site0.example/").unwrap()));
  assert!(!off.on(&Url::parse(&format!("https://site{MAX_OFF}.example/")).unwrap()));
}

/// WebKit's verdict for one request under content-blocker rules, evaluated the way WebKit does:
/// rules in order, a matching block blocks, a matching `ignore-previous-rules` lifts it.
#[test]
fn the_site_list_is_kept_whole_and_never_lost_to_a_read_error() {
  let dir = std::env::temp_dir().join(format!("nezzel-adblock-{}", uuid::Uuid::new_v4()));
  let path = dir.join(OFF_FILE);
  let url = Url::parse("https://www.videos.example.org/watch/42").unwrap();

  // Missing: empty, and the first save makes the file.
  let mut off = read_off(&path).unwrap();
  assert!(off.set(&url, false));
  write_off(&path, &off).unwrap();
  let kept = read_off(&path).unwrap();
  assert!(!kept.on(&url));
  assert!(!path.with_extension("json.tmp").exists());

  // There but unreadable (here: a folder): not taken as an empty list.
  let blocked = dir.join("unreadable");
  std::fs::create_dir_all(&blocked).unwrap();
  assert!(read_off(&blocked).is_none());
  let _ = std::fs::remove_dir_all(dir);

  // The file is read without the list's lock (requests ask it on the UI thread); a failed read
  // leaves it to be tried again.
  let list = Mutex::new(None);
  load_with(&list, || {
    assert!(list.try_lock().is_ok(), "read while the lock is held");
    None
  });
  assert!(list.lock().unwrap().is_none());
  load_with(&list, || Some(kept.clone()));
  assert!(!list.lock().unwrap().as_ref().unwrap().on(&url));
  load_with(&list, || unreachable!("read again once loaded"));
}

fn webkit_blocks(rules: &[CbRule], url: &str, page: &str, kind: CbResourceType) -> bool {
  webkit_verdict(&webkit_compiled(rules), url, page, kind)
}

/// Each rule with its filter compiled once: the bundled list has many thousands.
fn webkit_compiled(rules: &[CbRule]) -> Vec<(regex::Regex, &CbRule)> {
  rules
    .iter()
    .map(|rule| {
      let trigger = &rule.trigger;
      let filter = regex::RegexBuilder::new(&trigger.url_filter)
        .case_insensitive(!trigger.url_filter_is_case_sensitive.unwrap_or(false))
        .build()
        .unwrap();
      (filter, rule)
    })
    .collect()
}

fn webkit_verdict(
  rules: &[(regex::Regex, &CbRule)],
  url: &str,
  page: &str,
  kind: CbResourceType,
) -> bool {
  let (target, top) = (Url::parse(url).unwrap(), Url::parse(page).unwrap());
  let host = top.host_str().unwrap();
  let third = super::super::auto::site_of(&target) != super::super::auto::site_of(&top);
  let on_domain = |domains: &Vec<String>| {
    domains.iter().any(|domain| match domain.strip_prefix('*') {
      Some(base) => host == base || host.ends_with(&format!(".{base}")),
      None => host == domain,
    })
  };
  let mut blocked = false;
  for (filter, rule) in rules {
    let trigger = &rule.trigger;
    let applies = filter.is_match(url)
      && trigger
        .resource_type
        .as_ref()
        .is_none_or(|types| types.contains(&kind))
      && trigger
        .load_type
        .iter()
        .all(|load| matches!(load, adblock::content_blocking::CbLoadType::ThirdParty) == third)
      && trigger.if_domain.as_ref().is_none_or(on_domain)
      && !trigger.unless_domain.as_ref().is_some_and(on_domain)
      && trigger.if_top_url.is_none()
      && trigger.unless_top_url.is_none();
    if applies {
      blocked = rule.action.typ == CbType::Block;
    }
  }
  blocked
}

#[test]
fn webkit_form_gives_the_rust_matchers_verdicts() {
  let list = [
    "||ads.example.net^",
    "||track.example.io^$third-party",
    "/banner/ad_*.gif",
    "||cdn.example.com/ads.js$script",
    "@@||ads.example.net/allowed/",
    "||promo.example.net^$domain=news.example.org",
    "||widget.example.net^$domain=~blog.example.org",
    "&ad_slot=",
    "-ad-banner-",
  ]
  .join("\n");
  let engine = matcher(&list);
  let converted = webkit_rule_list(&list).unwrap();
  assert_eq!(converted.dropped, 0);
  let page = "https://news.example.org/story";
  let other = "https://blog.example.org/post";
  let cases = [
    (
      "https://ads.example.net/x.js",
      page,
      Kind::Script,
      CbResourceType::Script,
    ),
    (
      "https://ads.example.net/allowed/x.js",
      page,
      Kind::Script,
      CbResourceType::Script,
    ),
    (
      "https://track.example.io/p.gif",
      page,
      Kind::Image,
      CbResourceType::Image,
    ),
    (
      "https://site.example/banner/ad_1.gif",
      page,
      Kind::Image,
      CbResourceType::Image,
    ),
    (
      "https://cdn.example.com/ads.js",
      page,
      Kind::Script,
      CbResourceType::Script,
    ),
    (
      "https://cdn.example.com/ads.js",
      page,
      Kind::Image,
      CbResourceType::Image,
    ),
    (
      "https://promo.example.net/a.js",
      page,
      Kind::Script,
      CbResourceType::Script,
    ),
    (
      "https://promo.example.net/a.js",
      other,
      Kind::Script,
      CbResourceType::Script,
    ),
    (
      "https://widget.example.net/w.js",
      page,
      Kind::Script,
      CbResourceType::Script,
    ),
    (
      "https://widget.example.net/w.js",
      other,
      Kind::Script,
      CbResourceType::Script,
    ),
    (
      "https://api.example.org/q?x=1&ad_slot=3",
      page,
      Kind::Fetch,
      CbResourceType::Raw,
    ),
    (
      "https://img.example.org/top-ad-banner-1.png",
      page,
      Kind::Image,
      CbResourceType::Image,
    ),
    (
      "https://news.example.org/player.js",
      page,
      Kind::Script,
      CbResourceType::Script,
    ),
    // What a video plays passes both forms, even from a blocked host.
    (
      "https://ads.example.net/v/seg-1.ts",
      page,
      Kind::Fetch,
      CbResourceType::Raw,
    ),
    (
      "https://ads.example.net/v/clip",
      page,
      Kind::Media,
      CbResourceType::Media,
    ),
  ];
  let mut rules = converted.rules;
  rules.extend(never_blocked());
  for (url, page, kind, webkit) in cases {
    assert_eq!(
      webkit_blocks(&rules, url, page, webkit),
      blocks_with(&engine, url, page, kind),
      "{url} on {page}"
    );
  }
}

/// The count hooks the delegate a page really has: objc2 registers a `define_class!` class (as
/// wry's delegate is) under a generated name, so a lookup by the bare name finds nothing.
#[cfg(target_os = "macos")]
#[test]
fn the_blocked_count_hooks_the_delegates_real_class() {
  use objc2::rc::Retained;
  use objc2::runtime::{AnyClass, NSObject};
  use objc2::{msg_send, sel, ClassType};

  objc2::define_class!(
    #[unsafe(super(NSObject))]
    struct CountTestDelegate;
  );
  let delegate: Retained<CountTestDelegate> = unsafe { msg_send![CountTestDelegate::class(), new] };
  assert!(AnyClass::get(c"CountTestDelegate").is_none());

  let acted = sel!(_webView:contentRuleListWithIdentifier:performedAction:forURL:);
  let class = delegate.class();
  assert!(!class.responds_to(acted));
  assert!(platform::imp::add_counter(class));
  assert!(class.responds_to(acted));
  assert!(!platform::imp::add_counter(class), "added once per class");
}

#[test]
#[ignore = "measure: WebKit size, dropped filters and matcher latency on the full list"]
fn measure_full_list() {
  let start = std::time::Instant::now();
  let converted = webkit_rule_list(LIST).unwrap();
  let rules = converted.rules.len();
  let parts = webkit_parts(converted.rules).unwrap();
  let bytes: usize = parts.iter().map(String::len).sum();
  println!(
    "webkit: parts {} rules {rules} dropped {} bytes {bytes} in {:?}",
    parts.len(),
    converted.dropped,
    start.elapsed()
  );
  let start = std::time::Instant::now();
  let built = matcher(LIST);
  println!(
    "engine built in {:?}, serialized {} bytes",
    start.elapsed(),
    built.serialize().len()
  );
  engine();
  let urls = [
    (
      "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js",
      Kind::Script,
    ),
    (
      "https://www.google-analytics.com/g/collect?v=2&tid=G-1",
      Kind::Fetch,
    ),
    ("https://cdn.example.net/assets/app.4f3a.js", Kind::Script),
    (
      "https://img.example.net/photos/2026/11/cover.jpg",
      Kind::Image,
    ),
    (
      "https://videos.example.org/api/v1/items?page=2",
      Kind::Fetch,
    ),
    ("https://fonts.example.com/inter.woff2", Kind::Font),
  ];
  let mut times: Vec<_> = (0..6000)
    .map(|n| {
      let (url, kind) = urls[n % urls.len()];
      let url = format!("{url}#{n}");
      let start = std::time::Instant::now();
      blocks(&url, PAGE, kind);
      start.elapsed()
    })
    .collect();
  times.sort();
  println!(
    "matcher p50 {:?} p95 {:?} p99 {:?}",
    times[times.len() / 2],
    times[times.len() * 95 / 100],
    times[times.len() * 99 / 100]
  );
}
