//! Per-document capture state. Remote observations have no authority over browser or IPC state.

use super::{
  candidate::{compare_duration_and_host, Candidate, CandidateSet, Kind, Observation, Source},
  handoff::{CatchFormat, Handoff},
  manifest::ManifestInfo,
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::{
  collections::{HashMap, HashSet},
  sync::Arc,
  time::{Duration, Instant},
};
use uuid::Uuid;

const MAX_DRAIN_BYTES: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 500;
const MAX_URL_BYTES: usize = 8 * 1024;
const MAIN_DURATION_SEC: f64 = 60.0;
/// A main this long that has not played is a film or episode: a short player beside it (a
/// looping hero, a promo) does not take its place. News neighbours are shorter.
const FILM_SEC: f64 = 600.0;
/// Playback after which the only, short video on a page is offered as its main video.
const SHORT_ONLY_PLAYED_SEC: f64 = 15.0;
/// Removed players and discarded frames can stop reporting without a pause event.
const ACTIVE_PLAYER_TTL: Duration = Duration::from_secs(3);
/// Media elements a document tracks for EME evidence; more are ignored.
const MAX_ELEMENTS: usize = 64;
/// Retries before an encrypted verdict is shown (owner 2026-11-25: try harder) end by this.
pub(super) const LADDER_CAP: Duration = Duration::from_secs(40);
/// How long the Mobile version gets to show a video that plays here.
const MOBILE_WAIT: Duration = Duration::from_secs(15);
/// How long restoring the tab's own identity may take before the next step starts anyway.
const RESTORE_WAIT: Duration = Duration::from_secs(5);
/// Saved media this window remembers, to mark "already saved" across pages.
pub(super) const MAX_SAVED_URLS: usize = 500;
/// The most page text the geo-block check reads (`geo::blocked`).
const GEO_TEXT_CHARS: usize = 2000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum Phase {
  #[default]
  Looking,
  AdPlaying,
  Found,
  Drm,
  Live,
  NotFound,
  Saving,
  /// The page's renderer died again right after an automatic reload: the bar offers Reload.
  Crashed,
  /// A main-frame load failed before WebKit could show the requested page.
  LoadFailed,
  /// The site shows its "not available in your country" page (`geo::blocked`).
  GeoBlocked,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CatchCandidate {
  id: String,
  kind: MediaKind,
  #[serde(skip)]
  played: bool,
  #[serde(skip)]
  loaded_host: u8,
  #[serde(skip_serializing_if = "Option::is_none")]
  title: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  duration_sec: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  width: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  height: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  audio_only: Option<bool>,
  main: bool,
  #[serde(skip_serializing_if = "std::ops::Not::not")]
  already_saved: bool,
  /// A small JPEG `data:` URL (main videos only), fetched through an admitted request.
  #[serde(skip_serializing_if = "Option::is_none")]
  thumbnail: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum MediaKind {
  Hls,
  Dash,
  Direct,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CatchState {
  window_id: String,
  url: String,
  host: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  title: Option<String>,
  can_back: bool,
  can_forward: bool,
  loading: bool,
  phase: Phase,
  popups_blocked: u32,
  /// Ad blocking is on for this page's site (the "Block ads" switch).
  adblock: bool,
  /// Requests the ad blocker stopped on this page.
  ads_blocked: u32,
  played: bool,
  played_sec: u32,
  candidates: Vec<CatchCandidate>,
  #[serde(skip_serializing_if = "Option::is_none")]
  saved_download_id: Option<String>,
  first_run: bool,
  #[serde(skip_serializing_if = "Option::is_none")]
  signin_hint: Option<&'static str>,
  /// The site that offer is for; the shell sends it back with the import.
  #[serde(skip_serializing_if = "Option::is_none")]
  signin_site: Option<String>,
  /// Every tab of the window, in the side list's order (empty for a page outside a browser window).
  tabs: Vec<CatchTab>,
  active_tab_id: String,
}

impl CatchState {
  /// The browser window's view: the active tab's fields plus the tab list.
  pub(super) fn with_tabs(mut self, tabs: Vec<CatchTab>, active: &str) -> Self {
    self.tabs = tabs;
    self.active_tab_id = active.to_owned();
    self
  }

  pub(super) fn found(&self) -> bool {
    self.candidates.iter().any(|c| c.main)
  }

  pub(super) fn with_load_failure(mut self) -> Self {
    self.phase = Phase::LoadFailed;
    self.host = psl::domain_str(&self.host).unwrap_or(&self.host).to_owned();
    self
  }
}

/// One row of the side list.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CatchTab {
  pub(super) id: String,
  url: String,
  host: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  title: Option<String>,
  loading: bool,
  phase: Phase,
  /// A main candidate exists: the dot.
  found: bool,
  private: bool,
  #[serde(skip_serializing_if = "Option::is_none")]
  opener_id: Option<String>,
  asleep: bool,
  mobile: bool,
}

/// Where a tab is, apart from its page.
pub(super) struct TabPlace<'a> {
  pub(super) id: &'a str,
  pub(super) private: bool,
  pub(super) opener: Option<&'a str>,
  pub(super) asleep: bool,
  /// The tab's page presents the iPhone identity.
  pub(super) mobile: bool,
}

impl CatchTab {
  /// A tab with a live page: its row reads the page's own state.
  pub(super) fn live(place: TabPlace<'_>, state: &CatchState) -> Self {
    Self {
      id: place.id.to_owned(),
      url: state.url.clone(),
      host: state.host.clone(),
      title: state.title.clone(),
      loading: state.loading,
      phase: state.phase,
      found: state.found(),
      private: place.private,
      opener_id: place.opener.map(str::to_owned),
      asleep: place.asleep,
      mobile: place.mobile,
    }
  }

  /// A home or sleeping tab: only the address and title it keeps.
  pub(super) fn idle(place: TabPlace<'_>, url: &str, title: Option<&str>) -> Self {
    let parsed = Url::parse(url).ok().filter(is_http);
    Self {
      id: place.id.to_owned(),
      url: parsed.as_ref().map(Url::to_string).unwrap_or_default(),
      host: parsed
        .as_ref()
        .and_then(Url::host_str)
        .unwrap_or_default()
        .to_owned(),
      title: parsed.as_ref().and(title.map(str::to_owned)),
      loading: false,
      phase: Phase::Looking,
      found: false,
      private: place.private,
      opener_id: place.opener.map(str::to_owned),
      asleep: place.asleep,
      mobile: false,
    }
  }
}

#[derive(Default)]
pub(super) struct PopupPolicy {
  pub(super) allow_once: bool,
  blocked: u32,
  /// Refusals counted here that the page's own count has not reported yet.
  refused: u32,
}

impl PopupPolicy {
  /// The most pop-ups one read of a page's own count may add.
  const MAX_PAGE_COUNT: u32 = 20;

  pub(super) fn request(&mut self, eligible: bool) -> bool {
    if eligible && std::mem::take(&mut self.allow_once) {
      return true;
    }
    self.blocked = self.blocked.saturating_add(1);
    self.refused = self.refused.saturating_add(1);
    false
  }

  pub(super) fn deny(&mut self) {
    self.request(false);
  }

  /// A pop-up the user caused (a click) opens; any other waits for "allow once".
  pub(super) fn admit(&mut self, eligible: bool, gesture: bool) -> bool {
    if eligible && gesture {
      self.allow_once = false;
      return true;
    }
    self.request(eligible)
  }

  /// The page saw `failed` of its own pop-ups not open since it was last asked. Those refused
  /// here are counted already; the rest were stopped by the engine (no click caused them).
  pub(super) fn page_count(&mut self, failed: u32) {
    let known = self.refused.min(failed);
    self.refused -= known;
    self.blocked = self
      .blocked
      .saturating_add((failed - known).min(Self::MAX_PAGE_COUNT));
  }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hit<'a> {
  method: &'a str,
  #[serde(borrow)]
  frame: std::borrow::Cow<'a, str>,
  is_top: bool,
  url: Option<String>,
  requested_url: Option<String>,
  ct: Option<String>,
  msg_origin: Option<String>,
  #[serde(default)]
  eme_active: bool,
  #[serde(default)]
  eme_probe: bool,
  #[serde(default)]
  playing: bool,
  #[serde(default)]
  loaded: bool,
  play_delta_sec: Option<f64>,
  played_media_sec: Option<f64>,
  player_top: Option<i32>,
  duration_sec: Option<f64>,
  /// The media element of its frame the hit is about.
  media_id: Option<u64>,
}

fn page_url(raw: &str) -> Option<Url> {
  if raw.len() > MAX_URL_BYTES || raw.trim() != raw || raw.chars().any(char::is_control) {
    return None;
  }
  let mut url = Url::parse(raw).ok()?;
  if !is_http(&url) {
    return None;
  }
  url.set_fragment(None);
  Some(url)
}

pub(super) fn is_http(url: &Url) -> bool {
  matches!(url.scheme(), "http" | "https")
    && url.has_host()
    && url.username().is_empty()
    && url.password().is_none()
    && url.as_str().len() <= MAX_URL_BYTES
    // Tauri serves its own app, IPC and assets on these hosts (Windows uses http).
    && !url.host_str().is_some_and(|host| {
      let host = host.trim_end_matches('.');
      ["tauri.localhost", "ipc.localhost", "asset.localhost"]
        .iter()
        .any(|internal| host.eq_ignore_ascii_case(internal))
    })
}

fn hit_context(hit: &Hit<'_>, top: &Url) -> Option<(Url, String, Source)> {
  let frame = page_url(&hit.frame)?;
  let origin = frame.origin().ascii_serialization();
  if hit.is_top {
    if &frame != top || hit.msg_origin.is_some() {
      return None;
    }
  } else if !matches!(hit.msg_origin.as_deref(), Some("null"))
    && hit.msg_origin.as_deref() != Some(origin.as_str())
  {
    return None;
  }
  let source = match hit.method {
    "fetch" | "worker-fetch" => Source::Fetch,
    "xhr" | "worker-xhr" => Source::Xhr,
    "video.src"
    | "video.currentSrc"
    | "source.src"
    | "player.present"
    | "player.reset"
    | "player.inactive"
    | "player.progress"
    | "playing"
    | "encrypted"
    | "setMediaKeys"
    | "webkitSetMediaKeys"
    | "requestMediaKeySystemAccess" => Source::MediaElement,
    method if method.starts_with("perf:") && method.len() <= 64 => Source::Performance,
    _ => return None,
  };
  Some((
    frame,
    if hit.msg_origin.as_deref() == Some("null") {
      "null".into()
    } else {
      origin
    },
    source,
  ))
}

#[derive(Default)]
struct MediaDetails {
  id: String,
  duration: Option<f64>,
  playing: bool,
  playing_directly: bool,
  top_frame: bool,
  manifest: Option<ManifestInfo>,
  inspection_started: bool,
  formats: Option<Vec<CatchFormat>>,
  download_url: Option<Url>,
  handoff: Arc<tokio::sync::OnceCell<Handoff>>,
  /// The page address that yt-dlp read (the retry ladder's last step).
  rescued: bool,
  /// The page's own address, offered for the normal download path (`page_video`).
  page: bool,
}

/// What one media element of a frame showed. EME evidence stays with its element.
#[derive(Default)]
struct Element {
  encrypted: bool,
  previous_encryption: Option<Encrypted>,
  playing: bool,
  duration: Option<f64>,
  media: Option<Url>,
  blob_duration: Option<f64>,
  blob_growth_streak: u8,
  playback: Option<ElementPlayback>,
}

#[derive(Default)]
struct ElementPlayback {
  media: Option<Url>,
  duration: Option<f64>,
  position: Option<(i32, Instant)>,
  played_sec: f64,
  played_media_sec: f64,
}

/// One encrypted video as the verdict weighs it.
#[derive(Clone, Copy)]
struct Encrypted {
  duration: Option<f64>,
  playing: bool,
  /// A manifest names a DRM key system: evidence even before the video plays.
  manifest: bool,
}

impl Encrypted {
  fn long(&self) -> bool {
    self
      .duration
      .is_none_or(|seconds| seconds >= MAIN_DURATION_SEC)
  }

  fn known(&self) -> bool {
    self.manifest || self.playing || self.duration.is_some()
  }
}

#[derive(Default)]
struct Playback {
  seconds: f64,
  duration: Option<f64>,
}

#[derive(Default)]
pub(super) struct Document {
  candidates: CandidateSet,
  details: HashMap<Url, MediaDetails>,
  /// Keyed by frame and the hook's element number.
  elements: HashMap<(Url, u64), Element>,
  /// A save or format read of this page answered DRM.
  marked_drm: bool,
  /// yt-dlp read the page itself: that result is offered whatever the page plays.
  rescued: bool,
  /// The page's address names one video the normal path reads: it is the page's only offer.
  page_offer: bool,
  /// That video's picture on its site.
  page_picture: Option<Url>,
  played: bool,
  played_sec: f64,
  last_drain_at: Option<Instant>,
  player_seen: bool,
  pub(super) user_agent: Option<String>,
  pub(super) saving: bool,
  pub(super) saved: Option<String>,
  pub(super) drm_counted: bool,
  pub(super) crashed: bool,
  /// The top frame is a geo-block page, as the last drain saw it: a page that fills in later
  /// (it was only slow) is no longer one.
  pub(super) geo_blocked: bool,
  /// The last drain carried playback progress: the drain loop polls faster while it does.
  pub(super) playing: bool,
  /// The playing video's poster and the page's own picture, as the hook last named them.
  pub(super) pictures: (Option<Url>, Option<Url>),
  thumbnail_started: bool,
  /// The main video's picture as a small JPEG `data:` URL.
  pub(super) thumbnail: Option<String>,
}

/// What the drain loop needs to find a thumbnail for the page's main video.
pub(super) struct ThumbnailJob {
  pub(super) pictures: Vec<Url>,
  /// The page the pictures belong to: their Referer.
  pub(super) page: Url,
  pub(super) user_agent: Option<String>,
  /// The main video, for a frame when no picture works: its address, HLS or not, and its frame.
  pub(super) media: Option<(Url, bool, String)>,
}

impl Document {
  fn drain(&mut self, raw: &str, top: &Url, now: Instant) {
    if raw.len() > MAX_DRAIN_BYTES {
      return;
    }
    // Raw values isolate bad entries without guessing types or trusting page-provided classification.
    let Ok(hits) = serde_json::from_str::<Vec<&serde_json::value::RawValue>>(raw) else {
      return;
    };
    if hits.len() > MAX_ENTRIES {
      return;
    }

    let elapsed = self
      .last_drain_at
      .replace(now)
      .map_or(0.0, |last| now.duration_since(last).as_secs_f64());
    self.playing = false;
    let mut playback = HashMap::<(Url, Option<u64>, Option<Url>), Playback>::new();
    let mut blob_durations = HashMap::new();
    for hit in hits {
      let Ok(hit) = serde_json::from_str::<Hit<'_>>(hit.get()) else {
        continue;
      };
      let Some((frame, origin, source)) = hit_context(&hit, top) else {
        continue;
      };
      if hit.method == "player.present" {
        self.player_seen = true;
        continue;
      }
      let key = (frame.clone(), hit.media_id.unwrap_or_default());
      match hit.method {
        "player.reset" => {
          if let Some(id) = hit.media_id {
            if let Some(previous) = self.elements.remove(&key) {
              let mut previous_encryption = previous.previous_encryption;
              if previous.encrypted {
                let evidence = Encrypted {
                  duration: previous.duration,
                  playing: previous.playing,
                  manifest: false,
                };
                let strength = |e: Encrypted| (e.known() && e.long(), e.playing);
                if previous_encryption.is_none_or(|past| strength(evidence) > strength(past)) {
                  previous_encryption = Some(evidence);
                }
              }
              if previous_encryption.is_some() {
                self.elements.insert(
                  key.clone(),
                  Element {
                    previous_encryption,
                    ..Element::default()
                  },
                );
              }
            }
            blob_durations.remove(&key);
            playback.retain(|(owner, media_id, _), _| owner != &frame || *media_id != Some(id));
          }
          continue;
        }
        "player.inactive" => {
          if let Some(player) = hit
            .media_id
            .and_then(|_| self.elements.get_mut(&key))
            .and_then(|e| e.playback.as_mut())
          {
            player.position = None;
          }
          continue;
        }
        _ => {}
      }
      // Only an `encrypted` event is evidence: players attach keys to clear video too.
      let encrypted = hit.eme_active && hit.method == "encrypted";
      let duration = hit
        .duration_sec
        .filter(|s| source == Source::MediaElement && (0.0..=1.0e9).contains(s) && *s > 0.0);
      let media = hit.url.as_deref().and_then(|url| {
        let candidate = self.candidates.observe(Observation {
          url,
          requested_url: hit.requested_url.as_deref(),
          content_type: hit.ct.as_deref(),
          frame_url: frame.as_str(),
          frame_origin: &origin,
          source,
          first_seen: now,
          first_bytes: &[],
          is_byte_range: false,
          is_hls_segment: false,
          loaded: hit.loaded && source != Source::MediaElement,
          eme_probe: hit.eme_probe && hit.method == "requestMediaKeySystemAccess",
          eme_active: encrypted,
        })?;
        let url = candidate.url();
        let download_url = candidate.download_url();
        let details = self
          .details
          .entry(url.clone())
          .or_insert_with(|| MediaDetails {
            id: Uuid::new_v4().to_string(),
            download_url: Some(download_url.clone()),
            ..MediaDetails::default()
          });
        if details.download_url.as_ref() != Some(download_url) {
          details.download_url = Some(download_url.clone());
          details.manifest = None;
          details.inspection_started = false;
          details.formats = None;
          details.handoff = Arc::default();
        }
        if let Some(duration) = duration {
          details.duration = Some(duration);
        }
        details.top_frame |= hit.is_top;
        Some(url.clone())
      });
      let playing = hit.playing && hit.method == "playing";
      if source == Source::MediaElement
        && (encrypted || playing || hit.media_id.is_some())
        && (self.elements.len() < MAX_ELEMENTS || self.elements.contains_key(&key))
      {
        // `report` sends playback from non-HTTP sources without a URL. Only compare the
        // same identified playing element across snapshots, once per drain.
        if hit.url.is_some() {
          blob_durations.remove(&key);
        } else if let Some(duration) = duration.filter(|_| playing && hit.media_id.is_some()) {
          blob_durations.insert(key.clone(), duration);
        }
        let element = self.elements.entry(key).or_default();
        element.encrypted |= encrypted;
        element.playing |= playing;
        element.duration = duration.or(element.duration);
        element.media = media.clone().or(element.media.take());
        if hit.media_id.is_some() && (playing || hit.method == "player.progress") {
          if playing && element.playback.as_ref().is_none_or(|p| p.media != media) {
            element.playback = Some(ElementPlayback {
              media: media.clone(),
              ..Default::default()
            });
          }
          if let Some(player) = element.playback.as_mut().filter(|p| p.media == media) {
            player.duration = duration.or(player.duration);
            if let Some(top) = hit.player_top.filter(|top| playing && *top >= 0) {
              player.position = Some((top, now));
            }
            if let Some(seconds) = hit.played_media_sec.filter(|s| (0.0..=1.0e9).contains(s)) {
              player.played_media_sec = player.played_media_sec.max(seconds);
            }
          }
        }
        if hit.url.is_some() {
          element.blob_duration = None;
          element.blob_growth_streak = 0;
        }
      }
      if playing {
        self.played = true;
        self.playing = true;
        let delta = hit
          .play_delta_sec
          .filter(|s| (0.0..=1.0).contains(s))
          .unwrap_or_default();
        let playback = playback.entry((frame, hit.media_id, media)).or_default();
        playback.seconds += delta;
        playback.duration = duration.or(playback.duration);
      }
    }
    for (key, duration) in blob_durations {
      if let Some(element) = self.elements.get_mut(&key) {
        if let Some(previous) = element.blob_duration {
          let change = duration - previous;
          // A duration jump can be an ad giving way to the film on the same element.
          element.blob_growth_streak = if change > 0.0 && change <= 1.5 * elapsed {
            element.blob_growth_streak.saturating_add(1)
          } else {
            0
          };
        }
        element.blob_duration = Some(duration);
      }
    }
    // Concurrent players can report the same second. Credit at most the busiest player.
    self.played_sec += playback
      .values()
      .map(|p| p.seconds)
      .fold(0.0, f64::max)
      .min(1.25);
    for ((frame, id, media), playback) in playback {
      if let Some(player) = id
        .and_then(|id| self.elements.get_mut(&(frame.clone(), id)))
        .and_then(|e| e.playback.as_mut())
        .filter(|p| p.media == media)
      {
        player.played_sec += playback.seconds.min(1.25);
      }
      let playing_directly = media.is_some();
      let url = media.or_else(|| self.blob_stream(&frame, playback.duration, false));
      if let Some(details) = url.and_then(|url| self.details.get_mut(&url)) {
        details.playing = true;
        details.playing_directly |= playing_directly;
        details.duration = playback.duration.or(details.duration);
      }
    }
  }

  fn live_blob_frames(&self) -> HashSet<&str> {
    self
      .elements
      .iter()
      .filter(|(_, element)| element.blob_growth_streak >= 2)
      .map(|((frame, _), _)| frame.as_str())
      .filter(|frame| {
        !self.candidates.iter().any(|candidate| {
          *frame == candidate.frame_url
            && self.details.get(candidate.url()).is_some_and(|details| {
              details
                .manifest
                .as_ref()
                .is_some_and(|manifest| !manifest.is_live && manifest.duration_seconds.is_some())
            })
        })
      })
      .collect()
  }

  /// Bind blob playback to a stream in its frame using duration and successful media loads.
  /// A legacy guess can label a main stream, but cannot give it a player's priority or time.
  fn blob_stream(&self, frame: &Url, played: Option<f64>, require_match: bool) -> Option<Url> {
    let short = |seconds: f64| seconds < MAIN_DURATION_SEC;
    let streams: Vec<_> = self
      .candidates
      .iter()
      .filter(|c| frame.as_str() == c.frame_url && matches!(c.kind, Kind::Hls | Kind::Dash))
      .collect();
    let duration = |c: &Candidate| {
      self
        .details
        .get(c.url())?
        .manifest
        .as_ref()?
        .duration_seconds
    };
    let duration_match = |c: &Candidate| {
      played
        .zip(duration(c))
        .map(|(played, measured)| (measured - played).abs() <= 0.5)
    };
    let mut matching = streams.iter().copied().filter_map(|c| {
      let loaded = self.candidates.loaded_host(c);
      match duration_match(c) {
        Some(true) => Some((c, (loaded, true))),
        None if loaded == 2 => Some((c, (loaded, false))),
        _ => None,
      }
    });
    if let Some(mut best) = matching.next() {
      let mut unique = true;
      for candidate in matching {
        if candidate.1 > best.1 {
          best = candidate;
          unique = true;
        } else if candidate.1 == best.1 {
          unique = false;
        }
      }
      if unique {
        return Some(best.0.url().clone());
      }
    }
    if let [only] = streams[..] {
      return (duration_match(only) != Some(false)
        || (!require_match && !played.is_some_and(short)))
      .then(|| only.url().clone());
    }
    if require_match || played.is_some_and(short) {
      return None;
    }
    streams
      .into_iter()
      .filter_map(|c| Some((c, self.details.get(c.url())?)))
      .filter(|(_, d)| {
        !d.manifest
          .as_ref()
          .and_then(|m| m.duration_seconds)
          .or(d.duration)
          .is_some_and(short)
      })
      .max_by_key(|(c, d)| {
        (
          self.candidates.loaded_host(c),
          d.manifest.as_ref().is_some_and(|m| !m.variants.is_empty()),
          c.first_seen,
        )
      })
      .map(|(c, _)| c.url().clone())
  }

  pub(super) fn next_manifest(&mut self) -> Option<(Url, Url)> {
    self.candidates.iter().find_map(|candidate| {
      let details = self.details.get_mut(candidate.url())?;
      if details.inspection_started || !matches!(candidate.kind, Kind::Hls | Kind::Dash) {
        return None;
      }
      details.inspection_started = true;
      // The browser may have received a new signed address even if replaying the original
      // request no longer redirects. Keep the original only as the observation's identity.
      Some((candidate.url().clone(), candidate.download_url().clone()))
    })
  }

  pub(super) fn inspected(&mut self, url: &Url, info: ManifestInfo) {
    if !self.candidates.iter().any(|c| c.url() == url) {
      return;
    }
    if !info.variants.is_empty() {
      self.candidates.suppress_hls_variants(url, &info.variants);
      self
        .details
        .retain(|url, _| self.candidates.iter().any(|c| c.url() == url));
    }
    if let Some(details) = self.details.get_mut(url) {
      details.manifest = Some(info);
    }
  }

  pub(super) fn inspected_response(&mut self, url: &Url, response_url: &Url, info: ManifestInfo) {
    if self.details.get(url).and_then(|d| d.download_url.as_ref()) == Some(response_url) {
      self.inspected(url, info);
    }
  }

  pub(super) fn mark_drm(&mut self) {
    self.marked_drm = true;
  }

  /// The page's verdict before any retry: its main video is encrypted.
  pub(super) fn has_drm(&self) -> bool {
    self.candidates(&None, &HashSet::new()).1
  }

  /// Whether a clear main video is on offer.
  fn found_main(&self) -> bool {
    let (candidates, ..) = self.candidates(&None, &HashSet::new());
    candidates.iter().any(|c| c.main)
  }

  /// yt-dlp read the page address itself: offer that as the page's video.
  pub(super) fn rescue(
    &mut self,
    candidate: Candidate,
    handoff: Handoff,
    formats: Vec<CatchFormat>,
  ) {
    let url = candidate.url().clone();
    if !self.candidates.insert(candidate) {
      return;
    }
    self.details.insert(
      url,
      MediaDetails {
        id: Uuid::new_v4().to_string(),
        formats: Some(formats),
        handoff: Arc::new(tokio::sync::OnceCell::new_with(Some(handoff))),
        rescued: true,
        ..MediaDetails::default()
      },
    );
    self.rescued = true;
  }

  /// Offers the page's own address as its video, before and whatever the player streams.
  fn offer_page(&mut self, url: Url, video: super::page_video::PageVideo) {
    let candidate = Candidate::site_page(url, video.extractors);
    let url = candidate.url().clone();
    if !self.candidates.insert(candidate) {
      return;
    }
    self.details.insert(
      url,
      MediaDetails {
        id: Uuid::new_v4().to_string(),
        page: true,
        ..MediaDetails::default()
      },
    );
    self.page_offer = true;
    self.page_picture = video.picture;
  }

  pub(super) fn handoff_candidate(&self, id: &str) -> Option<&super::candidate::Candidate> {
    if self.has_drm() {
      return None;
    }
    self.candidates.iter().find(|c| {
      c.kind != Kind::Unknown
        && self.details.get(c.url()).is_some_and(|d| {
          d.id == id && !d.manifest.as_ref().is_some_and(|m| m.has_drm || m.is_live)
        })
    })
  }

  pub(super) fn handoff(&self, url: &Url) -> Arc<tokio::sync::OnceCell<Handoff>> {
    // A missing entry gets a fresh cell, never a panic: callers already checked the candidate.
    self
      .details
      .get(url)
      .map(|details| details.handoff.clone())
      .unwrap_or_default()
  }

  pub(super) fn formats(&self, url: &Url) -> Option<&[CatchFormat]> {
    self.details.get(url)?.formats.as_deref()
  }

  pub(super) fn cache_formats(&mut self, url: &Url, formats: Vec<CatchFormat>) {
    if let Some(details) = self.details.get_mut(url) {
      details.formats = Some(formats);
    }
  }

  fn candidates(
    &self,
    title: &Option<String>,
    saved: &HashSet<Url>,
  ) -> (Vec<CatchCandidate>, bool, bool) {
    self.candidates_with_live_blobs(title, saved, &HashSet::new())
  }

  fn candidates_with_live_blobs(
    &self,
    title: &Option<String>,
    saved: &HashSet<Url>,
    live_blob_frames: &HashSet<&str>,
  ) -> (Vec<CatchCandidate>, bool, bool) {
    let mut players = HashMap::<Url, (Option<i32>, f64)>::new();
    for ((frame, _), element) in &self.elements {
      let Some(player) = &element.playback else {
        continue;
      };
      // A short replacement cannot stand in for the protected film on this element.
      if element
        .previous_encryption
        .is_some_and(|e| e.known() && e.long())
        && player
          .duration
          .is_some_and(|seconds| seconds < MAIN_DURATION_SEC)
      {
        continue;
      }
      let media = player
        .media
        .clone()
        .or_else(|| self.blob_stream(frame, player.duration, true));
      if let Some(url) = media {
        let (top, played) = players.entry(url).or_default();
        if let Some((position, _)) = player.position.filter(|&(position, seen)| {
          position < i32::MAX
            && self
              .last_drain_at
              .is_some_and(|now| now.saturating_duration_since(seen) <= ACTIVE_PLAYER_TTL)
        }) {
          *top = Some(top.map_or(position, |previous| previous.min(position)));
        }
        *played = played.max(player.played_media_sec.max(player.played_sec));
      }
    }
    let mut candidates = Vec::new();
    let mut encrypted: Vec<_> = self
      .elements
      .values()
      .filter_map(|e| e.previous_encryption)
      .filter(|e| e.known() && e.long())
      .collect();
    let mut playing = HashSet::new();
    let mut active = Vec::new();
    let mut short_played = HashSet::new();
    let mut live = false;
    let mut page = None;
    for c in self.candidates.iter() {
      let Some(details) = self.details.get(c.url()) else {
        continue;
      };
      let manifest = details.manifest.as_ref();
      if c.has_drm() || manifest.is_some_and(|m| m.has_drm) {
        encrypted.push(Encrypted {
          duration: manifest
            .and_then(|m| m.duration_seconds)
            .or(details.duration),
          playing: details.playing,
          manifest: manifest.is_some_and(|m| m.has_drm),
        });
        continue;
      }
      if manifest.is_some_and(|m| m.is_live) {
        live = true;
        continue;
      }
      // A stream guessed from the live blob's frame is not a verified VOD offer.
      if details.playing
        && !details.playing_directly
        && live_blob_frames.contains(c.frame_url.as_str())
      {
        continue;
      }
      // The page's own address reads better than a stream of it (title, every quality). The
      // retry ladder's yt-dlp read of that same address replaces the offer's details.
      if self.page_offer && !details.page && !details.rescued {
        continue;
      }
      let kind = match c.kind {
        Kind::Hls => MediaKind::Hls,
        Kind::Dash => MediaKind::Dash,
        Kind::Direct => MediaKind::Direct,
        Kind::Unknown => continue,
      };
      let duration = manifest
        .and_then(|m| m.duration_seconds)
        .or(details.duration);
      let player = players.get(c.url());
      let played = details.playing || player.is_some();
      if duration.is_none() && !played && !details.rescued && !details.page {
        continue;
      }
      let loaded_host = self.candidates.loaded_host(c);
      if played {
        playing.insert(details.id.clone());
      }
      if let Some(&(top, seconds)) = player {
        if let Some(top) = top {
          active.push((top, loaded_host, duration, details.id.clone()));
        }
        if duration.is_some_and(|length| {
          seconds > 0.0 && seconds >= (length - 0.5).min(SHORT_ONLY_PLAYED_SEC)
        }) {
          short_played.insert(details.id.clone());
        }
      }
      if details.page {
        page = Some(details.id.clone());
      }
      let resolution = manifest.and_then(|m| {
        m.variants
          .iter()
          .filter_map(|v| v.resolution)
          .max_by_key(|&(_, height)| height)
      });
      candidates.push(CatchCandidate {
        id: details.id.clone(),
        kind,
        played,
        loaded_host,
        title: title.clone(),
        duration_sec: duration,
        width: resolution.map(|(width, _)| width),
        height: resolution.map(|(_, height)| height),
        audio_only: c
          .content_type
          .as_deref()
          .filter(|_| kind == MediaKind::Direct)
          .map(|mime| mime.to_ascii_lowercase().starts_with("audio/")),
        main: details.page || duration.is_none_or(|seconds| seconds >= MAIN_DURATION_SEC),
        already_saved: saved.contains(c.url()),
        thumbnail: None,
      });
    }
    // A visible player can outrank a preload. A lone short player needs more evidence
    // to stand alone and cannot take over from a main that already played.
    let any_main = candidates.iter().any(|c| c.main);
    if !self.page_offer && (any_main || active.len() > 1) {
      let preferred = active.iter().min_by(|a, b| {
        a.0
          .cmp(&b.0)
          .then_with(|| compare_duration_and_host((b.2, b.1), (a.2, a.1)))
          .then_with(|| a.3.cmp(&b.3))
      });
      let short =
        |duration: &Option<f64>| duration.is_some_and(|seconds| seconds < MAIN_DURATION_SEC);
      let unplayed_film = candidates
        .iter()
        .any(|c| c.main && !c.played && c.duration_sec.is_some_and(|seconds| seconds >= FILM_SEC));
      if let Some((_, _, _, id)) = preferred.filter(|(_, _, duration, _)| {
        !(short(duration) && unplayed_film)
          && (active.len() > 1
            || !short(duration)
            || !candidates.iter().any(|c| c.main && c.played))
      }) {
        for candidate in &mut candidates {
          candidate.main = &candidate.id == id;
        }
      }
    } else if !any_main {
      if let Some(played) = candidates
        .iter_mut()
        .filter(|c| short_played.contains(&c.id))
        .max_by(|a, b| {
          a.duration_sec
            .unwrap_or_default()
            .total_cmp(&b.duration_sec.unwrap_or_default())
        })
      {
        played.main = true;
      }
    }
    candidates.sort_by(|a, b| {
      b.main.cmp(&a.main).then_with(|| {
        if a.main && b.main {
          compare_duration_and_host(
            (b.duration_sec, b.loaded_host),
            (a.duration_sec, a.loaded_host),
          )
        } else {
          std::cmp::Ordering::Equal
        }
      })
    });
    // Encrypted elements whose media is not a candidate already weighed above (blob players).
    encrypted.extend(
      self
        .elements
        .values()
        .filter(|e| e.encrypted)
        .filter(|e| {
          !e.media.as_ref().is_some_and(|url| {
            self
              .candidates
              .iter()
              .any(|c| c.url() == url && c.has_drm())
          })
        })
        .map(|e| Encrypted {
          duration: e.duration,
          playing: e.playing,
          manifest: false,
        }),
    );
    let drm = self.marked_drm
      || (!self.rescued && encrypted_main(&encrypted, &candidates, &playing, self.played_sec));
    // The page's address is no offer for a live or encrypted video: the hand-off refuses both.
    if drm || live {
      candidates.retain(|c| page.as_ref() != Some(&c.id));
    }
    (candidates, drm, live)
  }
}

/// Whether the page's main video is the encrypted one: an encrypted ad or preview beside a
/// clear main video is not. An element of unknown length that has not played says nothing yet.
fn encrypted_main(
  encrypted: &[Encrypted],
  clear: &[CatchCandidate],
  playing: &HashSet<String>,
  played_sec: f64,
) -> bool {
  let mut known = encrypted.iter().filter(|e| e.known());
  if clear.iter().any(|c| c.main) {
    // The main video is the one playing: encrypted only when that is a long encrypted one.
    return !clear.iter().any(|c| c.main && playing.contains(&c.id))
      && known.any(|e| e.playing && e.long());
  }
  known.any(|e| {
    e.long()
      // Nothing clear at all: a short encrypted video that keeps playing is the page's video.
      || (clear.is_empty()
        && e.playing
        && e.duration.is_some_and(|seconds| {
          played_sec > 0.0 && played_sec >= (seconds - 0.5).min(SHORT_ONLY_PLAYED_SEC)
        }))
  })
}

/// The next retry the ladder asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Retry {
  /// Reload the tab as the Mobile version.
  Mobile,
  /// The Mobile version did not help: back to the tab's own identity, reloaded.
  Restore,
  /// yt-dlp on the page address.
  Ytdlp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
  Start,
  Mobile { generation: u64, since: Instant },
  Restore { generation: u64, since: Instant },
  Ytdlp,
  Done,
}

