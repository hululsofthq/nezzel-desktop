//! The hidden page as automatic catch sees it: the same candidates and phase the bar would show.

use super::*;
use crate::catch::auto::{site_of, Candidate, View};

#[derive(Deserialize)]
struct MeasuredVideo(String, f64, f64, Option<f64>);

// One bounded read at the look deadline. Take the hook snapshot last so events queued while
// reading the DOM are included. Return only a player flag and geometry for top-frame videos.
pub(in crate::catch) const FINAL_SNAPSHOT: &str = r#"(() => {
  try {
    if (document.readyState !== 'complete') return '';
    const videos = Array.from(document.querySelectorAll('video'));
    const media = videos.slice(0, 64).flatMap(video => {
      const src = video.currentSrc || video.src;
      if (!src || src.length > 8192) return [];
      const { width, height } = video.getBoundingClientRect();
      const style = getComputedStyle(video);
      return style.visibility === 'visible' && Number(style.opacity) > 0
        ? [[src, width, height, video.duration]] : [];
    });
    const play = /\b(?:play|watch|regarder)\b|تشغيل|شاهد|مشاهدة/i;
    const store = /\b(?:google[-\s]+play|play[-\s]+store)\b/i;
    const player = /(?:^|\s)(?:[\w-]+-)?(?:(?:video|movie|media)-player|player-(?:main|api))(?:\s|$)/i;
    const hasPlayer = videos.length > 0 || !!document.querySelector('audio') ||
      Array.from(document.querySelectorAll('iframe')).some(frame => {
        const { width, height } = frame.getBoundingClientRect();
        return width >= 300 && height >= 150 && (frame.hasAttribute('allowfullscreen') ||
          /(?:^|;)\s*(?:autoplay|fullscreen)(?=\s|;|$)/i.test(frame.getAttribute('allow') || ''));
      }) || Array.from(document.querySelectorAll('button, [role="button"]')).some(el => {
        const label = [el.getAttribute('aria-label'), el.getAttribute('title'), el.getAttribute('class')].join(' ');
        return play.test(label) && !store.test(label);
      }) || Array.from(document.querySelectorAll('[class*="player"]')).some(el => player.test(el.className));
    const snapshot = window.__nezzelCatch?.snapshot();
    return typeof snapshot === 'string' ? JSON.stringify([snapshot, hasPlayer, media]) : '';
  } catch { return ''; }
})()"#;

impl PageState {
  pub(in crate::catch) fn auto_view(&self, loading: bool) -> View {
    let live_blob_frames = self.document.live_blob_frames();
    let (candidates, drm, live) =
      self
        .document
        .candidates_with_live_blobs(&self.title, &self.saved_urls, &live_blob_frames);
    let phase = phase(
      self.document.crashed,
      false,
      drm,
      live || !live_blob_frames.is_empty(),
      self.document.geo_blocked,
      &candidates,
      self.document.played_sec as u32,
    );
    let mains = candidates
      .iter()
      .filter(|candidate| candidate.main)
      .filter_map(|candidate| {
        let source = self.document.candidates.iter().find(|source| {
          self
            .document
            .details
            .get(source.url())
            .is_some_and(|details| {
              details.id == candidate.id && (candidate.played || details.rescued || details.page)
            })
        })?;
        Some(Candidate {
          id: candidate.id.clone(),
          kind: match candidate.kind {
            MediaKind::Hls => "hls",
            MediaKind::Dash => "dash",
            MediaKind::Direct => "direct",
          },
          duration: candidate.duration_sec,
          loaded_host: candidate.loaded_host,
          sites: [
            site_of(source.url()),
            Url::parse(&source.frame_url)
              .ok()
              .as_ref()
              .and_then(site_of),
          ],
        })
      })
      .collect();
    View {
      top: Some(self.url.clone()).filter(is_http),
      loading,
      title: self.title.clone(),
      phase,
      has_player: self.document.player_seen
        || !self.document.elements.is_empty()
        || self.document.candidates.iter().next().is_some(),
      mains,
      lone: None,
      pictures: crate::catch::thumbnail::pictures(
        self.document.pictures.0.clone(),
        self.document.pictures.1.clone(),
      ),
    }
  }

