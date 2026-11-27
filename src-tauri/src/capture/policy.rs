use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::Duration;

use crate::sites::{site_by_domain, site_of, SiteId};

const MAX_CLIPBOARD_BYTES: usize = 8192;

// Deliberately conservative: the generic extractor must never turn arbitrary copied text
// into a network request. A site's hosts and their subdomains (dot boundary) only.
const SITES: &[SiteId] = {
  use SiteId::*;
  &[
    Dropbox,
    GoogleDrive,
    GoogleDocs,
    YouTube,
    Vimeo,
    TikTok,
    Instagram,
    Facebook,
    X,
    Twitch,
    SoundCloud,
    Dailymotion,
    Reddit,
    Bilibili,
    Pinterest,
    Ted,
    Bandcamp,
    InternetArchive,
    Rumble,
    Odysee,
    // Photo and post readers (resolve/, gallery-dl).
    LinkedIn,
    Threads,
    Bluesky,
    Telegram,
    Snapchat,
    Tumblr,
    Kick,
    Vk,
    Ok,
    Streamable,
    Imgur,
    NineGag,
    Likee,
    Flickr,
  ]
};

/// Who brought the link: the clipboard watcher on its own, or the user in the quick bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Source {
  /// Passive clipboard watching: the conservative site list only.
  #[default]
  Watch,
  /// The user opened the quick bar (shortcut, tray, browser tab, typing or pasting).
  Explicit,
}

/// `supported_url`, plus, when the user named the link, any public http(s) page. A known site's
/// own rules (private Telegram links, Drive folders, shorteners) still decide its links.
pub fn accepted_url(text: &str, source: Source) -> Option<(String, String)> {
  supported_url(text).or_else(|| {
    if source == Source::Explicit {
      explicit_url(text)
    } else {
      None
    }
  })
}

/// Copying links out of Arabic text can wrap them in directional formatting marks.
/// Strip boundaries only: never rewrite a path, share key, or query value.
fn link_text(text: &str) -> Option<&str> {
  let text = text.trim_matches(|c: char| {
    c.is_whitespace()
      || matches!(c,
    '\u{061c}' | '\u{200b}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}'
      | '\u{2060}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
  });
  let invalid = text.len() > MAX_CLIPBOARD_BYTES
    || text
      .chars()
      .any(|c| c.is_whitespace() || c.is_control() || c == '\\');
  (!invalid).then_some(text)
}

/// An http(s) link written out in full to a public host: no credentials, no other port, no local name
/// or private address. Hosts with a capture rule of their own are left to `supported_url`.
fn explicit_url(text: &str) -> Option<(String, String)> {
  let text = link_text(text)?;
  let scheme = text.split_once("://")?.0.to_ascii_lowercase();
  if scheme != "https" && scheme != "http" {
    return None;
  }
  let url = reqwest::Url::parse(text).ok()?;
  if !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
    return None;
  }
  let host = crate::net::host(&url)?.to_ascii_lowercase();
  if site_of(&host).is_some_and(|site| SITES.contains(&site))
    || crate::files::hosts::is_share(url.as_str())
    || crate::links::normalize::needs_redirect(&url)
    || !crate::catch::auto::may_be_public(&url)
  {
    return None;
  }
  Some((url.to_string(), host))
}