/// Retries before a visible tab shows "plays only in its own player": each step once per page,
/// all of them inside `LADDER_CAP`. The tab keeps showing "looking" meanwhile.
pub(super) struct Ladder {
  page: Option<Url>,
  started: Instant,
  step: Step,
}

impl Default for Ladder {
  fn default() -> Self {
    Self {
      page: None,
      started: Instant::now(),
      step: Step::Done,
    }
  }
}

impl Ladder {
  /// Whether the retries for this page are over (or never started on it).
  fn finished(&self, url: &Url, now: Instant) -> bool {
    self.page.as_ref() == Some(url)
      && (self.step == Step::Done || now.duration_since(self.started) >= LADDER_CAP)
  }
}

fn phase(
  crashed: bool,
  saving: bool,
  drm: bool,
  live: bool,
  geo_blocked: bool,
  candidates: &[CatchCandidate],
  played_sec: u32,
) -> Phase {
  if crashed {
    Phase::Crashed
  } else if saving {
    Phase::Saving
  } else if drm {
    Phase::Drm
  } else if candidates.iter().any(|c| c.main) {
    Phase::Found
  } else if live {
    Phase::Live
  } else if !candidates.is_empty() {
    Phase::AdPlaying
  } else if geo_blocked {
    // Below anything that plays: a page with a stream or an ad is not a block page.
    Phase::GeoBlocked
  } else if played_sec >= 10 {
    Phase::NotFound
  } else {
    Phase::Looking
  }
}

