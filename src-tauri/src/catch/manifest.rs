//! Text-only inspection of HLS playlists and DASH MPDs.

use quick_xml::events::Event;
use quick_xml::reader::{NsReader, Reader};
use quick_xml::XmlVersion;

pub(super) const MAX_HLS_VARIANTS: usize = 64;
/// Common-encryption schemes whose keys are public, not a DRM system: ClearKey and the W3C common PSSH.
const CLEAR_KEY_SCHEMES: [&str; 2] = [
  "urn:uuid:e2719d58-a985-b3c9-781a-b030af78d30e",
  "urn:uuid:1077efec-c0b2-4d02-ace3-3c1e52e2fb4b",
];

/// A rendition advertised by an HLS master playlist.
#[derive(Debug, PartialEq, Eq)]
pub struct Variant {
  /// A URL reference relative to the master, or an absolute URL.
  pub uri: String,
  /// Width and height in pixels.
  pub resolution: Option<(u32, u32)>,
  /// Peak bandwidth in bits per second.
  pub bandwidth: Option<u64>,
}

#[derive(Debug, Default, PartialEq)]
pub struct ManifestInfo {
  pub has_drm: bool,
  pub is_live: bool,
  /// Full presentation duration, when available for a finite stream.
  pub duration_seconds: Option<f64>,
  pub variants: Vec<Variant>,
}

impl ManifestInfo {
  /// Short finite streams can be grouped under "other videos."
  pub fn is_other_video(&self) -> bool {
    self.duration_seconds.is_some_and(|seconds| seconds < 60.0)
  }
}

/// Inspect a manifest supplied by the caller, retaining up to 64 HLS variants.
/// Unrecognized text returns `None`.
pub fn inspect(text: &str) -> Option<ManifestInfo> {
  let text = text.strip_prefix('\u{feff}').unwrap_or(text).trim_start();
  let mut lines = text.lines();
  if lines.next()?.trim_end() == "#EXTM3U" {
    inspect_hls(lines)
  } else {
    inspect_dash(text)
  }
}

pub(super) fn looks_like_dash(prefix: &[u8]) -> bool {
  let mut reader = Reader::from_reader(prefix);
  loop {
    // A body prefix may end before the root start tag is complete.
    if reader
      .get_ref()
      .trim_ascii_start()
      .strip_prefix(b"<MPD")
      .is_some_and(|rest| {
        rest
          .first()
          .is_none_or(|b| b.is_ascii_whitespace() || matches!(b, b'>' | b'/'))
      })
    {
      return true;
    }
    match reader.read_event() {
      Ok(Event::Start(element) | Event::Empty(element)) => {
        return element.local_name().as_ref() == "MPD"
      }
      Ok(Event::Decl(_) | Event::Comment(_) | Event::PI(_)) => {}
      Ok(Event::Text(text)) if text.as_ref().trim_ascii().is_empty() => {}
      _ => return false,
    }
  }
}

fn inspect_hls<'a>(lines: impl Iterator<Item = &'a str>) -> Option<ManifestInfo> {
  let mut info = ManifestInfo::default();
  let mut master = false;
  let mut media = false;
  let mut ended = false;
  let mut pending_variant = None;
  let mut pending_duration = None;
  let mut segment_count = 0;
  let mut duration = Some(0.0);

  for line in lines.map(str::trim).filter(|line| !line.is_empty()) {
    if !line.starts_with('#') {
      if let Some(attributes) = pending_variant
        .take()
        .filter(|_| info.variants.len() < MAX_HLS_VARIANTS)
      {
        info.variants.push(variant(line, attributes));
      }
      if let Some(segment) = pending_duration.take() {
        segment_count += 1;
        duration = duration
          .zip(segment)
          .and_then(|(total, seconds)| finite(total + seconds));
      }
      continue;
    }

    let (tag, value) = line.split_once(':').unwrap_or((line, ""));
    match tag {
      "#EXT-X-KEY" | "#EXT-X-SESSION-KEY" => {
        info.has_drm |= hls_key_needs_drm(value);
      }
      "#EXT-X-STREAM-INF" => {
        master = true;
        pending_variant = Some(value);
      }
      "#EXT-X-I-FRAME-STREAM-INF" => {
        master = true;
        if info.variants.len() >= MAX_HLS_VARIANTS {
          continue;
        }
        if let Some((_, uri)) = hls_attributes(value).find(|(key, _)| *key == "URI") {
          info.variants.push(variant(uri, value));
        }
      }
      "#EXT-X-MEDIA" => master = true,
      "#EXT-X-ENDLIST" => {
        media = true;
        ended = true;
      }
      "#EXTINF" => {
        media = true;
        if pending_duration.is_some() {
          duration = None;
        }
        let seconds = value.split(',').next().and_then(parse_seconds);
        pending_duration = Some(seconds);
      }
      "#EXT-X-TARGETDURATION" | "#EXT-X-MEDIA-SEQUENCE" | "#EXT-X-PART" | "#EXT-X-MAP" => {
        media = true
      }
      _ => {}
    }
  }

  if !master && !media {
    return None;
  }
  info.is_live = media && !master && !ended;
  if media && !master && ended && segment_count > 0 && pending_duration.is_none() {
    info.duration_seconds = duration;
  }
  Some(info)
}

/// AES-128 or SAMPLE-AES with a published key (no key format, or "identity") is saveable.
/// A key system's key format (FairPlay, Widevine, PlayReady) or a FairPlay `skd://` key is DRM.
fn hls_key_needs_drm(value: &str) -> bool {
  let (mut method, mut format, mut uri) = (None, None, "");
  for (key, value) in hls_attributes(value) {
    match key {
      "METHOD" => method = Some(value),
      "KEYFORMAT" => format = Some(value),
      "URI" => uri = value,
      _ => {}
    }
  }
  !method.is_some_and(|method| method.eq_ignore_ascii_case("NONE"))
    && (format.is_some_and(|format| !format.eq_ignore_ascii_case("identity"))
      || uri
        .get(..6)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("skd://")))
}

