//! Admission and per-window deduplication for untrusted browser observations.

use super::manifest::{looks_like_dash, Variant, MAX_HLS_VARIANTS};
use crate::net;
use icu_properties::{props::BidiControl, CodePointSetData};
use indexmap::IndexMap;
use percent_encoding::percent_decode_str;
use reqwest::Url;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::time::Instant;

const MAX_URL_BYTES: usize = 8 * 1024;
const MAX_CONTENT_TYPE_BYTES: usize = 256;
const MAX_CANDIDATES: usize = 200;

/// Prefer longer media. Host evidence breaks ties within the same rounded second.
pub(super) fn compare_duration_and_host(a: (Option<f64>, u8), b: (Option<f64>, u8)) -> Ordering {
  let (a_sec, b_sec) = (a.0.unwrap_or_default(), b.0.unwrap_or_default());
  a_sec
    .round()
    .total_cmp(&b_sec.round())
    .then_with(|| a.1.cmp(&b.1))
    .then_with(|| a_sec.total_cmp(&b_sec))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
  Hls,
  Dash,
  Direct,
  Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
  Performance,
  Fetch,
  Xhr,
  MediaElement,
  Native,
}

/// One hit from a browser hook. Frame information and media hints come from untrusted input.
pub struct Observation<'a> {
  pub url: &'a str,
  /// Original playlist request for a successful fetch/XHR redirect.
  pub requested_url: Option<&'a str>,
  pub content_type: Option<&'a str>,
  pub frame_url: &'a str,
  pub frame_origin: &'a str,
  pub source: Source,
  /// Timestamp assigned by the Rust receiver.
  pub first_seen: Instant,
  /// An optional body prefix supplied by a caller that already has it.
  pub first_bytes: &'a [u8],
  /// Whether the caller identified this hit as byte-range media.
  pub is_byte_range: bool,
  /// Whether playlist or hook context identifies this URL as an HLS segment.
  pub is_hls_segment: bool,
  /// The page received a successful network response for this address.
  pub loaded: bool,
  /// A capability probe reported by a hook.
  pub eme_probe: bool,
  /// Active encrypted-media use reported by a hook.
  pub eme_active: bool,
}

/// An admitted URL and its observed context. Frame metadata remains untrusted.
#[derive(Clone)]
pub struct Candidate {
  url: Url,
  redirected_url: Option<Url>,
  pub kind: Kind,
  pub content_type: Option<String>,
  pub frame_url: String,
  /// Can be the literal "null" for an opaque sandboxed frame.
  pub frame_origin: String,
  pub source: Source,
  pub first_seen: Instant,
  pub eme_probe: bool,
  pub eme_active: bool,
  /// Set only for a page address Nezzel itself vetted (`page_video`): the yt-dlp extractors
  /// that read it. Everything a page showed us stays on the generic extractor.
  site: Option<&'static str>,
}

impl Candidate {
  /// The normalized request URL used as the candidate's stable identity.
  pub fn url(&self) -> &Url {
    &self.url
  }

  pub(super) fn download_url(&self) -> &Url {
    self.redirected_url.as_ref().unwrap_or(&self.url)
  }

  /// Whether active EME is a DRM signal for this candidate.
  pub fn has_drm(&self) -> bool {
    self.eme_active
  }

  pub(super) fn site(&self) -> Option<&'static str> {
    self.site
  }

  /// A vetted single-video page of a site the normal download path reads, read the same way.
  pub(super) fn site_page(url: Url, extractors: &'static str) -> Self {
    Self {
      site: Some(extractors),
      ..Self::page(url)
    }
  }

  /// The page address itself as what to save: yt-dlp reads the page (the retry ladder's last step).
  pub fn page(mut url: Url) -> Self {
    // A fragment names a place in the page, not another video.
    url.set_fragment(None);
    Self {
      frame_url: url.to_string(),
      frame_origin: url.origin().ascii_serialization(),
      url,
      redirected_url: None,
      kind: Kind::Direct,
      content_type: None,
      source: Source::Native,
      first_seen: Instant::now(),
      eme_probe: false,
      eme_active: false,
      site: None,
    }
  }
}

/// Candidates in discovery order, with known HLS children excluded.
#[derive(Default)]
pub struct CandidateSet {
  candidates: IndexMap<Url, Candidate>,
  hls_variants: HashMap<Url, HashSet<Url>>,
  loaded_hosts: HashMap<(String, String), u8>,
}

impl CandidateSet {
  /// Adds a candidate made here, not from a hook (refused at capacity).
  pub fn insert(&mut self, candidate: Candidate) -> bool {
    if self.candidates.len() >= MAX_CANDIDATES && !self.candidates.contains_key(&candidate.url) {
      return false;
    }
    self.candidates.insert(candidate.url.clone(), candidate);
    true
  }