pub(super) struct PageState {
  pub(super) url: Url,
  pub(super) document: Document,
  pub(super) generation: u64,
  pub(super) popup: PopupPolicy,
  pub(super) deep: bool,
  pub(super) first_run: bool,
  pub(super) title: Option<String>,
  pub(super) saved_urls: HashSet<Url>,
  /// A browser profile is signed in to this sign-in wall's site (set by `site_tools::observe`).
  pub(super) signin_hint: bool,
  /// A tab the user sees: a page whose address names one video offers it (`page_video`).
  /// Automatic catch never does: the normal path already failed on its link.
  pub(super) offers_page: bool,
  /// The top page shows a password field (the hook's last snapshot): a sign-in wall on the page.
  pub(super) password_visible: bool,
  ladder: Ladder,
  /// Requests the ad blocker stopped on this page (`adblock`).
  pub(super) ads_blocked: u32,
  last: Option<CatchState>,
}

impl PageState {
  pub(super) fn new(url: Url) -> Self {
    Self {
      url,
      document: Document::default(),
      generation: 0,
      popup: PopupPolicy::default(),
      deep: false,
      first_run: false,
      title: None,
      saved_urls: HashSet::new(),
      signin_hint: false,
      offers_page: false,
      password_visible: false,
      ladder: Ladder::default(),
      ads_blocked: 0,
      last: None,
    }
  }