pub fn supported_url(text: &str) -> Option<(String, String)> {
  let text = link_text(text)?;
  let has_scheme = text.split_once(':').is_some_and(|(scheme, _)| {
    scheme
      .as_bytes()
      .first()
      .is_some_and(u8::is_ascii_alphabetic)
      && scheme
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"+.-".contains(&b))
  });
  let normalized;
  let text = if !has_scheme {
    normalized = format!("https://{text}");
    normalized.as_str()
  } else {
    text
  };
  let original = reqwest::Url::parse(text).ok()?;
  let target = crate::links::normalize::pure(text).ok()?;
  let text = target.as_str();
  let (scheme, rest) = text.split_once("://")?;
  let scheme = scheme.to_ascii_lowercase();
  if scheme != "https" && scheme != "http" {
    return None;
  }
  let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
  let authority = rest[..end].to_ascii_lowercase();
  // Credentials, nonstandard ports and escaped hosts are intentionally unsupported.
  let host = authority
    .strip_suffix(":443")
    .filter(|_| scheme == "https")
    .or_else(|| authority.strip_suffix(":80").filter(|_| scheme == "http"))
    .unwrap_or(&authority);
  let share = crate::files::hosts::is_share(text);
  let site = site_of(host).filter(|site| SITES.contains(site));
  if site.is_none() && !share && !crate::links::normalize::needs_redirect(&target) {
    return None;
  }
  if !host
    .bytes()
    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
  {
    return None;
  }
  if host.len() > 253
    || !host.split('.').all(|label| {
      !label.is_empty() && label.len() <= 63 && !label.starts_with('-') && !label.ends_with('-')
    })
  {
    return None;
  }
  let suffix = rest[end..].split('#').next().unwrap_or("");
  let path = suffix.split('?').next().unwrap_or("");
  if matches!(
    site,
    Some(SiteId::GoogleDrive | SiteId::GoogleDocs | SiteId::Dropbox)
  ) && !share
  {
    return None;
  }
  // Private Telegram invites and private channels never become a background request.
  if site == Some(SiteId::Telegram)
    && (path.starts_with("/+") || path.starts_with("/joinchat") || path.starts_with("/c/"))
  {
    return None;
  }
  // Generic lnkd.in redirects can leave LinkedIn; capture only its post short links.
  if crate::sites::host_is(host, "lnkd.in") && !path.starts_with("/p/") {
    return None;
  }
  // Public proxy aliases are exact hosts; subdomains remain manual-only.
  if crate::sites::SITES
    .iter()
    .flat_map(|s| s.short_links)
    .any(|alias| {
      matches!(*alias, "fxtwitter.com" | "vxtwitter.com") && crate::sites::host_is(host, alias)
    })
  {
    return None;
  }
  Some((original.to_string(), host.into()))
}

/// True for a site's home/feed page (no specific post). These get no metadata fetch.
/// Expects a URL already normalized by `supported_url`.
pub fn is_feed_page(url: &str) -> bool {
  let Ok(normalized) = crate::links::normalize::pure(url) else {
    return false;
  };
  let Some((_, rest)) = normalized.as_str().split_once("://") else {
    return false;
  };
  let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
  let host = &rest[..end];
  let path = rest[end..].split(['?', '#']).next().unwrap_or("");
  let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
  let first = segments.first().copied().unwrap_or("");
  let site = site_by_domain(host);
  if segments.is_empty() {
    return site.is_some();
  }
  match site {
    Some(SiteId::LinkedIn) => first == "feed" && !path.contains("urn:li:"),
    Some(SiteId::Facebook) => segments.is_empty() || (segments.len() == 1 && first == "home"),
    Some(SiteId::Instagram) => segments.is_empty(),
    Some(SiteId::X) => segments.len() == 1 && first == "home",
    Some(SiteId::TikTok) => segments.is_empty() || (segments.len() == 1 && first == "foryou"),
    Some(SiteId::Reddit) => {
      segments.is_empty()
        // `/r/<sub>/comments/…` is a post; `/r/<sub>/s/<token>` is Reddit's Share → Copy link form.
        || (first == "r"
          && segments.len() >= 2
          && segments.get(2).is_none_or(|s| !matches!(*s, "comments" | "s")))
    }
    Some(SiteId::YouTube) => segments.is_empty() || first == "feed",
    Some(SiteId::Tumblr) => matches!(first, "dashboard" | "explore" | "tagged" | "search"),
    Some(SiteId::Kick) => {
      segments.len() == 1 && matches!(first, "browse" | "categories" | "following")
    }
    Some(SiteId::Vk | SiteId::Ok) => segments.len() == 1 && first == "feed",
    Some(SiteId::Imgur) => segments.len() == 1 && matches!(first, "hot" | "new" | "top"),
    Some(SiteId::NineGag) => segments.len() == 1 && matches!(first, "hot" | "fresh" | "trending"),
    Some(SiteId::Flickr) => segments.len() == 1 && first == "explore",
    _ => false,
  }
}

#[derive(Default)]
pub struct CapturePolicy {
  last: Option<u64>,
  seen: HashMap<u64, Duration>,
  own: HashMap<u64, Duration>,
}

