use super::*;

fn observation(url: &str) -> Observation<'_> {
  Observation {
    url,
    requested_url: None,
    content_type: None,
    frame_url: "https://example.org/player",
    frame_origin: "null",
    source: Source::Performance,
    first_seen: Instant::now(),
    first_bytes: &[],
    is_byte_range: false,
    is_hls_segment: false,
    loaded: false,
    eme_probe: false,
    eme_active: false,
  }
}

#[test]
fn rejects_unsafe_urls_and_accepts_public_hosts() {
  let rejected = [
    "blob:https://example.org/a",
    "data:video/mp4;base64,AAAA",
    "ftp://example.org/video",
    "//example.org/a",
    "https:/example.org/a",
    "https://user:pass@example.org/a",
    "https://@example.org/a",
    "https://:pass@example.org/a",
    "https://localHOST./a",
    "https://player.localhost/a",
    "https://127.0.0.1/a",
    "https://127.1/a",
    "https://2130706433/a",
    "https://0x7f000001/a",
    "https://0x7f.0.0.1/a",
    "https://0177.0.0.1/a",
    "https://017700000001/a",
    "https://%31%32%37.0.0.1/a",
    "https://10.2.3.4/a",
    "https://172.31.3.4/a",
    "https://192.168.1.1/a",
    "https://169.254.1.1/a",
    "https://[::1]/a",
    "https://[fd00::1]/a",
    "https://[fc00::1]/a",
    "https://[febf::1]/a",
    "https://[::ffff:127.0.0.1]/a",
    "https://[::ffff:c0a8:101]/a",
    "https://[::192.168.1.1]/a",
    "https://[64:ff9b::c0a8:101]/a",
    "https://example.org/a\nb",
    "https://example.org/a\u{85}b",
    "https://example.org/a\u{202e}b",
    "https://example.org/a\u{2067}b",
    "https://example.org/a%0db",
    "https://example.org/a%C2%85b",
    "https://example.org/a%FF%E2%80%AEb",
    "https://example.org\\@example.org/a",
  ];
  for url in rejected {
    assert!(
      CandidateSet::default().observe(observation(url)).is_none(),
      "{url:?}"
    );
  }

  for url in [
    "https://example.org/video",
    "http://localhost.example.org/video",
    "https://8.8.8.8/video",
    "https://0x08080808/video",
    "https://[2606:4700::1111]/video",
    "https://example.org/فيديو%20أول",
  ] {
    assert!(
      CandidateSet::default().observe(observation(url)).is_some(),
      "{url}"
    );
  }
}

#[test]
fn enforces_eight_kibibytes() {
  let prefix = "https://example.org/";
  let at_limit = format!("{prefix}{}", "a".repeat(8192 - prefix.len()));
  let over_limit = format!("{at_limit}a");
  let mut candidates = CandidateSet::default();
  assert!(candidates.observe(observation(&at_limit)).is_some());
  assert!(candidates.observe(observation(&over_limit)).is_none());
}

#[test]
fn classifies_from_mime_path_and_manifest_bytes() {
  use Kind::*;

  for (path, content_type, bytes, expected) in [
    (
      "/a",
      Some("Application/Vnd.Apple.MpegURL; charset=utf-8"),
      &b""[..],
      Hls,
    ),
    ("/a", Some("audio/x-mpegurl"), &b""[..], Hls),
    ("/a", Some("application/dash+xml"), &b""[..], Dash),
    ("/a", Some("video/mp4"), &b""[..], Direct),
    ("/a", Some("application/ogg"), &b""[..], Direct),
    ("/a.aac", None, &b""[..], Direct),
    ("/a.aac", Some("audio/aac"), &b""[..], Direct),
    ("/a.mp3", None, &b""[..], Direct),
    ("/a.m4a", None, &b""[..], Direct),
    ("/a.M3U8?x=.mpd", None, &b""[..], Hls),
    ("/a.mpd", Some("video/mp4"), &b""[..], Dash),
    (
      "/a.webm",
      Some("application/octet-stream"),
      &b""[..],
      Direct,
    ),
    ("/a%2Em3u8", None, &b""[..], Hls),
    (
      "/a.mp4",
      Some("video/mp4"),
      &b"\xef\xbb\xbf#EXTM3U\r\n"[..],
      Hls,
    ),
    ("/a", None, &b"<MPD"[..], Dash),
    (
      "/a",
      Some("text/xml"),
      &b"<?xml version=\"1.0\"?>\n<MPD type=\"static\">"[..],
      Dash,
    ),
    (
      "/a",
      None,
      &b"<!-- fake -->\n<d:MPD xmlns:d='urn:mpeg:dash:schema:mpd:2011'>"[..],
      Dash,
    ),
    ("/a", None, &b"<MPDExample>"[..], Unknown),
    ("/a", None, &b"#EXTM3Uexample\n"[..], Unknown),
    ("/a?x=.mp4", Some("text/html"), &b""[..], Unknown),
  ] {
    let url = format!("https://example.org{path}");
    let hit = Observation {
      content_type,
      first_bytes: bytes,
      ..observation(&url)
    };
    assert_eq!(
      CandidateSet::default().observe(hit).map(|item| item.kind),
      Some(expected),
      "{path} {content_type:?} {bytes:?}"
    );
  }
}