  /// The encrypted verdict the tab shows and counts: only after the retries ran out.
  pub(super) fn drm_final(&self, now: Instant) -> bool {
    self.document.has_drm() && self.ladder.finished(&self.url, now)
  }

  /// Advances the retry ladder of a visible tab; returns the retry to run now, if any.
  /// `mobile`: the tab already presents the Mobile version (no mobile step then).
  pub(super) fn next_retry(&mut self, mobile: bool, now: Instant) -> Option<Retry> {
    let (_, drm, _) = self.document.candidates(&None, &HashSet::new());
    if self.ladder.page.as_ref() != Some(&self.url) {
      if !drm {
        return None;
      }
      self.ladder = Ladder {
        page: Some(self.url.clone()),
        started: now,
        step: Step::Start,
      };
    }
    let capped = now.duration_since(self.ladder.started) >= LADDER_CAP;
    let (step, retry) = match self.ladder.step {
      Step::Start if !drm || capped => return None,
      Step::Start if mobile => (Step::Ytdlp, Retry::Ytdlp),
      Step::Start => (
        Step::Mobile {
          generation: self.generation,
          since: now,
        },
        Retry::Mobile,
      ),
      // The reload has not replaced the page yet, or the Mobile version is still looking.
      Step::Mobile { generation, since }
        if !capped
          && (generation == self.generation || !drm)
          && now.duration_since(since) < MOBILE_WAIT =>
      {
        return None
      }
      // It found a clear video: the Mobile version stays.
      Step::Mobile { .. } if !drm && self.document.found_main() => {
        self.ladder.step = Step::Done;
        return None;
      }
      Step::Mobile { .. } => (
        Step::Restore {
          generation: self.generation,
          since: now,
        },
        Retry::Restore,
      ),
      Step::Restore { generation, since }
        if generation == self.generation && now.duration_since(since) < RESTORE_WAIT =>
      {
        return None
      }
      Step::Restore { .. } if capped => {
        self.ladder.step = Step::Done;
        return None;
      }
      Step::Restore { .. } => (Step::Ytdlp, Retry::Ytdlp),
      Step::Ytdlp | Step::Done => return None,
    };
    self.ladder.step = step;
    Some(retry)
  }