fn hls_attributes(value: &str) -> impl Iterator<Item = (&str, &str)> {
  let mut quoted = false;
  value
    .split(move |c| {
      if c == '"' {
        quoted = !quoted;
      }
      c == ',' && !quoted
    })
    .filter_map(|attribute| {
      let (key, value) = attribute.split_once('=')?;
      let value = value.trim();
      Some((
        key.trim(),
        value
          .strip_prefix('"')
          .and_then(|v| v.strip_suffix('"'))
          .unwrap_or(value),
      ))
    })
}

fn variant(uri: &str, attributes: &str) -> Variant {
  let mut variant = Variant {
    uri: uri.into(),
    resolution: None,
    bandwidth: None,
  };
  for (key, value) in hls_attributes(attributes) {
    match key {
      "RESOLUTION" => {
        variant.resolution = value
          .split_once('x')
          .and_then(|(width, height)| Some((width.parse().ok()?, height.parse().ok()?)));
      }
      "BANDWIDTH" => variant.bandwidth = value.parse().ok(),
      _ => {}
    }
  }
  variant
}

fn inspect_dash(text: &str) -> Option<ManifestInfo> {
  let mut reader = NsReader::from_str(text);
  let mut info = ManifestInfo::default();
  let mut depth = 0;
  let mut root_seen = false;
  let mut presentation_duration = None;
  let mut period_end = Some(0.0);

  loop {
    let event = reader.read_event().ok()?;
    match event {
      Event::Start(ref element) | Event::Empty(ref element) => {
        let (_, name) = reader.resolver().resolve_element(element.name());
        if depth == 0 {
          if root_seen || name.as_ref() != "MPD" {
            return None;
          }
          root_seen = true;
        }
        let period = depth == 1 && name.as_ref() == "Period";
        let mut start = period_end;
        let mut duration = None;
        let protection = name.as_ref() == "ContentProtection";
        for attribute in element.attributes() {
          let attribute = attribute.ok()?;
          let key = attribute.key.as_ref();
          match (depth, period, key) {
            // Common encryption alone (mp4protection, ClearKey) is not DRM; a key system's UUID is.
            (_, _, "schemeIdUri") if protection => {
              let scheme = attribute.normalized_value(XmlVersion::Implicit1_0).ok()?;
              let scheme = scheme.trim();
              info.has_drm |= scheme
                .get(..9)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("urn:uuid:"))
                && !CLEAR_KEY_SCHEMES
                  .iter()
                  .any(|clear| scheme.eq_ignore_ascii_case(clear));
            }
            (0, _, "type") => {
              info.is_live = attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .ok()?
                .trim()
                == "dynamic";
            }
            (0, _, "mediaPresentationDuration") => {
              presentation_duration =
                dash_duration(&attribute.normalized_value(XmlVersion::Implicit1_0).ok()?);
            }
            (_, true, "start") => {
              start = dash_duration(&attribute.normalized_value(XmlVersion::Implicit1_0).ok()?);
            }
            (_, true, "duration") => {
              duration = dash_duration(&attribute.normalized_value(XmlVersion::Implicit1_0).ok()?);
            }
            _ => {}
          }
        }
        if period {
          period_end = start
            .zip(duration)
            .and_then(|(start, duration)| finite(start + duration));
        }
        if matches!(event, Event::Start(_)) {
          depth += 1;
        }
      }
      Event::End(_) => depth -= 1,
      Event::DocType(_) => return None,
      Event::Text(text) if depth == 0 && !text.as_ref().trim_ascii().is_empty() => return None,
      Event::CData(_) | Event::GeneralRef(_) if depth == 0 => return None,
      Event::Eof => break,
      _ => {}
    }
  }
  if !root_seen || depth != 0 {
    return None;
  }
  if !info.is_live {
    info.duration_seconds = presentation_duration.or(period_end.filter(|&duration| duration > 0.0));
  }
  Some(info)
}

fn dash_duration(value: &str) -> Option<f64> {
  let value = value.trim().strip_prefix('P')?;
  let (days, clock) = value
    .split_once('T')
    .map_or((value, None), |(days, clock)| (days, Some(clock)));
  let mut seconds = if days.is_empty() {
    0.0
  } else {
    parse_seconds(days.strip_suffix('D')?)? * 86_400.0
  };

  let Some(mut clock) = clock else {
    return (!days.is_empty()).then_some(finite(seconds)?);
  };
  if clock.is_empty() {
    return None;
  }
  let mut previous = 4_000.0;
  while !clock.is_empty() {
    let unit_at = clock.find(|c: char| !c.is_ascii_digit() && c != '.')?;
    let amount = parse_seconds(&clock[..unit_at])?;
    let multiplier = match clock.as_bytes()[unit_at] {
      b'H' => 3_600.0,
      b'M' => 60.0,
      b'S' => 1.0,
      _ => return None,
    };
    if multiplier >= previous {
      return None;
    }
    seconds += amount * multiplier;
    previous = multiplier;
    clock = &clock[unit_at + 1..];
  }
  finite(seconds)
}

fn parse_seconds(value: &str) -> Option<f64> {
  let value = value.trim();
  if !value
    .bytes()
    .all(|byte| byte.is_ascii_digit() || byte == b'.')
  {
    return None;
  }
  finite(value.parse().ok()?)
}

fn finite(seconds: f64) -> Option<f64> {
  seconds.is_finite().then_some(seconds)
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