#[test]
fn drops_segments_and_byte_range_media() {
  for (path, content_type, is_byte_range, is_hls_segment) in [
    ("/video.TS?token=fake", Some("video/mp4"), false, false),
    ("/chunk.m4s", None, false, false),
    ("/chunk.aac", Some("audio/iso.segment"), false, false),
    ("/chunk.aac", Some("audio/aac"), false, true),
    ("/chunk.mp3", None, false, true),
    ("/chunk.vtt", None, false, false),
    ("/video.mp4", None, true, false),
    ("/segment", Some("video/mp2t"), false, false),
    ("/segment", Some("video/iso.segment"), false, false),
    ("/segment", Some("text/vtt"), false, false),
  ] {
    let url = format!("https://example.org{path}");
    let hit = Observation {
      content_type,
      is_byte_range,
      is_hls_segment,
      ..observation(&url)
    };
    assert!(CandidateSet::default().observe(hit).is_none(), "{path}");
  }
}

#[test]
fn accumulates_eme_signals_and_flags_drm_only_for_active_eme() {
  let url = "https://example.org/video.mp4";
  let mut candidates = CandidateSet::default();
  let probe = Observation {
    eme_probe: true,
    ..observation(url)
  };
  let candidate = candidates.observe(probe).unwrap();
  assert!(candidate.eme_probe);
  assert!(!candidate.has_drm());

  let active = Observation {
    eme_active: true,
    ..observation(url)
  };
  let candidate = candidates.observe(active).unwrap();
  assert!(candidate.eme_probe);
  assert!(candidate.eme_active);
  assert!(candidate.has_drm());
  assert!(candidates.observe(observation(url)).unwrap().has_drm());
}

#[test]
fn rejects_oversized_metadata_before_storing_or_enriching() {
  let url = "https://example.org/video";
  let content_type = "a".repeat(256);
  let frame_url = "b".repeat(8192);
  let frame_origin = "c".repeat(8192);
  let oversized_type = format!("{}é", "a".repeat(255));
  let oversized_url = format!("{}é", "b".repeat(8191));
  let oversized_origin = format!("{}é", "c".repeat(8191));
  let mut candidates = CandidateSet::default();

  let admitted = candidates
    .observe(Observation {
      content_type: Some(&content_type),
      frame_url: &frame_url,
      frame_origin: &frame_origin,
      ..observation(url)
    })
    .unwrap();
  assert_eq!(
    admitted.content_type.as_deref(),
    Some(content_type.as_str())
  );
  assert_eq!(admitted.frame_url, frame_url);
  assert_eq!(admitted.frame_origin, frame_origin);

  for (content_type, frame_url, frame_origin) in [
    (
      Some(oversized_type.as_str()),
      frame_url.as_str(),
      frame_origin.as_str(),
    ),
    (
      Some("application/dash+xml"),
      oversized_url.as_str(),
      frame_origin.as_str(),
    ),
    (
      Some("application/dash+xml"),
      frame_url.as_str(),
      oversized_origin.as_str(),
    ),
  ] {
    let hit = Observation {
      content_type,
      frame_url,
      frame_origin,
      ..observation(url)
    };
    assert!(candidates.observe(hit).is_none());
  }
  assert_eq!(candidates.iter().next().unwrap().kind, Kind::Unknown);
}