  /// The yt-dlp step ended (found or not): the ladder is over.
  pub(super) fn ytdlp_done(&mut self) {
    if self.ladder.page.as_ref() == Some(&self.url) && self.ladder.step == Step::Ytdlp {
      self.ladder.step = Step::Done;
    }
  }

  pub(super) fn navigated(&mut self, url: Url) {
    self.url = url;
    self.title = None;
    self.document = Document::default();
    self.signin_hint = false;
    self.password_visible = false;
    self.ads_blocked = 0;
    self.popup.allow_once = false;
    self.popup.refused = 0;
    self.generation = self.generation.wrapping_add(1);
    self.offer_page();
  }

  /// Whether the tab left this page for `url`. A fragment-only change of an offered page is the
  /// same video: the address follows it and the offer stays.
  pub(super) fn moved_to(&mut self, url: &Url) -> bool {
    if self.url == *url {
      return false;
    }
    if self.document.page_offer && page_url(self.url.as_str()) == page_url(url.as_str()) {
      self.url = url.clone();
      return false;
    }
    true
  }

  /// Offers the page's own address when it names one video (also after an in-page address
  /// change: the drain loop calls `navigated` for those too).
  pub(super) fn offer_page(&mut self) {
    if !self.offers_page {
      return;
    }
    if let Some(video) = super::page_video::page_video(&self.url) {
      self.document.offer_page(self.url.clone(), video);
    }
  }