  /// Admit or enrich a hit. Refuse oversized metadata and new URLs at capacity.
  pub fn observe(&mut self, observation: Observation<'_>) -> Option<&Candidate> {
    if observation.frame_url.len() > MAX_URL_BYTES
      || observation.frame_origin.len() > MAX_URL_BYTES
      || observation
        .content_type
        .is_some_and(|value| value.len() > MAX_CONTENT_TYPE_BYTES)
    {
      return None;
    }

    let url = normalize_url(observation.url, None)?;
    let suffix = extension(&url);
    let mime = observation.content_type.map_or_else(String::new, |raw| {
      raw
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
    });
    let segment = observation.is_byte_range
      || observation.is_hls_segment
      || matches!(suffix.as_str(), "ts" | "m4s")
      || matches!(
        mime.as_str(),
        "video/mp2t" | "video/iso.segment" | "audio/iso.segment"
      );
    if segment || suffix == "vtt" || mime == "text/vtt" {
      if observation.loaded && segment {
        self.note_loaded_host(&url, observation.frame_url, 2);
      }
      return None;
    }

    let bytes_kind = sniff_manifest(observation.first_bytes);
    let kind = bytes_kind.unwrap_or_else(|| classify(&mime, &suffix));
    // A playlist may be served as JavaScript or another generic type after a redirect.
    let (url, redirected_url, kind) = match observation
      .requested_url
      .filter(|_| {
        matches!(observation.source, Source::Fetch | Source::Xhr)
          && url.scheme() == "https"
          && (kind == Kind::Hls
            || (kind == Kind::Unknown
              && matches!(
                mime.as_str(),
                "" | "application/javascript"
                  | "text/javascript"
                  | "application/xml"
                  | "text/xml"
                  | "application/octet-stream"
                  | "text/plain"
              )))
      })
      .and_then(|requested| normalize_url(requested, None))
      .filter(|requested| classify("", &extension(requested)) == Kind::Hls)
    {
      Some(requested) if requested != url => (requested, Some(url), Kind::Hls),
      _ => (url, None, kind),
    };
    if observation.loaded && kind != Kind::Unknown {
      self.note_loaded_host(
        redirected_url.as_ref().unwrap_or(&url),
        observation.frame_url,
        if kind == Kind::Direct { 2 } else { 1 },
      );
    }
    if (self.candidates.len() >= MAX_CANDIDATES && !self.candidates.contains_key(&url))
      || self.hls_variants.values().any(|variants| {
        variants.contains(&url)
          || redirected_url
            .as_ref()
            .is_some_and(|target| variants.contains(target))
      })
    {
      return None;
    }
    let candidate = self
      .candidates
      .entry(url.clone())
      .or_insert_with(|| Candidate {
        url,
        redirected_url: None,
        kind,
        content_type: observation.content_type.map(str::to_owned),
        frame_url: observation.frame_url.into(),
        frame_origin: observation.frame_origin.into(),
        source: observation.source,
        first_seen: observation.first_seen,
        eme_probe: observation.eme_probe,
        eme_active: observation.eme_active,
        site: None,
      });
    if let Some(target) = redirected_url {
      if candidate.frame_url != observation.frame_url
        || candidate.frame_origin != observation.frame_origin
      {
        return None;
      }
      candidate.redirected_url = Some(target);
    }
    candidate.eme_probe |= observation.eme_probe;
    candidate.eme_active |= observation.eme_active;
    if kind != Kind::Unknown
      && (bytes_kind.is_some()
        || candidate.kind == Kind::Unknown
        || (candidate.kind == Kind::Direct && matches!(kind, Kind::Hls | Kind::Dash)))
    {
      candidate.kind = kind;
      if let Some(content_type) = observation.content_type {
        candidate.content_type = Some(content_type.into());
      }
    } else if candidate.content_type.is_none() {
      candidate.content_type = observation.content_type.map(str::to_owned);
    }
    Some(candidate)
  }

  pub fn iter(&self) -> impl Iterator<Item = &Candidate> {
    self.candidates.values()
  }

  fn note_loaded_host(&mut self, url: &Url, frame: &str, evidence: u8) {
    if let Some(host) = url.host_str() {
      let key = (frame.to_owned(), host.to_owned());
      if self.loaded_hosts.len() < MAX_CANDIDATES || self.loaded_hosts.contains_key(&key) {
        let previous = self.loaded_hosts.entry(key).or_default();
        *previous = (*previous).max(evidence);
      }
    }
  }

  pub(super) fn loaded_host(&self, candidate: &Candidate) -> u8 {
    candidate.download_url().host_str().map_or(0, |host| {
      self
        .loaded_hosts
        .get(&(candidate.frame_url.clone(), host.to_owned()))
        .copied()
        .unwrap_or_default()
    })
  }