#[test]
fn limits_candidates_per_window_and_enriches_existing_hits_at_capacity() {
  let mut candidates = CandidateSet::default();
  for i in 0..200 {
    let url = format!("https://example.org/{i}");
    assert!(candidates.observe(observation(&url)).is_some());
  }
  assert!(candidates
    .observe(observation("https://example.org/extra"))
    .is_none());

  let hit = Observation {
    content_type: Some("video/mp4"),
    ..observation("https://EXAMPLE.ORG:443/0#player")
  };
  assert_eq!(candidates.observe(hit).unwrap().kind, Kind::Direct);
  assert_eq!(candidates.iter().count(), 200);
}

#[test]
fn deduplicates_and_enriches_without_changing_first_seen() {
  let mut candidates = CandidateSet::default();
  let first = observation("HTTPS://EXAMPLE.ORG:443/old/../video?token=one#fragment");
  let time = first.first_seen;
  assert_eq!(candidates.observe(first).unwrap().kind, Kind::Unknown);
  let second = Observation {
    content_type: Some("application/dash+xml"),
    source: Source::Fetch,
    ..observation("https://example.org/video?token=one")
  };
  let candidate = candidates.observe(second).unwrap();
  assert_eq!(
    candidate.url().as_str(),
    "https://example.org/video?token=one"
  );
  assert_eq!(candidate.kind, Kind::Dash);
  assert_eq!(candidate.first_seen, time);
  assert_eq!(candidate.source, Source::Performance);
  assert_eq!(candidate.frame_origin, "null");
  candidates
    .observe(observation("https://example.org/video?token=two"))
    .unwrap();
  assert_eq!(candidates.iter().count(), 2);
}

#[test]
fn suppresses_known_variants_before_or_after_the_master() {
  let mut candidates = CandidateSet::default();
  let master = "https://example.org/hls/master.m3u8";
  let first_variant = "https://EXAMPLE.ORG:443/hls/360/playlist.m3u8?token=fake";
  let second_variant = "https://example.org/hls/720.m3u8#player";
  candidates.observe(observation(first_variant)).unwrap();
  let master = candidates
    .observe(observation(master))
    .unwrap()
    .url()
    .clone();
  let manifest = super::super::manifest::inspect(include_str!("fixtures/master.m3u8")).unwrap();
  candidates.suppress_hls_variants(&master, &manifest.variants);

  assert_eq!(
    candidates.iter().map(Candidate::url).collect::<Vec<_>>(),
    [&master]
  );
  assert!(candidates.observe(observation(first_variant)).is_none());
  assert!(candidates.observe(observation(second_variant)).is_none());
  assert!(candidates
    .observe(observation(
      "https://example.org/hls/360/playlist.m3u8?token=other"
    ))
    .is_some());
}

#[test]
fn retains_at_most_64_variants_for_each_normalized_master() {
  let mut candidates = CandidateSet::default();
  let first = Url::parse("https://example.org/first/master.m3u8").unwrap();
  let second = Url::parse("https://example.org/second/master.m3u8").unwrap();
  let variants: Vec<_> = (0..65)
    .map(|i| Variant {
      uri: format!("{i}.m3u8"),
      bandwidth: None,
      resolution: None,
    })
    .collect();
  for master in [&first, &second] {
    candidates.suppress_hls_variants(master, &variants);
    assert!(candidates
      .observe(observation(master.join("63.m3u8").unwrap().as_str()))
      .is_none());
    assert!(candidates
      .observe(observation(master.join("64.m3u8").unwrap().as_str()))
      .is_some());
  }

  let mut refreshed = first.clone();
  refreshed.set_fragment(Some("player"));
  candidates.suppress_hls_variants(&refreshed, &variants[1..]);
  assert!(candidates
    .observe(observation(first.join("0.m3u8").unwrap().as_str()))
    .is_some());
  assert!(candidates
    .observe(observation(first.join("64.m3u8").unwrap().as_str()))
    .is_none());
  assert!(candidates
    .observe(observation(second.join("0.m3u8").unwrap().as_str()))
    .is_none());
}