  /// Whether the page offers something to save now: a main video, not a short clip beside it.
  pub(super) fn has_candidate(&self) -> bool {
    let (candidates, ..) = self.document.candidates(&self.title, &self.saved_urls);
    candidates.iter().any(|candidate| candidate.main)
  }

  pub(super) fn reset_delivery(&mut self) {
    self.last = None;
  }

  /// Remembers a saved media address, forgetting an arbitrary old one past the cap.
  pub(super) fn remember_saved(&mut self, url: Url) {
    if self.saved_urls.len() >= MAX_SAVED_URLS && !self.saved_urls.contains(&url) {
      if let Some(old) = self.saved_urls.iter().next().cloned() {
        self.saved_urls.remove(&old);
      }
    }
    self.saved_urls.insert(url);
  }

  pub(super) fn apply_drain(&mut self, native_url: &Url, raw: &str) {
    // The engine JSON-encodes the eval result: one string, which the hook serialized itself as
    // `[location, title, userAgent, hits, pictures, passwordVisible]` with a `JSON.stringify`
    // captured before page scripts.
    if raw.len() > MAX_DRAIN_BYTES * 2 {
      return;
    }
    let Ok(snapshot) = serde_json::from_str::<String>(raw) else {
      return;
    };
    self.apply_snapshot(native_url, &snapshot);
  }