  pub(in crate::catch) fn auto_final_view(&mut self, raw: &str) -> Option<View> {
    if raw.len() > 4 * MAX_DRAIN_BYTES {
      return None;
    }
    let raw: String = serde_json::from_str(raw).ok()?;
    let (snapshot, has_player, media): (String, bool, Vec<MeasuredVideo>) =
      serde_json::from_str(&raw).ok()?;
    let top = page_url(self.url.as_str())?;
    if !self.apply_snapshot(&top, &snapshot) {
      return None;
    }
    let mut view = self.auto_view(false);
    view.has_player |= has_player || self.password_visible;
    if self.document.played || !matches!(view.phase, Phase::Looking | Phase::Found) {
      return Some(view);
    }
    // A rendition its master lists is the same video, on whichever host it is served.
    let candidates = &self.document.candidates;
    let rendition = |stream: &Url| {
      candidates.iter().any(|master| {
        master.url() != stream
          && self
            .document
            .details
            .get(master.url())
            .and_then(|details| details.manifest.as_ref())
            .is_some_and(|manifest| {
              manifest.variants.iter().any(|variant| {
                master
                  .url()
                  .join(&variant.uri)
                  .is_ok_and(|url| &url == stream)
              })
            })
      })
    };
    let mut streams = candidates
      .iter()
      .filter(|c| c.kind != Kind::Unknown && !rendition(c.url()));
    let Some(stream) = streams.next() else {
      return Some(view);
    };
    let details = self.document.details.get(stream.url())?;
    if streams.next().is_some()
      || !details.top_frame
      || stream.frame_url != top.as_str()
      || stream.frame_origin == "null"
      || stream.has_drm()
      || self.document.elements.iter().any(|((frame, _), element)| {
        frame == &top
          && element.media.is_none()
          && (element.encrypted || element.previous_encryption.is_some())
      })
    {
      return Some(view);
    }
    let Some(MeasuredVideo(_, _, _, measured)) =
      media.iter().find(|MeasuredVideo(src, width, height, _)| {
        *width >= 24.0
          && *height >= 24.0
          && page_url(src).is_some_and(|src| &src == stream.url() || &src == stream.download_url())
      })
    else {
      return Some(view);
    };
    let (kind, duration) = match stream.kind {
      Kind::Hls | Kind::Dash => {
        let duration = details
          .manifest
          .as_ref()
          .filter(|m| !m.has_drm && !m.is_live)
          .and_then(|m| m.duration_seconds);
        (
          if stream.kind == Kind::Hls {
            "hls"
          } else {
            "dash"
          },
          duration,
        )
      }
      Kind::Direct => ("direct", *measured),
      Kind::Unknown => return Some(view),
    };
    if let Some(duration) = duration.filter(|d| (60.0..=1.0e9).contains(d)) {
      view.lone = Some(Candidate {
        id: details.id.clone(),
        kind,
        duration: Some(duration),
        loaded_host: self.document.candidates.loaded_host(stream),
        sites: [site_of(stream.url()), site_of(&top)],
      });
    }
    Some(view)
  }
}

#[cfg(feature = "catch-matrix")]
impl PageState {
  /// What the page showed, for the developer's sweep table: sites and counts, never addresses.
  pub(in crate::catch) fn matrix_note(&self, loading: bool) -> serde_json::Value {
    let view = self.auto_view(loading);
    let observed: Vec<_> = self
      .document
      .candidates
      .iter()
      .take(16)
      .map(|candidate| {
        let details = self.document.details.get(candidate.url());
        let manifest = details.and_then(|details| details.manifest.as_ref());
        serde_json::json!({
          "kind": format!("{:?}", candidate.kind),
          "source": format!("{:?}", candidate.source),
          "site": site_of(candidate.url()),
          "frame": Url::parse(&candidate.frame_url).ok().as_ref().and_then(site_of),
          "mime": candidate.content_type,
          "drm": candidate.has_drm() || manifest.is_some_and(|manifest| manifest.has_drm),
          "live": manifest.is_some_and(|manifest| manifest.is_live),
          "inspected": manifest.is_some(),
          "duration": manifest.and_then(|manifest| manifest.duration_seconds)
            .or(details.and_then(|details| details.duration)),
          "playing": details.is_some_and(|details| details.playing),
        })
      })
      .collect();
    serde_json::json!({
      "top": view.top.as_ref().and_then(site_of),
      "loading": loading,
      "phase": view.phase,
      "mains": view.mains.len(),
      "observed": observed,
      "elements": self.document.elements.len(),
      "encryptedElements": self.document.elements.values().filter(|e| e.encrypted).count(),
      "elementDurations": self.document.elements.values().filter_map(|e| e.duration).collect::<Vec<_>>(),
      "blobElements": self.document.elements.values().filter(|e| e.media.is_none()).count(),
      "playedSec": self.document.played_sec,
      "adsBlocked": self.ads_blocked,
      "titled": self.title.is_some(),
    })
  }
}