fn fingerprint(text: &str) -> u64 {
  let mut hash = DefaultHasher::new();
  text.hash(&mut hash);
  hash.finish()
}

fn link_fingerprint(url: &str) -> u64 {
  fingerprint(
    crate::links::normalize::canonical_key(url)
      .as_deref()
      .unwrap_or(url),
  )
}

fn watched_urls(text: &str) -> Option<Vec<String>> {
  // Keep the single-link path, including its handling of directional marks around the URL.
  if let Some((url, _)) = supported_url(text) {
    return Some(vec![url]);
  }
  text
    .split_whitespace()
    .map(|part| supported_url(part).map(|(url, _)| url))
    .collect::<Option<Vec<_>>>()
    .filter(|urls| !urls.is_empty())
}

impl CapturePolicy {
  pub fn own_write(&mut self, text: &str, now: Duration) {
    if text.len() > MAX_CLIPBOARD_BYTES {
      return;
    }
    for url in watched_urls(text).into_iter().flatten() {
      self.own.insert(link_fingerprint(&url), now);
    }
  }

  #[cfg(test)]
  pub fn observe(&mut self, text: &str, now: Duration) -> Option<Vec<String>> {
    let urls = self.decide(text, now).offer()?;
    self.acknowledge(text, &urls, now);
    Some(urls)
  }

  /// Consume just the links delivered for this clipboard read.
  pub fn acknowledge(&mut self, text: &str, urls: &[String], now: Duration) {
    self.last = Some(fingerprint(text));
    for url in urls {
      self.seen.insert(link_fingerprint(url), now);
    }
  }

  /// Startup is an observation, not a copy or an offered card. A later changed clipboard may offer it.
  pub fn baseline(&mut self, text: &str) {
    self.last = Some(fingerprint(text));
  }

  pub fn decide(&mut self, text: &str, now: Duration) -> Decision {
    if text.len() > MAX_CLIPBOARD_BYTES {
      self.last = None;
      return Decision::TooLong;
    }
    let raw_hash = fingerprint(text);
    if self.last == Some(raw_hash) {
      return Decision::Unchanged;
    }
    self.last = Some(raw_hash);
    let ttl = Duration::from_secs(600);
    self.seen.retain(|_, at| now.saturating_sub(*at) < ttl);
    self.own.retain(|_, at| now.saturating_sub(*at) < ttl);
    let Some(mut urls) = watched_urls(text) else {
      return Decision::Unsupported;
    };
    let mut unique = HashSet::new();
    let mut suppressed = Decision::Seen;
    urls.retain(|url| {
      let key = link_fingerprint(url);
      if self.own.contains_key(&key) {
        suppressed = Decision::Own;
        return false;
      }
      !self.seen.contains_key(&key) && unique.insert(key)
    });
    if urls.is_empty() {
      return suppressed;
    }
    // Keep the copy eligible until the watcher confirms delivery.
    self.last = None;
    Decision::Offer(urls)
  }
}

/// What the watcher made of one clipboard read; `kind` is the content-free label for diagnostics.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
  Unchanged,
  TooLong,
  Unsupported,
  Own,
  Seen,
  Offer(Vec<String>),
}