  fn apply_snapshot(&mut self, native_url: &Url, snapshot: &str) -> bool {
    if snapshot.len() > MAX_DRAIN_BYTES * 2 {
      return false;
    }
    let Ok(parts) = serde_json::from_str::<Vec<&serde_json::value::RawValue>>(snapshot) else {
      return false;
    };
    let (Some(location), Some(title), Some(agent), Some(drain)) = (
      parts
        .first()
        .and_then(|raw| serde_json::from_str::<String>(raw.get()).ok()),
      parts
        .get(1)
        .and_then(|raw| serde_json::from_str::<String>(raw.get()).ok()),
      parts
        .get(2)
        .and_then(|raw| serde_json::from_str::<String>(raw.get()).ok()),
      parts.get(3),
    ) else {
      return false;
    };
    let pictures = parts
      .get(4)
      .and_then(|raw| serde_json::from_str::<(String, String)>(raw.get()).ok())
      .unwrap_or_default();
    // `[status, text]`: the top document's HTTP status (0: unknown) and a short page's text.
    let geo = parts
      .get(5)
      .and_then(|raw| serde_json::from_str::<(u16, String)>(raw.get()).ok())
      .unwrap_or_default();
    let Some(top) = page_url(native_url.as_str()) else {
      return false;
    };
    // Both the page's own word and this page's address: a snapshot of the address the tab
    // moved to in page never lands on the offer of the one before it.
    if page_url(&location).as_ref() != Some(&top)
      || page_url(self.url.as_str()) != Some(top.clone())
    {
      return false;
    }
    let title: String = title
      .chars()
      .filter(|c| !c.is_control())
      .take(256)
      .collect();
    let title = without_site_suffix(&title, &top);
    self.title = (!title.is_empty()).then(|| title.to_owned());
    let status = (geo.0 != 0).then_some(geo.0);
    // A long page sends no text: its title alone says nothing either.
    let text: String = if geo.1.is_empty() {
      String::new()
    } else {
      format!("{title} {}", geo.1)
        .chars()
        .take(GEO_TEXT_CHARS)
        .collect()
    };
    self.document.geo_blocked = super::geo::blocked(&top, status, &text);
    let picture = |raw: &str| page_url(raw).filter(|_| !raw.is_empty());
    self.document.pictures = (picture(&pictures.0), picture(&pictures.1));
    // An older hook sends no flag: no password field then. A redirect still loading is read
    // before this page state moves to it: the next page's field is not this page's.
    self.password_visible = page_url(self.url.as_str()).as_ref() == Some(&top)
      && parts
        .get(6)
        .and_then(|raw| serde_json::from_str::<bool>(raw.get()).ok())
        .unwrap_or(false);
    self.document.user_agent =
      (agent.len() <= 1024 && !agent.chars().any(char::is_control)).then_some(agent);
    self.document.drain(drain.get(), &top, Instant::now());
    true
  }

  #[cfg(test)]
  pub(super) fn replay(&mut self, id: &str, status: BrowserStatus) -> CatchState {
    self.reset_delivery();
    self.changed(id, status).expect("initial snapshot")
  }