  /// Refresh the inspected master's child references and remove matching hits.
  /// Keep up to 200 master lists per window.
  pub fn suppress_hls_variants(&mut self, master: &Url, variants: &[Variant]) {
    let Some(master) = normalize_url(master.as_str(), None) else {
      return;
    };
    if self.hls_variants.len() >= MAX_CANDIDATES && !self.hls_variants.contains_key(&master) {
      return;
    }
    let urls: HashSet<_> = variants
      .iter()
      .take(MAX_HLS_VARIANTS)
      .filter_map(|variant| normalize_url(&variant.uri, Some(&master)))
      // A cross-host child can be the only address the browser successfully loaded.
      .filter(|url| url != &master && url.host_str() == master.host_str())
      .collect();
    for url in &urls {
      self.candidates.shift_remove(url);
    }
    if urls.is_empty() {
      self.hls_variants.remove(&master);
    } else {
      self.hls_variants.insert(master, urls);
    }
  }
}

fn extension(url: &Url) -> String {
  let path = url.path().rsplit('/').next().unwrap_or_default();
  let filename = percent_decode_str(path).decode_utf8_lossy();
  filename
    .rsplit_once('.')
    .map_or_else(String::new, |(_, ext)| ext.to_ascii_lowercase())
}

/// Perform syntax and literal-address checks without resolving hostnames.
/// Any U3 fetch must resolve the host, check the resolved addresses through
/// [`crate::net::admit`], and connect using the admitted addresses.
pub(super) fn normalize_url(raw: &str, base: Option<&Url>) -> Option<Url> {
  let bidi = CodePointSetData::new::<BidiControl>();
  let forbidden = |c: char| c.is_control() || bidi.contains(c);
  if raw.len() > MAX_URL_BYTES
    || raw.trim() != raw
    || raw.contains('\\')
    || raw.chars().any(forbidden)
    || (raw.contains('%')
      && percent_decode_str(raw)
        .decode_utf8_lossy()
        .chars()
        .any(forbidden))
  {
    return None;
  }

  // URL serialization can erase an empty userinfo marker. Check the raw authority as well.
  let (authority, mut url) = match raw.split_once(':') {
    Some((scheme, rest))
      if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") =>
    {
      (Some(rest.strip_prefix("//")?), Url::parse(raw).ok()?)
    }
    _ => (raw.strip_prefix("//"), base?.join(raw).ok()?),
  };
  if authority.is_some_and(|rest| {
    rest
      .split(['/', '?', '#'])
      .next()
      .is_none_or(|authority| authority.is_empty() || authority.contains('@'))
  }) || !matches!(url.scheme(), "http" | "https")
    || !url.username().is_empty()
    || url.password().is_some()
  {
    return None;
  }

  // The URL parser canonicalizes decimal, hexadecimal, and octal IPv4 before this check.
  let host = net::host(&url)?;
  let domain = host.trim_end_matches('.');
  if domain == "localhost"
    || domain.ends_with(".localhost")
    || host
      .parse::<IpAddr>()
      .is_ok_and(|ip| !net::public_address(ip))
  {
    return None;
  }

  url.set_fragment(None);
  (url.as_str().len() <= MAX_URL_BYTES).then_some(url)
}

fn sniff_manifest(bytes: &[u8]) -> Option<Kind> {
  let prefix = &bytes[..bytes.len().min(4096)];
  let prefix = prefix
    .strip_prefix(b"\xef\xbb\xbf")
    .unwrap_or(prefix)
    .trim_ascii_start();
  if prefix
    .strip_prefix(b"#EXTM3U")
    .is_some_and(|rest| rest.first().is_none_or(u8::is_ascii_whitespace))
  {
    Some(Kind::Hls)
  } else if looks_like_dash(prefix) {
    Some(Kind::Dash)
  } else {
    None
  }
}

fn classify(mime: &str, extension: &str) -> Kind {
  match (mime, extension) {
    (
      "application/vnd.apple.mpegurl"
      | "application/x-mpegurl"
      | "application/mpegurl"
      | "audio/mpegurl"
      | "audio/x-mpegurl",
      _,
    ) => Kind::Hls,
    ("application/dash+xml", _) => Kind::Dash,
    (_, "m3u8" | "m3u") => Kind::Hls,
    (_, "mpd") => Kind::Dash,
    ("application/mp4" | "application/ogg", _)
    | (
      _,
      "mp4" | "webm" | "m4v" | "mov" | "mkv" | "avi" | "aac" | "mp3" | "m4a" | "ogg" | "oga"
      | "ogv" | "opus" | "wav" | "flac" | "flv" | "wmv",
    ) => Kind::Direct,
    (mime, _) if mime.starts_with("video/") || mime.starts_with("audio/") => Kind::Direct,
    _ => Kind::Unknown,
  }
}

#[cfg(test)]
#[path = "candidate_tests.rs"]
mod tests;