impl Decision {
  pub fn kind(&self) -> &'static str {
    match self {
      Self::Unchanged => "unchanged",
      Self::TooLong => "too-long",
      Self::Unsupported => "unsupported",
      Self::Own => "own-write",
      Self::Seen => "already-offered",
      Self::Offer(..) => "offer",
    }
  }

  pub fn offer(self) -> Option<Vec<String>> {
    match self {
      Self::Offer(urls) => Some(urls),
      _ => None,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  const URL: &str = "https://www.youtube.com/watch?v=abc";

  #[test]
  fn watches_only_lists_of_supported_links() {
    const SECOND: &str = "https://vimeo.com/42";
    for (separator, middle, expected) in [
      ("\n", "", Some(vec![URL, SECOND])),
      ("\r\n", "", Some(vec![URL, SECOND])),
      (" \t", "", Some(vec![URL, SECOND])),
      ("\n", "http://192.168.1.1/\n", None),
      ("\n", "ftp://vimeo.com/42\n", None),
      ("\n", "look at this\n", None),
    ] {
      let mut policy = CapturePolicy::default();
      let text = format!("{URL}{separator}{middle}{SECOND}");
      assert_eq!(
        policy.observe(&text, Duration::ZERO),
        expected.map(|urls| urls.into_iter().map(str::to_owned).collect()),
        "clipboard separator {separator:?}, middle {middle:?}"
      );
      assert!(policy.observe(&text, Duration::ZERO).is_none());
      if middle.is_empty() {
        assert_eq!(policy.decide(SECOND, Duration::ZERO), Decision::Seen);
      }
    }
  }

  #[test]
  fn watches_each_canonical_link_once_without_consuming_other_copies() {
    let mut policy = CapturePolicy::default();
    let own = "https://vimeo.com/42";
    let other_own = "https://vimeo.com/43";
    policy.own_write(&format!("{own}\n{other_own}"), Duration::ZERO);
    let text = format!("{own}\n{URL}\nhttps://youtu.be/abc?si=other\n{other_own}");
    assert_eq!(
      policy.observe(&text, Duration::ZERO),
      Some(vec![URL.into()])
    );
    assert_eq!(policy.decide(own, Duration::ZERO), Decision::Own);
    assert_eq!(policy.decide(URL, Duration::ZERO), Decision::Seen);
  }

  #[test]
  fn wrappers_and_shorteners_are_captured_with_original_urls_and_canonical_identity() {
    let wrapped =
      "https://l.facebook.com/l.php?u=https%3A%2F%2Fyoutube.com%2Fwatch%3Fv%3Dabc%26si%3Dx";
    assert_eq!(
      supported_url(wrapped),
      Some((wrapped.into(), "youtube.com".into()))
    );
    let mut policy = CapturePolicy::default();
    assert!(policy.observe(wrapped, Duration::ZERO).is_some());
    assert_eq!(
      policy.decide("https://youtu.be/abc?si=other", Duration::ZERO),
      Decision::Seen
    );
    for host in crate::links::normalize::SHORTENERS
      .iter()
      .filter(|host| **host != "lnkd.in")
    {
      assert!(
        supported_url(&format!("https://{host}/abc")).is_some(),
        "{host}"
      );
    }
    for host in [
      "https://bit.ly.evil.test/a",
      "https://d.fxtwitter.com/u/status/1",
    ] {
      assert!(supported_url(host).is_none(), "{host}");
    }
    assert!(is_feed_page(
      "https://google.com/url?q=https%3A%2F%2Fyoutube.com"
    ));
  }

  #[test]
  fn only_a_link_the_user_named_may_be_any_public_page() {
    let page = "https://movies4us.net/watch/movie-the-intouchables-nw1ii";
    // Passive watching keeps the site list; the quick bar takes the page as it is.
    assert!(accepted_url(page, Source::Watch).is_none());
    assert_eq!(
      accepted_url(page, Source::Explicit),
      Some((page.into(), "movies4us.net".into()))
    );
    assert!(accepted_url("http://Example.com/a?b=1", Source::Explicit).is_some());
    // Listed sites read the same either way.
    assert_eq!(
      accepted_url("https://youtu.be/abc", Source::Explicit),
      supported_url("https://youtu.be/abc")
    );
    for refused in [
      "http://localhost/x",
      "http://localhost:3000/x",
      "http://127.0.0.1/x",
      "http://192.168.1.10/video",
      "http://10.0.0.2/",
      "http://[::1]/x",
      "http://[fd00::1]/x",
      "https://router.local/x",
      "https://nas.home.arpa/x",
      "https://intranet/x",
      "https://user:pw@example.com/x",
      "https://example.com:8443/x",
      "file:///etc/passwd",
      "javascript:alert(1)",
      "ftp://example.com/a",
      "data:text/html,hi",
      // Typed text without a scheme stays a known-site shortcut only.
      "example.com/watch",
      "hello",
      // A known site's own refusals stand.
      "https://t.me/+secret",
      "https://t.me/c/123/4",
      "https://drive.google.com/drive/folders/abc",
      "https://lnkd.in/abc",
    ] {
      assert!(
        accepted_url(refused, Source::Explicit).is_none(),
        "{refused}"
      );
    }
  }

  #[test]
  fn linkedin_capture_accepts_only_post_short_links() {
    assert!(supported_url("https://lnkd.in/p/abc").is_some());
    assert!(supported_url("https://lnkd.in/abc").is_none());
  }

  #[test]
  fn startup_text_is_not_a_copy_but_later_changed_text_is() {
    let mut policy = CapturePolicy::default();
    policy.baseline(URL);
    assert_eq!(policy.decide(URL, Duration::ZERO), Decision::Unchanged);
    // A different copy works; clearing/non-link text also makes a fresh copy of the original eligible.
    let other = "https://youtu.be/different";
    assert!(policy.decide(other, Duration::ZERO).offer().is_some());
    policy.decide("plain text", Duration::ZERO);
    assert!(policy.decide(URL, Duration::ZERO).offer().is_some());
  }

  #[test]
  fn normalization_keeps_share_keys_and_rejects_embedded_marks_and_nonshares() {
    for mark in [
      '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}',
      '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}', '\u{200b}', '\u{2060}',
      '\u{feff}',
    ] {
      assert_eq!(
        supported_url(&format!("{mark}youtu.be/abc?si=x&t=10{mark}"))
          .unwrap()
          .0,
        "https://youtu.be/abc?si=x&t=10"
      );
    }
    assert_eq!(
      supported_url("dropbox.com/scl/fi/abc/report.pdf?rlkey=key&dl=0")
        .unwrap()
        .0,
      "https://dropbox.com/scl/fi/abc/report.pdf?rlkey=key&dl=0"
    );
    for raw in [
      "https://you\u{200f}tube.com/watch?v=a",
      "https://pixeldrain.com/",
      "https://mediafire.com/folder/abc",
      "https://github.com/owner/repo",
      "https://d.fxtwitter.com/nasa/status/1",
      "https://fxtwitter.com.evil.test/nasa/status/1",
    ] {
      assert!(supported_url(raw).is_none(), "{raw}");
    }
    assert!(is_feed_page(&supported_url("youtube.com").unwrap().0));
    // Private Telegram messages remain manual-only; they must not trigger the watcher.
    assert!(supported_url("https://t.me/c/123456/7").is_none());
    assert!(!crate::extract::registry::stages_for("https://t.me/c/123456/7").is_empty());
  }

  #[test]
  fn url_patterns_reach_their_engine_after_clipboard_normalization() {
    use crate::extract::registry::{readers, stages_for};
    use crate::routing::StageId;
    let mut failures = Vec::new();
    for row in include_str!("../extract/fixtures/url-patterns.tsv").lines() {
      if row.starts_with('#') || row.is_empty() {
        continue;
      }
      let fields: Vec<_> = row.split('\t').collect();
      let [site, shape, input, engine, reader, ..] = fields.as_slice() else {
        panic!("bad fixture: {row}");
      };
      if *engine == "unsupported" {
        assert!(
          supported_url(input).is_none(),
          "unsupported site must not be captured: {input}"
        );
        continue;
      }
      let Some((url, host)) = supported_url(input) else {
        failures.push(format!("{site}/{shape}: clipboard rejected"));
        continue;
      };
      let detected = crate::sites::site_of(&host).map_or("-", |s| s.site().key);
      if detected != *site {
        failures.push(format!("{shape}: site {detected}, expected {site}"));
      }
      let routed = crate::links::normalize::pure(&url).unwrap();
      let stages = stages_for(routed.as_str());
      let want = match *engine {
        "files" => StageId::Files,
        "gallery" => StageId::Gallery,
        "ytdlp" => StageId::Ytdlp,
        _ => panic!("bad engine"),
      };
      // YouTube is exclusive; other post routes intentionally start with the I/O-free Files gate.
      let prefix = if *site == "youtube" || want == StageId::Files {
        vec![want]
      } else {
        vec![StageId::Files, want]
      };
      if !stages.starts_with(&prefix) {
        failures.push(format!(
          "{shape}: stages {stages:?}, expected prefix {prefix:?}"
        ));
      }
      if is_feed_page(&url) {
        failures.push(format!("{shape}: post flagged as feed"));
      }
      let actual = readers(routed.as_str())
        .first()
        .map_or_else(|| "-".to_owned(), |r| format!("{r:?}"));
      if actual != *reader {
        failures.push(format!("{shape}: reader {actual}, expected {reader}"));
      }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
  }

  #[test]
  fn accepts_supported_hosts_only_and_normalizes_for_dedupe() {
    assert_eq!(
      supported_url(" HTTPS://WWW.YouTube.com/watch?v=abc#t=2 "),
      Some((format!("{URL}#t=2"), "www.youtube.com".into()))
    );
    assert!(supported_url("https://m.youtube.com/watch?v=abc").is_some());
    // Links the share buttons of post sites copy.
    for text in [
      "https://lnkd.in/p/eYKXMxJS",
      "https://www.linkedin.com/posts/someone_activity-1-abc",
      "https://www.threads.com/@a/post/b",
      "https://bsky.app/profile/a/post/b",
      "https://t.me/channel/12",
      "https://pin.it/abc",
      "https://www.dropbox.com/s/abc/report.pdf?dl=0",
      "dropbox.com/s/abc/report.pdf?dl=0",
      "http://drive.google.com/file/d/abc/view",
      "https://docs.google.com/document/d/abc/edit",
      "https://www.snapchat.com/spotlight/abc",
      "https://x.com/a/status/1/photo/1",
      // Every host of a site counts (sites.rs), embeds included.
      "https://www.youtube-nocookie.com/embed/abc",
    ] {
      assert!(supported_url(text).is_some(), "rejected share link: {text}");
    }
    for text in [
      "hello",
      "file:///youtube.com/a",
      "javascript:youtube.com",
      "https://youtube.com.evil.test/a",
      "https://notyoutube.com/a",
      "https://youtube.com@evil.test/a",
      "https://evil.test/youtube.com",
      "https://youtube.com\\@evil.test/a",
      "https://youtube.com/a another secret",
      "https://127.0.0.1/a",
      "https://youtube.com:99999/a",
      "https://foo..youtube.com/a",
      "https://youtube.com/\nprivate",
      "https://t.me/+AbCdEf123",
      "https://t.me/joinchat/AbCdEf",
      "https://t.me/c/123456/7",
      "https://drive.google.com/drive/folders/abc",
      "https://docs.google.com/forms/d/abc/viewform",
      "https://dropbox.com/home",
      "https://sub.dropbox.com/s/abc/report.pdf",
      "https://www.kwai.com/",
    ] {
      assert!(
        supported_url(text).is_none(),
        "unexpectedly accepted fixture: {text}"
      );
    }
  }

  #[test]
  fn a_skipped_or_failed_card_does_not_consume_the_clipboard_link() {
    let mut policy = CapturePolicy::default();
    assert!(matches!(
      policy.decide(URL, Duration::ZERO),
      Decision::Offer(..)
    ));
    // A quick bar was open, or the native card failed to show: no delivery acknowledgement.
    assert!(matches!(
      policy.decide(URL, Duration::from_millis(500)),
      Decision::Offer(..)
    ));
    policy.decide("another copy", Duration::from_secs(1));
    assert!(matches!(
      policy.decide(URL, Duration::from_secs(2)),
      Decision::Offer(..)
    ));
  }

  #[test]
  fn a_shown_card_stays_acknowledged_after_dismiss_or_action_and_refocus() {
    let mut policy = CapturePolicy::default();
    assert!(policy.observe(URL, Duration::ZERO).is_some());
    // Dismiss/download does not reset the clipboard policy; refocus must not resurrect the card.
    assert_eq!(
      policy.decide(URL, Duration::from_secs(1)),
      Decision::Unchanged
    );
    policy.decide("another copy", Duration::from_secs(2));
    assert_eq!(policy.decide(URL, Duration::from_secs(3)), Decision::Seen);
    assert!(matches!(
      policy.decide("https://youtu.be/second", Duration::from_secs(4)),
      Decision::Offer(..)
    ));
  }

  #[test]
  fn dedupes_urls_for_ten_minutes_even_after_other_copies() {
    let mut policy = CapturePolicy::default();
    assert!(policy.observe(URL, Duration::ZERO).is_some());
    assert!(policy.observe(URL, Duration::from_secs(1)).is_none());
    policy.observe("unrelated text", Duration::from_secs(2));
    assert!(policy.observe(URL, Duration::from_secs(599)).is_none());
    policy.observe("unrelated text", Duration::from_secs(600));
    assert!(policy.observe(URL, Duration::from_secs(600)).is_some());
  }

  #[test]
  fn self_write_suppression_survives_intervening_copies_but_expires() {
    let mut policy = CapturePolicy::default();
    policy.own_write(URL, Duration::ZERO);
    policy.observe("another copy", Duration::from_secs(1));
    // Diagnostics label each read without its text.
    let mut fresh = CapturePolicy::default();
    let kinds: Vec<_> = [URL, URL, "hello", URL]
      .iter()
      .map(|text| {
        let decision = fresh.decide(text, Duration::ZERO);
        if let Decision::Offer(urls) = &decision {
          fresh.acknowledge(text, urls, Duration::ZERO);
        }
        decision.kind()
      })
      .collect();
    assert_eq!(
      kinds,
      ["offer", "unchanged", "unsupported", "already-offered"]
    );
    assert_eq!(
      fresh.decide(&"x".repeat(9000), Duration::ZERO).kind(),
      "too-long"
    );
    assert!(policy.observe(URL, Duration::from_secs(2)).is_none());
    policy.observe("another copy", Duration::from_secs(601));
    assert!(policy.observe(URL, Duration::from_secs(601)).is_some());
  }

  #[test]
  fn feed_pages_are_flagged_and_posts_are_not() {
    for url in [
      "kick.com",
      "https://vk.com/",
      "https://imgur.com/?utm_source=share",
      "https://www.tumblr.com/",
      "https://ok.ru/",
      "https://9gag.com/",
      "https://likee.video/",
      "https://www.flickr.com/",
      "https://streamable.com/",
      "https://www.tumblr.com/dashboard",
      "https://kick.com/browse",
      "https://vk.com/feed",
      "https://ok.ru/feed",
      "https://imgur.com/hot",
      "https://9gag.com/fresh",
      "https://www.flickr.com/explore/",
      "https://www.linkedin.com/feed/",
      "https://www.linkedin.com/feed/?trk=x",
      "https://www.facebook.com/",
      "https://www.facebook.com/home",
      "https://www.instagram.com/",
      "https://x.com/home",
      "https://twitter.com/home",
      "https://www.tiktok.com/",
      "https://www.tiktok.com/foryou",
      "https://www.reddit.com/",
      "https://www.reddit.com/r/rust/",
      "https://www.reddit.com/r/rust",
      "https://www.reddit.com/r/rust/hot/",
      "https://www.reddit.com/r/rust/top/?t=week",
      "https://www.youtube.com/",
      "https://www.youtube.com/feed/subscriptions",
    ] {
      let (url, _) = supported_url(url).unwrap();
      assert!(is_feed_page(&url), "not flagged as feed: {url}");
    }
    for url in [
      "https://www.tumblr.com/nasa/820337874767675392/",
      "https://kick.com/clip/clip_123",
      "https://vk.com/video-1_2",
      "https://ok.ru/video/123",
      "https://imgur.com/gallery/abc",
      "https://9gag.com/gag/abc",
      "https://likee.video/@user/video/123",
      "https://www.flickr.com/photos/gsfc/55529808082/",
      "https://streamable.com/abc",
      "https://www.linkedin.com/feed/update/urn:li:activity:123/",
      "https://www.linkedin.com/posts/someone_activity-1-abc",
      "https://www.facebook.com/watch/?v=1",
      "https://www.facebook.com/user/posts/1",
      "https://www.instagram.com/reel/abc/",
      "https://www.instagram.com/p/abc/",
      "https://x.com/a/status/1",
      "https://www.tiktok.com/@a/video/1",
      "https://www.reddit.com/r/rust/comments/abc/title/",
      "https://www.reddit.com/r/soccer/s/AbC123xyz",
      "https://www.youtube.com/watch?v=abc",
      "https://www.youtube.com/shorts/abc",
      "https://youtu.be/abc",
    ] {
      let (url, _) = supported_url(url).unwrap();
      assert!(!is_feed_page(&url), "post flagged as feed: {url}");
    }
  }
}