  #[cfg(test)]
  pub(super) fn changed(&mut self, id: &str, status: BrowserStatus) -> Option<CatchState> {
    let state = self.snapshot(id, status);
    if self.last.as_ref() == Some(&state) {
      return None;
    }
    self.last = Some(state.clone());
    Some(state)
  }

  /// What the home page shows about a save of this candidate: its duration and height.
  pub(super) fn save_details(&self, candidate_id: &str) -> (Option<f64>, Option<u32>) {
    let (candidates, ..) = self.document.candidates(&self.title, &self.saved_urls);
    candidates
      .iter()
      .find(|candidate| candidate.id == candidate_id)
      .map(|candidate| (candidate.duration_sec, candidate.height))
      .unwrap_or_default()
  }

  /// The page's state now, without the per-page "only differences" filter (a browser window
  /// filters once for all its tabs).
  pub(super) fn snapshot(&self, id: &str, status: BrowserStatus) -> CatchState {
    let (mut candidates, drm, live) = self.document.candidates(&self.title, &self.saved_urls);
    let drm = drm && self.ladder.finished(&self.url, Instant::now());
    for candidate in candidates.iter_mut().filter(|candidate| candidate.main) {
      candidate.thumbnail = self.document.thumbnail.clone();
    }
    let played_sec = self.document.played_sec as u32;
    CatchState {
      window_id: id.to_owned(),
      url: if is_http(&self.url) {
        self.url.to_string()
      } else {
        String::new()
      },
      host: self.url.host_str().unwrap_or_default().to_owned(),
      title: self.title.clone(),
      can_back: status.can_back,
      can_forward: status.can_forward,
      loading: status.loading,
      phase: phase(
        self.document.crashed,
        self.document.saving || self.document.saved.is_some(),
        drm,
        live,
        self.document.geo_blocked,
        &candidates,
        played_sec,
      ),
      popups_blocked: self.popup.blocked,
      adblock: super::adblock::on(&self.url),
      ads_blocked: self.ads_blocked,
      played: self.document.played,
      played_sec,
      candidates,
      saved_download_id: self.document.saved.clone(),
      first_run: self.first_run,
      signin_hint: self.signin_hint.then_some("offer"),
      signin_site: self
        .url
        .host_str()
        .filter(|_| self.signin_hint)
        .and_then(super::hidden_signin::copy_site),
      tabs: Vec::new(),
      active_tab_id: String::new(),
    }
  }

  /// Once per page, when it shows a main video: where its thumbnail can come from.
  pub(super) fn thumbnail_job(&mut self) -> Option<ThumbnailJob> {
    if self.document.thumbnail_started || self.document.has_drm() || !is_http(&self.url) {
      return None;
    }
    let (candidates, drm, _) = self.document.candidates(&self.title, &self.saved_urls);
    let main = candidates.iter().find(|c| c.main);
    let main = main.filter(|_| !drm)?;
    self.document.thumbnail_started = true;
    let media = self
      .document
      .handoff_candidate(&main.id)
      // A page address is no media file to take a frame from.
      .filter(|c| matches!(c.kind, Kind::Hls | Kind::Direct) && c.source != Source::Native)
      .map(|c| {
        // A sandboxed frame has no origin of its own to name: the page stands in (as the
        // download does).
        let referer = if c.frame_origin == "null" {
          self.url.to_string()
        } else {
          c.frame_url.clone()
        };
        (c.download_url().clone(), c.kind == Kind::Hls, referer)
      });
    let (poster, page) = self.document.pictures.clone();
    let mut pictures = super::thumbnail::pictures(poster, page);
    if let Some(picture) = self.document.page_picture.clone() {
      pictures.retain(|url| url != &picture);
      pictures.insert(0, picture);
    }
    Some(ThumbnailJob {
      pictures,
      page: self.url.clone(),
      user_agent: self.document.user_agent.clone(),
      media,
    })
  }
}

/// The title without a trailing site name (" | Movies4U", " - YouTube"): a separator
/// (| – — - ·) followed by the page's registrable domain or its name (or a near form of the
/// name, `near_name`), ignoring case, spaces, dots, hyphens and apostrophes. A title that is only the site name stays, and so does any
/// tail with other punctuation (`— "Example"?`) or a page on a bare IP address.
pub(super) fn without_site_suffix<'a>(title: &'a str, page: &Url) -> &'a str {
  let Some(host) = crate::net::host(page) else {
    return title;
  };
  let host = host.trim_end_matches('.').to_ascii_lowercase();
  if host
    .trim_start_matches('[')
    .trim_end_matches(']')
    .parse::<std::net::IpAddr>()
    .is_ok()
  {
    return title;
  }
  let domain = psl::domain_str(&host).unwrap_or(&host);
  let squash = |text: &str| -> String {
    text
      .chars()
      .filter(|c| c.is_alphanumeric())
      .flat_map(char::to_lowercase)
      .collect()
  };
  let names = [
    squash(domain),
    squash(domain.split('.').next().unwrap_or_default()),
    squash(host.trim_start_matches("www.")),
  ];
  let trimmed = title.trim_end();
  let cut = trimmed
    .char_indices()
    .filter(|&(at, c)| match c {
      '|' => true,
      // Dashes and dots only between spaces: "Spider-Man" and "Paris–Lyon" are one word.
      '-' | '–' | '—' | '·' => {
        trimmed[..at].ends_with(' ') && trimmed[at + c.len_utf8()..].starts_with(' ')
      }
      _ => false,
    })
    .map(|(at, c)| (at, c.len_utf8()))
    .next_back();
  let Some((at, width)) = cut else {
    return title;
  };
  let (head, tail) = (trimmed[..at].trim_end(), &trimmed[at + width..]);
  if !tail
    .chars()
    .all(|c| c.is_alphanumeric() || c.is_whitespace() || matches!(c, '.' | '-' | '\''))
  {
    return title;
  }
  let tail = squash(tail);
  if head.is_empty() || tail.is_empty() || !(names.contains(&tail) || near_name(&tail, &names[1])) {
    return title;
  }
  head
}

/// A site name that differs from the domain's name by a short ending ("Movies4U" on
/// movies4us.net, "Movies4Us" on movies4u.top): one is the start of the other, the shorter has
/// at least 4 characters and the longer adds at most 2.
fn near_name(tail: &str, name: &str) -> bool {
  let (short, long) = if tail.len() <= name.len() {
    (tail, name)
  } else {
    (name, tail)
  };
  short.chars().count() >= 4 && long.starts_with(short) && long.len() - short.len() <= 2
}

#[derive(Clone, Copy, Default)]
pub(super) struct BrowserStatus {
  pub(super) can_back: bool,
  pub(super) can_forward: bool,
  pub(super) loading: bool,
}

/// What automatic catch reads from a hidden page.
#[path = "auto_view.rs"]
pub(super) mod auto_view;

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
