use super::*;
use serde_json::{json, Value};

fn page() -> PageState {
  PageState::new(Url::parse("https://example.com/watch").unwrap())
}

fn hit(method: &str, url: Option<&str>, extra: Value) -> Value {
  let mut value = json!({
    "method": method, "url": url, "frame": "https://example.com/watch", "isTop": true
  });
  value
    .as_object_mut()
    .unwrap()
    .extend(extra.as_object().unwrap().clone());
  value
}

/// What the engine hands back for the hook's snapshot: a JSON string holding the JSON array.
fn snapshot(parts: Value) -> String {
  serde_json::to_string(&parts.to_string()).unwrap()
}

/// The retry ladder of this page has run out: an encrypted verdict shows.
fn exhaust_retries(page: &mut PageState) {
  page.ladder = Ladder {
    page: Some(page.url.clone()),
    started: Instant::now(),
    step: Step::Done,
  };
}

fn drain(page: &mut PageState, hits: Vec<Value>) {
  let url = page.url.clone();
  let data = snapshot(json!([url.as_str(), "Video", "Test browser", hits]));
  page.apply_drain(&url, &data);
}

#[test]
fn auto_catch_sees_player_evidence_even_without_a_main_candidate() {
  for evidence in [
    hit("player.present", None, json!({})),
    hit(
      "fetch",
      Some("https://cdn.example.org/video.m3u8"),
      json!({}),
    ),
  ] {
    let mut page = page();
    assert!(!page.auto_view(false).has_player);
    drain(&mut page, vec![evidence]);
    let view = page.auto_view(false);
    assert!(view.has_player);
    assert!(view.mains.is_empty());
    page.navigated(Url::parse("https://example.com/other").unwrap());
    assert!(!page.auto_view(false).has_player);
  }
}

#[test]
fn automatic_catch_distinguishes_live_playlists_from_vod_regardless_of_mime() {
  let url = "https://cdn.example.org/video.m3u8";
  for (mime, response, endlist, film_duration, expected) in [
    (
      "application/xml",
      "https://cdn.example.org/payload.xml",
      "",
      None,
      Phase::Live,
    ),
    (
      "application/xml",
      "https://cdn.example.org/payload.xml",
      "",
      Some(1200.0),
      Phase::Found,
    ),
    (
      "application/xml",
      "https://cdn.example.org/payload.xml",
      "",
      Some(75.0),
      Phase::Found,
    ),
    (
      "application/javascript",
      "https://cdn.example.org/payload.js",
      "#EXT-X-ENDLIST\n",
      Some(75.0),
      Phase::Found,
    ),
  ] {
    let mut page = page();
    let mut hits = vec![framed_hit(
      "xhr",
      Some(response),
      json!({"ct": mime, "requestedUrl": url}),
    )];
    if let Some(duration) = film_duration {
      hits.push(hit(
        "playing",
        Some("https://cdn.example.org/film.mp4"),
        json!({"playing": true, "durationSec": duration, "mediaId": 1}),
      ));
    }
    drain(&mut page, hits);
    let (job, address) = page.document.next_manifest().unwrap();
    assert_eq!(job.as_str(), url);
    assert_eq!(address.as_str(), response);
    let body = format!("#EXTM3U\n#EXTINF:80,\na.ts\n{endlist}");
    page.document.inspected_response(
      &job,
      &address,
      super::super::manifest::inspect(&body).unwrap(),
    );
    let view = page.auto_view(false);
    assert_eq!(view.phase, expected, "{mime}");
    assert_eq!(
      view
        .mains
        .iter()
        .map(|main| main.duration)
        .collect::<Vec<_>>(),
      film_duration.map(Some).into_iter().collect::<Vec<_>>()
    );
  }
}

#[test]
fn only_a_lone_measured_top_frame_video_can_be_offered_without_playback() {
  let hls = "https://cdn.example.org/film.m3u8";
  let mp4 = "https://cdn.example.org/film.mp4";
  for (label, address, width, measured, inspected, third_party, extra, offer) in [
    (
      "long direct video",
      mp4,
      640,
      Some(80.0),
      None,
      false,
      false,
      true,
    ),
    (
      "long VOD playlist",
      hls,
      640,
      None,
      Some(80.0),
      false,
      false,
      true,
    ),
    (
      "short unplayed video",
      mp4,
      640,
      Some(8.0),
      None,
      false,
      false,
      false,
    ),
    (
      "tracker even with duration",
      mp4,
      1,
      Some(80.0),
      None,
      false,
      false,
      false,
    ),
    (
      "third-party frame",
      hls,
      640,
      None,
      Some(80.0),
      true,
      false,
      false,
    ),
    (
      "inspection pending",
      hls,
      640,
      Some(80.0),
      None,
      false,
      false,
      false,
    ),
    (
      "two streams",
      mp4,
      640,
      Some(80.0),
      None,
      false,
      true,
      false,
    ),
  ] {
    let mut page = page();
    let frame = if third_party {
      json!({"isTop": false, "frame": "https://ad.example.net/embed", "msgOrigin": "https://ad.example.net"})
    } else {
      json!({})
    };
    let mut hits = vec![hit("video.currentSrc", Some(address), frame)];
    if extra {
      hits.push(hit("fetch", Some(hls), json!({})));
    }
    drain(&mut page, hits);
    if let Some(seconds) = inspected {
      page.document.inspected(
        &Url::parse(address).unwrap(),
        ManifestInfo {
          duration_seconds: Some(seconds),
          ..ManifestInfo::default()
        },
      );
    }
    assert!(page.auto_view(false).mains.is_empty(), "{label}");
    let hook = json!([page.url.as_str(), "Video", "Test browser", []]).to_string();
    let raw = snapshot(json!([hook, true, [[address, width, 360, measured]]]));
    let view = page.auto_final_view(&raw).unwrap();
    assert!(view.has_player);
    assert_eq!(
      view.lone.and_then(|c| c.duration),
      offer.then_some(80.0),
      "{label}"
    );
  }
}

#[test]
fn a_rendition_its_master_lists_on_another_host_is_not_a_second_stream() {
  use super::super::manifest::Variant;
  // As on DW: the page's master lists absolute renditions on a CDN host the player then loads.
  let master = "https://vod.example.org/film/master.m3u8";
  let rendition = "https://cdn.example.net/film/index-f1.m3u8";
  for (listed, offer) in [(true, true), (false, false)] {
    let mut page = page();
    drain(
      &mut page,
      vec![
        hit("video.currentSrc", Some(master), json!({})),
        hit("perf:video", Some(rendition), json!({})),
      ],
    );
    page.document.inspected(
      &Url::parse(master).unwrap(),
      ManifestInfo {
        duration_seconds: Some(150.0),
        variants: listed
          .then(|| Variant {
            uri: rendition.into(),
            resolution: Some((480, 270)),
            bandwidth: None,
          })
          .into_iter()
          .collect(),
        ..ManifestInfo::default()
      },
    );
    let hook = json!([page.url.as_str(), "Video", "Test browser", []]).to_string();
    let raw = snapshot(json!([hook, true, [[master, 640, 360, 150.0]]]));
    let view = page.auto_final_view(&raw).unwrap();
    assert_eq!(
      view.lone.and_then(|c| c.duration),
      offer.then_some(150.0),
      "listed: {listed}"
    );
  }
}

const BLOB_FRAME: &str = "https://player.example.net/embed";

fn framed_hit(method: &str, url: Option<&str>, extra: Value) -> Value {
  let mut hit = hit(method, url, extra);
  hit["isTop"] = json!(false);
  hit["frame"] = json!(BLOB_FRAME);
  hit["msgOrigin"] = json!("https://player.example.net");
  hit
}

fn blob_drains(page: &mut PageState, samples: &[&[f64]]) {
  let start = Instant::now();
  for (index, durations) in samples.iter().enumerate() {
    let hits: Vec<_> = durations
      .iter()
      .map(|duration| {
        framed_hit(
          "playing",
          None,
          json!({"playing": true, "durationSec": duration, "mediaId": 1}),
        )
      })
      .collect();
    page.document.drain(
      &serde_json::to_string(&hits).unwrap(),
      &page.url,
      start + Duration::from_secs(index as u64),
    );
  }
}

#[test]
fn a_blob_source_swap_does_not_look_live() {
  let cases: &[&[&[f64]]] = &[
    &[&[30.0], &[1500.0]],
    &[&[30.0], &[31.0], &[1500.0], &[1501.0]],
    &[&[30.0], &[], &[32.0], &[33.0]],
  ];
  for samples in cases {
    let mut page = page();
    blob_drains(&mut page, samples);
    assert_eq!(page.auto_view(false).phase, Phase::Looking, "{samples:?}");
  }
}

#[test]
fn a_six_hour_blob_vod_does_not_look_live() {
  let mut page = page();
  blob_drains(&mut page, &[&[50390.0], &[50390.0]]);
  assert_eq!(page.auto_view(false).phase, Phase::Looking);
}

#[test]
fn a_blob_needs_two_consecutive_steady_increases_to_look_live() {
  let cases: &[(&[&[f64]], Phase)] = &[
    (&[&[100.0, 101.0, 102.0]], Phase::Looking),
    (&[&[100.0], &[101.0]], Phase::Looking),
    (&[&[100.0], &[101.0], &[101.0], &[102.0]], Phase::Looking),
    (&[&[100.0], &[101.0], &[102.0]], Phase::Live),
  ];
  for &(samples, expected) in cases {
    let mut page = page();
    blob_drains(&mut page, samples);
    assert_eq!(page.auto_view(false).phase, expected, "{samples:?}");
  }
}

#[test]
fn a_live_blob_only_suppresses_its_frames_blob_mapped_stream() {
  let playlist = "https://cdn.example.org/stream.m3u8";
  for (verified_vod, film_is_top, expected, durations) in [
    (false, None, Phase::Live, vec![]),
    (false, Some(true), Phase::Found, vec![Some(1200.0)]),
    (false, Some(false), Phase::Found, vec![Some(1200.0)]),
    (true, None, Phase::Found, vec![Some(80.0)]),
  ] {
    let mut page = page();
    let mut hits = vec![framed_hit("xhr", Some(playlist), json!({}))];
    if let Some(film_is_top) = film_is_top {
      let film_hit = if film_is_top { hit } else { framed_hit };
      hits.push(film_hit(
        "playing",
        Some("https://cdn.example.org/film.mp4"),
        json!({"playing": true, "durationSec": 1200, "mediaId": 2}),
      ));
    }
    drain(&mut page, hits);
    if verified_vod {
      page.document.inspected(
        &Url::parse(playlist).unwrap(),
        ManifestInfo {
          duration_seconds: Some(80.0),
          ..ManifestInfo::default()
        },
      );
    }
    blob_drains(&mut page, &[&[100.0], &[101.0], &[102.0]]);
    let view = page.auto_view(false);
    assert_eq!(view.phase, expected);
    assert_eq!(
      view
        .mains
        .iter()
        .map(|main| main.duration)
        .collect::<Vec<_>>(),
      durations
    );
  }
}

#[test]
fn phases_follow_duration_playback_protection_and_save() {
  let media = Some("https://cdn.example.org/video.mp4");
  let hls = "https://cdn.example.org/video.m3u8";
  let cases = [
    ("empty", vec![], None, 0.0, false, Phase::Looking),
    ("9.9 seconds", vec![], None, 9.9, false, Phase::Looking),
    ("10 seconds", vec![], None, 10.0, false, Phase::NotFound),
    (
      "unknown duration, idle",
      vec![hit("video.src", media, json!({}))],
      None,
      0.0,
      false,
      Phase::Looking,
    ),
    (
      "unknown duration, playing",
      vec![hit("playing", media, json!({"playing": true}))],
      None,
      0.0,
      false,
      Phase::Found,
    ),
    (
      "short",
      vec![hit("video.currentSrc", media, json!({"durationSec": 59.9}))],
      None,
      10.0,
      false,
      Phase::AdPlaying,
    ),
    (
      "main at 60",
      vec![hit("video.currentSrc", media, json!({"durationSec": 60}))],
      None,
      0.0,
      false,
      Phase::Found,
    ),
    (
      "clear EME probe",
      vec![hit(
        "requestMediaKeySystemAccess",
        None,
        json!({"emeProbe": true}),
      )],
      None,
      0.0,
      false,
      Phase::Looking,
    ),
    (
      "active EME",
      vec![hit(
        "encrypted",
        None,
        json!({"emeActive": true, "durationSec": 3600, "mediaId": 1}),
      )],
      None,
      0.0,
      false,
      Phase::Drm,
    ),
    (
      "keys attached without an encrypted event",
      vec![
        hit(
          "setMediaKeys",
          None,
          json!({"emeActive": true, "mediaId": 1}),
        ),
        hit(
          "playing",
          None,
          json!({"playing": true, "durationSec": 3600, "mediaId": 1}),
        ),
      ],
      None,
      0.0,
      false,
      Phase::Looking,
    ),
    (
      "live manifest",
      vec![hit("fetch", Some(hls), json!({}))],
      Some(ManifestInfo {
        is_live: true,
        ..Default::default()
      }),
      10.0,
      false,
      Phase::Live,
    ),
    (
      "protected manifest",
      vec![hit("fetch", Some(hls), json!({}))],
      Some(ManifestInfo {
        has_drm: true,
        ..Default::default()
      }),
      10.0,
      false,
      Phase::Drm,
    ),
    ("queued", vec![], None, 10.0, true, Phase::Saving),
  ];
  for (name, hits, manifest, played, saved, expected) in cases {
    let mut page = page();
    drain(&mut page, hits);
    if let Some(info) = manifest {
      page.document.inspected(&Url::parse(hls).unwrap(), info);
    }
    page.document.played_sec = played;
    page.document.saved = saved.then(|| "download-1".into());
    exhaust_retries(&mut page);
    assert_eq!(
      page
        .changed("catch-id", BrowserStatus::default())
        .unwrap()
        .phase,
      expected,
      "{name}"
    );
  }
}

#[test]
fn drain_drops_bad_entries_and_origins_but_accepts_cross_origin_and_opaque_frames() {
  let mut page = page();
  let good = hit(
    "video.src",
    Some("https://cdn.example.org/good.mp4"),
    json!({"durationSec": 80}),
  );
  let forged_top = hit(
    "video.src",
    Some("https://cdn.example.org/bad1.mp4"),
    json!({"frame": "https://example.com/other", "durationSec": 80}),
  );
  let forged_origin = hit(
    "video.src",
    Some("https://cdn.example.org/bad2.mp4"),
    json!({"frame": "https://player.example.org/frame", "isTop": false, "msgOrigin": "https://evil.example.net", "durationSec": 80, "emeActive": true}),
  );
  let wrong_types = hit(
    "playing",
    Some("https://cdn.example.org/bad3.mp4"),
    json!({"playing": "true", "durationSec": 80}),
  );
  let private_host = hit(
    "video.src",
    Some("http://127.1/private.mp4"),
    json!({"durationSec": 80}),
  );
  let opaque = hit(
    "video.src",
    Some("https://cdn.example.org/opaque.mp4"),
    json!({"frame": "https://sandbox.example.net/embed", "isTop": false, "msgOrigin": "null", "durationSec": 80}),
  );
  let nested = hit(
    "video.src",
    Some("https://cdn.example.org/nested.mp4"),
    json!({"frame": "https://player.example.net/embed", "isTop": false, "msgOrigin": "https://player.example.net", "durationSec": 80}),
  );
  drain(
    &mut page,
    vec![
      json!(true),
      good,
      forged_top,
      forged_origin,
      wrong_types,
      private_host,
      opaque,
      nested,
    ],
  );
  let urls: Vec<_> = page
    .document
    .candidates
    .iter()
    .map(|c| c.url().path())
    .collect();
  assert_eq!(urls, ["/good.mp4", "/opaque.mp4", "/nested.mp4"]);
  assert!(!page.document.has_drm());
}

#[test]
fn drain_rejects_oversized_batches_and_stale_document_results() {
  let mut page = page();
  let valid = hit(
    "playing",
    None,
    json!({"playing": true, "playDeltaSec": 1.0}),
  );
  drain(&mut page, vec![valid.clone(); MAX_ENTRIES + 1]);
  assert!(!page.document.played);
  let url = page.url.clone();
  let oversized = json!([valid.clone(), "x".repeat(MAX_DRAIN_BYTES)]);
  let raw = snapshot(json!([url.as_str(), "", "", oversized]));
  page.apply_drain(&url, &raw);
  assert!(!page.document.played);
  let raw = snapshot(json!(["https://example.com/old", "", "", [valid]]));
  page.apply_drain(&url, &raw);
  assert!(!page.document.played);
}

#[test]
fn drain_reads_only_the_hook_serialized_string() {
  let mut page = page();
  let url = page.url.clone();
  let valid = hit(
    "playing",
    None,
    json!({"playing": true, "playDeltaSec": 1.0}),
  );
  // An engine-serialized array (the old shape), an empty result and junk are all ignored.
  for raw in [
    json!([url.as_str(), "", "", [valid.clone()]]).to_string(),
    "\"\"".to_owned(),
    String::new(),
    "{}".to_owned(),
    snapshot(json!({"not": "an array"})),
  ] {
    page.apply_drain(&url, &raw);
    assert!(!page.document.played, "{raw}");
  }
  page.apply_drain(&url, &snapshot(json!([url.as_str(), "", "", [valid]])));
  assert!(page.document.played && page.document.playing);
  drain(&mut page, vec![]);
  assert!(!page.document.playing, "a quiet drain ends playback");
}

#[test]
fn handoff_for_an_unknown_media_url_is_a_fresh_cell_not_a_panic() {
  let page = page();
  let missing = Url::parse("https://cdn.example.org/gone.mp4").unwrap();
  assert!(page.document.handoff(&missing).get().is_none());
}

#[test]
fn saved_urls_stay_capped_and_keep_the_newest() {
  let mut page = page();
  for index in 0..MAX_SAVED_URLS + 25 {
    page.remember_saved(Url::parse(&format!("https://cdn.example.org/{index}.mp4")).unwrap());
  }
  assert_eq!(page.saved_urls.len(), MAX_SAVED_URLS);
  let newest = Url::parse(&format!(
    "https://cdn.example.org/{}.mp4",
    MAX_SAVED_URLS + 24
  ))
  .unwrap();
  assert!(page.saved_urls.contains(&newest));
  page.remember_saved(newest);
  assert_eq!(
    page.saved_urls.len(),
    MAX_SAVED_URLS,
    "a repeat evicts nothing"
  );
}

#[test]
fn a_crashed_page_shows_crashed_until_it_loads_again() {
  let mut page = page();
  page.document.saving = true;
  page.document.crashed = true;
  let status = BrowserStatus::default();
  assert_eq!(
    page.changed("catch-test", status).unwrap().phase,
    Phase::Crashed
  );
  page.navigated(page.url.clone());
  assert_eq!(
    page.changed("catch-test", status).unwrap().phase,
    Phase::Looking
  );
}

#[test]
fn play_seconds_count_progress_without_counting_simultaneous_players_twice() {
  let mut page = page();
  let watching = [
    hit(
      "playing",
      Some("https://cdn.example.org/a.mp4"),
      json!({"playing": true, "playDeltaSec": 0.7}),
    ),
    hit(
      "playing",
      Some("https://cdn.example.org/b.mp4"),
      json!({"playing": true, "playDeltaSec": 0.5}),
    ),
    hit(
      "playing",
      None,
      json!({"playing": true, "playDeltaSec": 300}),
    ),
  ];
  drain(&mut page, watching.to_vec());
  assert!(page.document.played);
  assert_eq!(page.document.played_sec, 0.7);
  drain(&mut page, vec![]);
  assert_eq!(page.document.played_sec, 0.7);
}

#[test]
fn the_playing_manifest_wins_over_longer_preloaded_neighbors() {
  let clips = [
    ("83", 83.0),
    ("27", 27.7),
    ("92", 92.0),
    ("45", 45.0),
    ("43", 43.0),
  ];
  let addresses = clips.map(|(name, _)| {
    let host = if name == "83" { "mse" } else { "cdn" };
    format!("https://{host}.example.org/{name}.m3u8")
  });
  for (playing, expected) in [
    (Some(Some(addresses[1].as_str())), 27.7),
    (Some(None), 27.7),
    (None, 92.0),
  ] {
    let mut page = page();
    drain(
      &mut page,
      addresses
        .iter()
        .enumerate()
        .map(|(index, url)| hit("fetch", Some(url), json!({"loaded": index == 0})))
        .collect(),
    );
    for (url, (_, seconds)) in addresses.iter().zip(clips) {
      page.document.inspected(
        &Url::parse(url).unwrap(),
        ManifestInfo {
          duration_seconds: Some(seconds),
          ..Default::default()
        },
      );
    }
    if let Some(src) = playing {
      drain(
        &mut page,
        vec![hit(
          "playing",
          src,
          json!({
            "playing": true, "durationSec": 27.72, "mediaId": 1, "playerTop": 10
          }),
        )],
      );
      assert_eq!(page.auto_view(false).mains[0].duration, Some(expected));
    }
    let snapshot = page.snapshot("catch-test", BrowserStatus::default());
    assert_eq!(
      snapshot
        .candidates
        .iter()
        .find(|c| c.main)
        .unwrap()
        .duration_sec,
      Some(expected)
    );
  }
}

#[test]
fn the_topmost_visible_playing_video_wins() {
  let mut page = page();
  drain(
    &mut page,
    vec![
      hit(
        "playing",
        Some("https://cdn.example.org/lower.mp4"),
        json!({
          "playing": true, "durationSec": 653.8, "mediaId": 1, "playerTop": 648
        }),
      ),
      hit(
        "playing",
        Some("https://cdn.example.org/first.mp4"),
        json!({
          "playing": true, "durationSec": 5.0, "mediaId": 2, "playerTop": 100
        }),
      ),
      hit(
        "playing",
        Some("https://cdn.example.org/offscreen.mp4"),
        json!({
          "playing": true, "durationSec": 7200.0, "mediaId": 3, "playerTop": i32::MAX
        }),
      ),
    ],
  );
  assert_eq!(page.auto_view(false).mains[0].duration, Some(5.0));
  drain(
    &mut page,
    vec![hit("player.inactive", None, json!({"mediaId": 2}))],
  );
  assert_eq!(page.auto_view(false).mains[0].duration, Some(653.8));
  let after = page.document.last_drain_at.unwrap() + ACTIVE_PLAYER_TTL + Duration::from_millis(1);
  page.document.drain("[]", &page.url, after);
  assert_eq!(page.auto_view(false).mains[0].duration, Some(7200.0));
}

#[test]
fn offscreen_or_looping_playback_does_not_replace_an_unplayed_film() {
  // An offscreen player, and a short visible one (a looping hero or a promo).
  for (case, duration, top) in [("offscreen", 120.0, i32::MAX), ("looping", 20.0, 0)] {
    let mut page = page();
    drain(
      &mut page,
      vec![
        hit(
          "video.currentSrc",
          Some("https://cdn.example.org/film.mp4"),
          json!({"durationSec": 5400, "mediaId": 1}),
        ),
        hit(
          "playing",
          Some("https://cdn.example.org/clip.mp4"),
          json!({"playing": true, "durationSec": duration, "mediaId": 2, "playerTop": top}),
        ),
      ],
    );
    let snapshot = page.snapshot("catch-test", BrowserStatus::default());
    let main = snapshot.candidates.iter().find(|c| c.main).unwrap();
    assert_eq!(main.duration_sec, Some(5400.0), "{case}");
  }
}

#[test]
fn a_lone_short_player_does_not_replace_a_main_that_already_played() {
  let mut page = page();
  drain(
    &mut page,
    vec![
      hit(
        "playing",
        Some("https://cdn.example.org/film.mp4"),
        json!({"playing": true, "durationSec": 5400, "mediaId": 1, "playerTop": 100}),
      ),
      hit("player.inactive", None, json!({"mediaId": 1})),
      hit(
        "playing",
        Some("https://cdn.example.org/clip.mp4"),
        json!({"playing": true, "durationSec": 20, "mediaId": 2, "playerTop": 0}),
      ),
    ],
  );
  let mains = page.auto_view(false).mains;
  assert_eq!(mains.len(), 1);
  assert_eq!(mains[0].duration, Some(5400.0));
}

#[test]
fn blob_handoff_prefers_the_host_that_served_segments() {
  let origin = "https://origin.example.org/film.m3u8";
  let edge = "https://edge.example.net/film.m3u8";
  for (edge_duration, edge_loaded, expected) in [
    (None, false, origin),
    (None, true, edge),
    (Some(6738.0), true, edge),
    (Some(27.0), true, origin),
  ] {
    let mut page = page();
    let hits = vec![
      hit("fetch", Some(edge), json!({})),
      hit("video.src", Some(origin), json!({})),
      hit(
        "fetch",
        Some("https://edge.example.net/opaque"),
        json!({"loaded": edge_loaded, "ct": "video/MP2T"}),
      ),
    ];
    drain(&mut page, hits);
    page.document.inspected(
      &Url::parse(origin).unwrap(),
      ManifestInfo {
        duration_seconds: Some(6738.0),
        ..Default::default()
      },
    );
    if let Some(duration) = edge_duration {
      page.document.inspected(
        &Url::parse(edge).unwrap(),
        ManifestInfo {
          duration_seconds: Some(duration),
          ..Default::default()
        },
      );
    }
    drain(
      &mut page,
      vec![hit(
        "playing",
        None,
        json!({
          "playing": true, "durationSec": 6738.0, "mediaId": 1, "playerTop": 100
        }),
      )],
    );
    let view = page.auto_view(false);
    let chosen = page.document.handoff_candidate(&view.mains[0].id).unwrap();
    assert_eq!(chosen.download_url().as_str(), expected);
  }
}

#[test]
fn short_clip_progress_is_credited_only_to_the_source_that_played() {
  let mut page = page();
  let src = "https://cdn.example.org/clip.mp4";
  let other = "https://cdn.example.org/other.mp4";
  drain(
    &mut page,
    vec![hit(
      "playing",
      Some(src),
      json!({
        "playing": true, "durationSec": 10, "playDeltaSec": 1, "mediaId": 1, "playerTop": 0,
        "playedMediaSec": 9.6
      }),
    )],
  );
  assert_eq!(page.auto_view(false).mains[0].duration, Some(10.0));
  drain(
    &mut page,
    vec![
      hit("player.reset", None, json!({"mediaId": 1})),
      hit(
        "playing",
        Some(other),
        json!({
          "playing": true, "durationSec": 8, "mediaId": 1, "playerTop": 0, "playedMediaSec": 0
        }),
      ),
    ],
  );
  assert_eq!(
    page.snapshot("catch-test", BrowserStatus::default()).phase,
    Phase::AdPlaying
  );
  assert!(page.auto_view(false).mains.is_empty());
}

#[test]
fn looping_short_clip_keeps_the_existing_playback_threshold() {
  let mut page = page();
  let src = "https://cdn.example.org/loop.mp4";
  for _ in 0..8 {
    drain(
      &mut page,
      vec![hit(
        "playing",
        Some(src),
        json!({"playing": true, "durationSec": 10, "playDeltaSec": 1, "mediaId": 1, "playerTop": 0}),
      )],
    );
  }
  drain(
    &mut page,
    vec![hit("player.inactive", None, json!({"mediaId": 1}))],
  );
  assert_eq!(
    page.snapshot("catch-test", BrowserStatus::default()).phase,
    Phase::AdPlaying
  );
}

#[test]
fn a_page_with_only_a_short_clip_offers_it_once_it_has_played() {
  let clip = |url: &str, seconds: f64| {
    hit(
      "video.currentSrc",
      Some(url),
      json!({"durationSec": seconds}),
    )
  };
  let playback = |url: &str, seconds: f64| {
    hit(
      "playing",
      Some(url),
      json!({
        "playing": true, "mediaId": 1, "playedMediaSec": seconds
      }),
    )
  };
  let phase = |tab: &mut PageState| tab.replay("catch-id", BrowserStatus::default());
  let mut tab = page();
  drain(
    &mut tab,
    vec![
      clip("https://cdn.example.org/teaser.mp4", 8.0),
      clip("https://cdn.example.org/clip.mp4", 30.0),
    ],
  );
  drain(
    &mut tab,
    vec![playback("https://cdn.example.org/clip.mp4", 14.9)],
  );
  assert_eq!(phase(&mut tab).phase, Phase::AdPlaying);

  drain(
    &mut tab,
    vec![playback("https://cdn.example.org/clip.mp4", 15.0)],
  );
  let state = phase(&mut tab);
  assert_eq!(state.phase, Phase::Found);
  let main: Vec<_> = state.candidates.iter().filter(|c| c.main).collect();
  assert_eq!(main.len(), 1);
  assert_eq!(main[0].duration_sec, Some(30.0));

  // A longer video that shows up later takes over.
  drain(
    &mut tab,
    vec![
      hit("player.inactive", None, json!({"mediaId": 1})),
      clip("https://cdn.example.org/film.mp4", 1800.0),
    ],
  );
  let state = phase(&mut tab);
  assert_eq!(state.phase, Phase::Found);
  let main: Vec<_> = state.candidates.iter().filter(|c| c.main).collect();
  assert_eq!(main.len(), 1);
  assert_eq!(main[0].duration_sec, Some(1800.0));

  // A clip shorter than 15 s counts once it has played to its end.
  let mut tab = page();
  drain(
    &mut tab,
    vec![clip("https://cdn.example.org/teaser.mp4", 8.0)],
  );
  drain(
    &mut tab,
    vec![playback("https://cdn.example.org/teaser.mp4", 7.0)],
  );
  assert_eq!(phase(&mut tab).phase, Phase::AdPlaying);
  drain(
    &mut tab,
    vec![playback("https://cdn.example.org/teaser.mp4", 7.6)],
  );
  assert_eq!(phase(&mut tab).phase, Phase::Found);
}

#[test]
fn blob_playback_labels_only_an_unambiguous_manifest_in_its_frame() {
  let mut page = page();
  let asset = hit("fetch", Some("https://cdn.example.org/ad.m3u8"), json!({}));
  let blob = hit(
    "playing",
    None,
    json!({"playing": true, "durationSec": 25, "playDeltaSec": 0.5}),
  );
  drain(&mut page, vec![asset, blob.clone()]);
  let state = page.changed("catch-id", BrowserStatus::default()).unwrap();
  assert_eq!(state.phase, Phase::AdPlaying);
  assert_eq!(state.candidates[0].duration_sec, Some(25.0));

  let other = hit(
    "fetch",
    Some("https://cdn.example.org/second.m3u8"),
    json!({}),
  );
  drain(&mut page, vec![other, blob]);
  let state = page.changed("catch-id", BrowserStatus::default()).unwrap();
  assert_eq!(state.phase, Phase::AdPlaying);
  assert_eq!(state.candidates.len(), 1);
}

#[test]
fn blob_playback_after_a_preroll_finds_the_main_master_playlist() {
  use super::super::manifest::Variant;
  let ad = "https://ads.example.net/preroll.m3u8";
  let main = "https://cdn.example.org/master.m3u8";
  let master = || ManifestInfo {
    variants: vec![Variant {
      uri: "720.m3u8".into(),
      resolution: Some((1280, 720)),
      bandwidth: None,
    }],
    ..Default::default()
  };
  // Main playback reports its duration (long) or none at all.
  for played in [json!({"durationSec": 1800}), json!({})] {
    let mut page = page();
    drain(
      &mut page,
      vec![
        hit("fetch", Some(ad), json!({})),
        hit("fetch", Some(main), json!({})),
      ],
    );
    page.document.inspected(
      &Url::parse(ad).unwrap(),
      ManifestInfo {
        duration_seconds: Some(15.0),
        ..Default::default()
      },
    );
    page
      .document
      .inspected(&Url::parse(main).unwrap(), master());
    let mut blob = json!({"playing": true, "playDeltaSec": 0.5});
    blob
      .as_object_mut()
      .unwrap()
      .extend(played.as_object().unwrap().clone());
    drain(&mut page, vec![hit("playing", None, blob)]);
    let state = page.changed("catch-id", BrowserStatus::default()).unwrap();
    assert_eq!(state.phase, Phase::Found, "{played}");
    assert!(state
      .candidates
      .iter()
      .any(|c| c.main && c.height == Some(720)));
  }
}

#[test]
fn emits_only_differences_and_resets_saved_state_on_navigation() {
  let mut page = page();
  let status = BrowserStatus::default();
  assert!(page.changed("catch-id", status).is_some());
  assert!(page.changed("catch-id", status).is_none());
  page.document.saved = Some("download-1".into());
  assert_eq!(
    page.changed("catch-id", status).unwrap().phase,
    Phase::Saving
  );
  assert!(page.changed("catch-id", status).is_none());
  page.navigated(Url::parse("https://example.com/next").unwrap());
  let next = page.changed("catch-id", status).unwrap();
  assert_eq!(next.phase, Phase::Looking);
  assert_eq!(next.saved_download_id, None);
}

#[test]
fn popup_permission_is_consumed_only_by_an_eligible_request() {
  let mut page = page();
  assert!(!page.popup.request(true));
  page.popup.allow_once = true;
  assert!(!page.popup.request(false));
  assert!(page.popup.request(true));
  assert!(!page.popup.request(true));
  assert_eq!(
    page
      .changed("catch-id", BrowserStatus::default())
      .unwrap()
      .popups_blocked,
    3
  );
}

#[test]
fn a_popup_no_click_caused_is_blocked_and_counted_and_a_clicked_one_opens() {
  let blocked = |page: &PageState| {
    page
      .snapshot("catch-id", BrowserStatus::default())
      .popups_blocked
  };
  let mut page = page();
  // Windows: the engine never says who caused a pop-up, so it waits for "allow once".
  assert!(!page.popup.admit(true, false));
  assert_eq!(blocked(&page), 1);
  page.popup.allow_once = true;
  assert!(page.popup.admit(true, false));
  // A click opens it, but never an address that is not a web page.
  assert!(page.popup.admit(true, true));
  assert!(!page.popup.admit(false, true));
  assert_eq!(blocked(&page), 2);
  // The page saw three of its own pop-ups fail: the two refused above are counted already,
  // the third was stopped by the engine (no click).
  page.popup.page_count(3);
  assert_eq!(blocked(&page), 3);
  page.popup.page_count(1);
  assert_eq!(blocked(&page), 4);
  // A page cannot run the count up without limit.
  page.popup.page_count(u32::MAX);
  assert_eq!(blocked(&page), 24);
}

#[test]
fn tauri_internal_hosts_are_not_web_pages() {
  for raw in [
    "http://tauri.localhost/",
    "https://IPC.localhost/cmd",
    "http://asset.localhost./file",
  ] {
    assert!(!is_http(&Url::parse(raw).unwrap()), "{raw}");
  }
  assert!(is_http(&Url::parse("https://example.com/").unwrap()));
}

#[test]
fn the_window_state_carries_the_tab_list_and_the_active_tabs_fields() {
  let mut active = page();
  drain(
    &mut active,
    vec![hit(
      "video.src",
      Some("https://cdn.example.com/a.mp4"),
      json!({ "durationSec": 600.0 }),
    )],
  );
  let page_state = active.snapshot("catch-id", BrowserStatus::default());
  let place = |id, asleep| TabPlace {
    id,
    private: false,
    opener: None,
    asleep,
    mobile: false,
  };
  let tabs = vec![
    CatchTab::live(place("t1", false), &page_state),
    CatchTab::idle(
      TabPlace {
        opener: Some("t1"),
        private: true,
        ..place("t2", true)
      },
      "https://example.org/x",
      Some("Kept"),
    ),
    CatchTab::idle(place("t3", false), "", None),
  ];
  let value = serde_json::to_value(page_state.with_tabs(tabs, "t1")).unwrap();
  assert_eq!(value["activeTabId"], "t1");
  assert_eq!(value["url"], "https://example.com/watch");
  assert_eq!(value["phase"], "found");
  assert_eq!(value["tabs"][0]["found"], true);
  assert_eq!(value["tabs"][1]["asleep"], true);
  assert_eq!(value["tabs"][1]["openerId"], "t1");
  assert_eq!(value["tabs"][1]["title"], "Kept");
  assert_eq!(value["tabs"][1]["private"], true);
  assert_eq!(value["tabs"][2]["url"], "");
  assert_eq!(value["tabs"][2]["found"], false);
}

#[test]
fn the_encrypted_verdict_belongs_to_the_main_video_not_to_the_page() {
  let main = Some("https://cdn.example.org/film.mp4");
  let encrypted = |id: u64, seconds: f64, playing: bool| {
    let mut hits = vec![hit(
      "encrypted",
      None,
      json!({"emeActive": true, "durationSec": seconds, "mediaId": id}),
    )];
    if playing {
      hits.push(hit(
        "playing",
        None,
        json!({"playing": true, "durationSec": seconds, "mediaId": id}),
      ));
    }
    hits
  };
  let clear_main = |playing: bool| {
    hit(
      if playing {
        "playing"
      } else {
        "video.currentSrc"
      },
      main,
      json!({"playing": playing, "durationSec": 5400, "mediaId": 9}),
    )
  };
  let cases = [
    (
      "encrypted ad playing beside a clear main",
      [encrypted(1, 20.0, true), vec![clear_main(false)]].concat(),
      false,
    ),
    (
      "encrypted preview, clear main playing",
      [encrypted(1, 120.0, true), vec![clear_main(true)]].concat(),
      false,
    ),
    (
      "the long encrypted video is the one playing",
      [encrypted(1, 3600.0, true), vec![clear_main(false)]].concat(),
      true,
    ),
    (
      "encrypted main, nothing clear",
      encrypted(1, 3600.0, true),
      true,
    ),
    (
      "an encrypted ad still loading says nothing yet",
      vec![hit(
        "encrypted",
        None,
        json!({"emeActive": true, "mediaId": 1}),
      )],
      false,
    ),
    (
      "a short encrypted ad that just started",
      encrypted(1, 20.0, true),
      false,
    ),
  ];
  for (name, hits, drm) in cases {
    let mut page = page();
    drain(&mut page, hits);
    assert_eq!(page.document.has_drm(), drm, "{name}");
    let auto = page.auto_view(false);
    assert_eq!(auto.phase == Phase::Drm, drm, "{name}");
    if name == "encrypted preview, clear main playing" {
      assert_eq!(auto.mains.len(), 1);
    }
    exhaust_retries(&mut page);
    let phase = page.replay("catch-id", BrowserStatus::default()).phase;
    assert_eq!(phase == Phase::Drm, drm, "{name}: {phase:?}");
  }
}

fn encrypted_main(page: &mut PageState) {
  let frame = page.url.to_string();
  drain(
    page,
    vec![
      hit(
        "encrypted",
        None,
        json!({"emeActive": true, "durationSec": 3600, "mediaId": 1, "frame": frame}),
      ),
      hit(
        "playing",
        None,
        json!({"playing": true, "durationSec": 3600, "mediaId": 1, "frame": frame}),
      ),
    ],
  );
}

#[test]
fn a_clear_promo_after_a_player_reset_keeps_the_encrypted_film_verdict() {
  let mut page = page();
  encrypted_main(&mut page);
  assert_eq!(page.auto_view(false).phase, Phase::Drm);
  drain(
    &mut page,
    vec![hit("player.reset", None, json!({"mediaId": 1}))],
  );
  drain(
    &mut page,
    vec![hit(
      "playing",
      Some("https://cdn.example.org/promo.mp4"),
      json!({
        "playing": true, "durationSec": 10, "mediaId": 1, "playerTop": 0, "playedMediaSec": 9.6
      }),
    )],
  );
  assert_eq!(page.auto_view(false).phase, Phase::Drm);
  exhaust_retries(&mut page);
  assert_eq!(
    page.replay("catch-id", BrowserStatus::default()).phase,
    Phase::Drm
  );
}

/// The reload a retry asks for: same address, fresh document.
fn reload(page: &mut PageState) {
  page.navigated(page.url.clone());
}

#[test]
fn an_encrypted_main_shows_only_after_each_retry_ran_once() {
  let t0 = Instant::now();
  let at = |seconds: u64| t0 + Duration::from_secs(seconds);
  let shown = |page: &mut PageState| page.replay("catch-id", BrowserStatus::default()).phase;
  let mut page = page();
  encrypted_main(&mut page);
  assert_eq!(page.next_retry(false, at(0)), Some(Retry::Mobile));
  // Meanwhile the tab says it is still looking, and nothing is counted.
  assert_eq!(shown(&mut page), Phase::Looking);
  assert!(!page.drm_final(at(0)));
  // The reload has not replaced the page yet.
  assert_eq!(page.next_retry(true, at(1)), None);
  reload(&mut page);
  encrypted_main(&mut page);
  // The Mobile version is encrypted too: the tab's own identity comes back.
  assert_eq!(page.next_retry(true, at(2)), Some(Retry::Restore));
  assert_eq!(page.next_retry(false, at(3)), None);
  reload(&mut page);
  encrypted_main(&mut page);
  assert_eq!(page.next_retry(false, at(4)), Some(Retry::Ytdlp));
  assert_eq!(page.next_retry(false, at(5)), None);
  assert!(!page.drm_final(at(5)));
  page.ytdlp_done();
  // Each step ran once: nothing more is asked, and the verdict shows and counts.
  for second in [6, 60, 600] {
    assert_eq!(page.next_retry(false, at(second)), None);
  }
  assert!(page.drm_final(at(6)));
  assert_eq!(shown(&mut page), Phase::Drm);

  // A tab already showing the Mobile version goes straight to yt-dlp.
  let mut page = self::page();
  encrypted_main(&mut page);
  assert_eq!(page.next_retry(true, at(0)), Some(Retry::Ytdlp));

  // The Mobile version plays a clear video: it stays, and nothing else runs.
  let mut page = self::page();
  encrypted_main(&mut page);
  assert_eq!(page.next_retry(false, at(0)), Some(Retry::Mobile));
  reload(&mut page);
  drain(
    &mut page,
    vec![hit(
      "playing",
      Some("https://m.example.com/film.mp4"),
      json!({"playing": true, "durationSec": 3600, "mediaId": 1}),
    )],
  );
  assert_eq!(page.next_retry(true, at(1)), None);
  assert_eq!(page.next_retry(true, at(20)), None);
  assert_eq!(page.ladder.step, Step::Done);
  assert_eq!(shown(&mut page), Phase::Found);

  // A page that never answers: the cap ends the retries, and the identity still comes back.
  let mut page = self::page();
  encrypted_main(&mut page);
  assert_eq!(page.next_retry(false, at(0)), Some(Retry::Mobile));
  let late = t0 + LADDER_CAP;
  assert_eq!(page.next_retry(true, late), Some(Retry::Restore));
  assert!(page.drm_final(late));
  assert_eq!(page.next_retry(false, late), None);

  // Another page starts its own ladder; a clear page starts none.
  let mut page = self::page();
  assert_eq!(page.next_retry(false, at(0)), None);
  encrypted_main(&mut page);
  assert_eq!(page.next_retry(false, at(0)), Some(Retry::Mobile));
  page.navigated(Url::parse("https://example.com/other").unwrap());
  encrypted_main(&mut page);
  assert_eq!(page.next_retry(false, at(1)), Some(Retry::Mobile));
}

#[test]
fn a_page_yt_dlp_reads_is_offered_whatever_the_page_plays() {
  use super::super::handoff::{CapturedRequest, Handoff, RequestContext};
  let mut page = page();
  encrypted_main(&mut page);
  let candidate = Candidate::page(page.url.clone());
  let handoff = Handoff::new(CapturedRequest::new(
    &candidate,
    RequestContext {
      frame_url: page.url.to_string(),
      frame_origin: "https://example.com".into(),
      user_agent: "Test browser".into(),
      cookies: Vec::new(),
    },
  ));
  page.document.rescue(candidate, handoff, Vec::new());
  exhaust_retries(&mut page);
  let state = page.replay("catch-id", BrowserStatus::default());
  assert_eq!(state.phase, Phase::Found);
  assert_eq!(state.candidates.len(), 1);
  assert!(!page.drm_final(Instant::now()));
  assert!(page
    .document
    .handoff_candidate(&state.candidates[0].id)
    .is_some());
}

#[test]
fn a_trailing_site_name_leaves_the_title() {
  let cases = [
    (
      "https://movies4u.example/film/1",
      "The Film (2024) | Movies4U",
      "The Film (2024)",
    ),
    (
      "https://www.youtube.com/watch?v=x",
      "Ocean waves - YouTube",
      "Ocean waves",
    ),
    ("https://www.movies4u.com/f", "Film – movies4u.com", "Film"),
    ("https://movies4u.com/f", "Film — Movies 4U", "Film"),
    ("https://watch.site.co.uk/a", "Clip · SITE", "Clip"),
    ("https://movies4u.com/f", "Film|Movies4U", "Film"),
    // A near form of the domain's name: a short ending added or dropped.
    (
      "https://movies4us.net/watch/movie-the-intouchables-nw1ii",
      "Watch The Intouchables (2011) Online Free | Movies4U",
      "Watch The Intouchables (2011) Online Free",
    ),
    ("https://movies4u.top/f", "Film - Movies4Us", "Film"),
    // Too short, or too far from the name: kept.
    (
      "https://movies4us.net/f",
      "The Intouchables | Part 2",
      "The Intouchables | Part 2",
    ),
    (
      "https://movies4us.net/f",
      "Trailer | Movies",
      "Trailer | Movies",
    ),
    ("https://tvshows.com/f", "Clip | TV", "Clip | TV"),
    ("http://10.0.0.1/f", "Clip | 10001", "Clip | 10001"),
    // Not the site: kept whole.
    ("https://movies4u.com/f", "Film - Part 2", "Film - Part 2"),
    (
      "https://example.com/f",
      "Spider-Man - Example",
      "Spider-Man",
    ),
    ("https://example.com/f", "Spider-Example", "Spider-Example"),
    // A bare IP address has no site name ("216" would pass for one).
    ("http://93.184.216.34/watch", "Flight - 216", "Flight - 216"),
    ("https://lyon.fr/t", "Train Paris–Lyon", "Train Paris–Lyon"),
    ("https://example.com/f", "Spider–Example", "Spider–Example"),
    (
      "https://example.com/f",
      "Who is — \"Example\"?",
      "Who is — \"Example\"?",
    ),
    // Only the site name, or the name first: unchanged.
    ("https://example.com/", "Example", "Example"),
    ("https://example.com/", " | Example", " | Example"),
    (
      "https://example.com/",
      "Example | Home of films",
      "Example | Home of films",
    ),
  ];
  for (page, title, expected) in cases {
    let page = Url::parse(page).unwrap();
    assert_eq!(without_site_suffix(title, &page), expected, "{title}");
  }
  let mut page = PageState::new(Url::parse("https://www.youtube.com/watch").unwrap());
  let url = page.url.clone();
  page.apply_drain(
    &url,
    &snapshot(json!([url.as_str(), "Ocean waves - YouTube", "Agent", []])),
  );
  assert_eq!(page.title.as_deref(), Some("Ocean waves"));
}

#[test]
fn the_thumbnail_tries_the_poster_then_the_page_picture_once_a_main_video_shows() {
  let mut page = page();
  let url = page.url.clone();
  let pictures = |poster: &str, image: &str| {
    snapshot(json!([url.as_str(), "Video", "Agent", [], [poster, image]]))
  };
  page.apply_drain(
    &url,
    &pictures(
      "https://cdn.example.com/poster.jpg",
      "https://example.com/og.jpg",
    ),
  );
  assert!(page.thumbnail_job().is_none(), "no main video yet");
  drain(
    &mut page,
    vec![hit(
      "playing",
      Some("https://cdn.example.org/video.mp4"),
      json!({"playing": true, "durationSec": 600.0}),
    )],
  );
  // A drain without pictures (the old hook) keeps none.
  assert!(page.document.pictures == (None, None));
  page.apply_drain(
    &url,
    &pictures("javascript:alert(1)", "https://example.com/og.jpg"),
  );
  page.apply_drain(
    &url,
    &pictures(
      "https://cdn.example.com/poster.jpg",
      "https://example.com/og.jpg",
    ),
  );
  let job = page.thumbnail_job().expect("a main video");
  assert_eq!(
    job.pictures,
    [
      Url::parse("https://cdn.example.com/poster.jpg").unwrap(),
      Url::parse("https://example.com/og.jpg").unwrap()
    ]
  );
  let (media, hls, _) = job.media.expect("a frame source");
  assert_eq!(media.as_str(), "https://cdn.example.org/video.mp4");
  assert!(!hls);
  assert!(page.thumbnail_job().is_none(), "once per page");
  page.document.thumbnail = Some("data:image/jpeg;base64,AA==".into());
  let state = page.snapshot("w", BrowserStatus::default());
  assert_eq!(
    state.candidates[0].thumbnail.as_deref(),
    Some("data:image/jpeg;base64,AA==")
  );
  page.navigated(Url::parse("https://example.com/next").unwrap());
  assert!(page.document.thumbnail.is_none());

  // A video in a sandboxed frame (opaque origin): the page stands in as Referer.
  let mut page = self::page();
  drain(
    &mut page,
    vec![hit(
      "playing",
      Some("https://cdn.example.org/sandboxed.mp4"),
      json!({"frame": "https://sandbox.example.net/embed", "isTop": false, "msgOrigin": "null", "playing": true, "durationSec": 600.0}),
    )],
  );
  let (_, _, referer) = page.thumbnail_job().unwrap().media.unwrap();
  assert_eq!(referer, page.url.as_str());
}

/// What a YouTube page drains while its player plays: a MediaSource (blob) element and the
/// player's own fetches of googlevideo pieces, none of them a stream a download can reuse.
fn youtube_playback(page: &mut PageState, title: &str) {
  let frame = page.url.to_string();
  let on_page = |method: &str, url: Option<&str>, extra: Value| {
    let mut value = hit(method, url, extra);
    value["frame"] = json!(frame);
    value
  };
  let hits = vec![
    on_page(
      "fetch",
      Some(
        "https://rr3---sn-4g5e6nzz.googlevideo.com/videoplayback?expire=1&itag=243&range=0-9999",
      ),
      json!({"ct": "application/vnd.yt-ump"}),
    ),
    on_page(
      "xhr",
      Some("https://rr3---sn-4g5e6nzz.googlevideo.com/videoplayback?expire=1&itag=251&range=0-999"),
      json!({"ct": "audio/webm"}),
    ),
    on_page(
      "playing",
      None,
      json!({"playing": true, "durationSec": 31.0, "playDeltaSec": 0.5, "mediaId": 1}),
    ),
  ];
  let url = page.url.clone();
  let data = snapshot(json!([url.as_str(), title, "Test browser", hits]));
  page.apply_drain(&url, &data);
}

fn visible(url: &str) -> PageState {
  let mut page = PageState::new(Url::parse(url).unwrap());
  page.offers_page = true;
  page.offer_page();
  page
}

#[test]
fn a_youtube_short_opened_in_page_is_offered_at_once_by_its_address() {
  let status = BrowserStatus::default();
  let mut page = visible("https://www.youtube.com/@SomeChannel");
  youtube_playback(&mut page, "SomeChannel - YouTube");
  let state = page.replay("catch-id", status);
  assert_eq!(
    state.phase,
    Phase::Looking,
    "the player's pieces are no video"
  );
  assert!(state.candidates.is_empty());

  // The channel opens a Short by history.pushState: the drain loop sees the new address.
  let short = Url::parse("https://www.youtube.com/shorts/ynQcf3PSsyI").unwrap();
  page.navigated(short.clone());
  let state = page.replay("catch-id", status);
  assert_eq!(state.phase, Phase::Found, "before anything plays");
  assert_eq!(state.url, short.as_str());
  let [offer] = &state.candidates[..] else {
    panic!("one offer: {:?}", state.candidates.len());
  };
  assert!(offer.main);
  let id = offer.id.clone();
  let candidate = page.document.handoff_candidate(&id).expect("savable");
  assert_eq!(candidate.url(), &short);
  assert_eq!(candidate.download_url(), &short);

  youtube_playback(&mut page, "Funny cat #shorts - YouTube");
  let state = page.replay("catch-id", status);
  assert_eq!(
    state.phase,
    Phase::Found,
    "a short clip is still the page's video"
  );
  assert_eq!(state.candidates.len(), 1);
  assert_eq!(state.candidates[0].id, id);
  assert_eq!(
    state.candidates[0].title.as_deref(),
    Some("Funny cat #shorts")
  );

  let job = page.thumbnail_job().expect("the offer has a picture");
  assert_eq!(
    job.pictures[0].as_str(),
    "https://i.ytimg.com/vi/ynQcf3PSsyI/hqdefault.jpg"
  );
  assert!(job.media.is_none(), "a page address is no frame source");

  // Scrolling to the next Short changes the address again: a new offer for it.
  let next = Url::parse("https://www.youtube.com/shorts/AbCdEfGhIjK").unwrap();
  page.navigated(next.clone());
  let state = page.replay("catch-id", status);
  assert_eq!(state.phase, Phase::Found);
  assert_ne!(state.candidates[0].id, id);
  let candidate = page
    .document
    .handoff_candidate(&state.candidates[0].id)
    .unwrap();
  assert_eq!(candidate.url(), &next);
}

#[test]
fn a_video_page_offers_only_its_address_and_automatic_catch_never_does() {
  let mut page = visible("https://www.youtube.com/watch?v=dQw4w9WgXcQ");
  let frame = page.url.to_string();
  let mut stream = hit(
    "playing",
    Some("https://cdn.example.org/video.mp4"),
    json!({"playing": true, "durationSec": 600.0}),
  );
  stream["frame"] = json!(frame);
  let url = page.url.clone();
  page.apply_drain(
    &url,
    &snapshot(json!([url.as_str(), "Video", "Agent", [stream]])),
  );
  let state = page.replay("catch-id", BrowserStatus::default());
  assert_eq!(state.candidates.len(), 1);
  assert_eq!(
    page
      .document
      .handoff_candidate(&state.candidates[0].id)
      .unwrap()
      .url(),
    &url
  );

  // A hidden page (automatic catch) reads the link the normal path already failed on.
  let mut hidden = PageState::new(Url::parse("https://www.youtube.com/@SomeChannel").unwrap());
  hidden.navigated(Url::parse("https://www.youtube.com/shorts/ynQcf3PSsyI").unwrap());
  let state = hidden.replay("catch-id", BrowserStatus::default());
  assert_eq!(state.phase, Phase::Looking);
  assert!(state.candidates.is_empty());
}

#[test]
fn the_ladders_read_of_an_offered_page_stays_offered() {
  use super::super::handoff::{CapturedRequest, Handoff, RequestContext};
  let mut page = visible("https://www.youtube.com/watch?v=dQw4w9WgXcQ");
  let candidate = Candidate::page(page.url.clone());
  let handoff = Handoff::new(CapturedRequest::new(
    &candidate,
    RequestContext {
      frame_url: page.url.to_string(),
      frame_origin: "https://www.youtube.com".into(),
      user_agent: "Test browser".into(),
      cookies: Vec::new(),
    },
  ));
  page.document.rescue(candidate, handoff, Vec::new());
  let state = page.replay("catch-id", BrowserStatus::default());
  assert_eq!(state.phase, Phase::Found);
  assert_eq!(state.candidates.len(), 1);
}

#[test]
fn a_snapshot_of_another_address_never_lands_on_the_offered_page() {
  let status = BrowserStatus::default();
  let mut page = visible("https://www.youtube.com/shorts/ynQcf3PSsyI");
  youtube_playback(&mut page, "First Short - YouTube");
  let offered = page.replay("catch-id", status).candidates[0].id.clone();
  let caught = page.document.candidates.iter().count();

  // The next Short opened in page while the tab still loads: the drain loop has not moved
  // this page to the new address yet, and the engine already reports it.
  let next = Url::parse("https://www.youtube.com/shorts/AbCdEfGhIjK").unwrap();
  let mut stream = hit(
    "playing",
    Some("https://cdn.example.org/next.mp4"),
    json!({"playing": true, "durationSec": 600.0}),
  );
  stream["frame"] = json!(next.as_str());
  page.apply_drain(
    &next,
    &snapshot(json!([
      next.as_str(),
      "Second Short - YouTube",
      "Test browser",
      [stream],
      ["", "https://i.example.org/second.jpg"]
    ])),
  );
  let state = page.replay("catch-id", status);
  assert_eq!(state.candidates[0].id, offered);
  assert_eq!(state.candidates[0].title.as_deref(), Some("First Short"));
  assert_eq!(page.document.pictures, (None, None));
  assert_eq!(page.document.candidates.iter().count(), caught);
}

#[test]
fn a_live_or_encrypted_video_page_offers_nothing_it_cannot_save() {
  let status = BrowserStatus::default();
  // A livestream: the player's live playlist says so, whatever the address offers.
  let mut page = visible("https://www.youtube.com/watch?v=dQw4w9WgXcQ");
  let hls = "https://manifest.googlevideo.com/api/manifest/hls_variant/live.m3u8";
  let mut playlist = hit("fetch", Some(hls), json!({}));
  playlist["frame"] = json!(page.url.as_str());
  let url = page.url.clone();
  page.apply_drain(
    &url,
    &snapshot(json!([url.as_str(), "Live", "Test browser", [playlist]])),
  );
  page.document.inspected(
    &Url::parse(hls).unwrap(),
    ManifestInfo {
      is_live: true,
      ..Default::default()
    },
  );
  let state = page.replay("catch-id", status);
  assert_eq!(state.phase, Phase::Live);
  assert!(state.candidates.is_empty());

  // An encrypted film while the retries still run: no Save that the hand-off would refuse.
  let mut page = visible("https://www.youtube.com/watch?v=dQw4w9WgXcQ");
  encrypted_main(&mut page);
  assert!(page.document.has_drm());
  let state = page.replay("catch-id", status);
  assert_eq!(state.phase, Phase::Looking);
  assert!(state.candidates.is_empty(), "nothing the hand-off refuses");
  exhaust_retries(&mut page);
  assert_eq!(page.replay("catch-id", status).phase, Phase::Drm);
}

#[test]
fn an_offered_page_is_one_video_whatever_its_fragment() {
  let status = BrowserStatus::default();
  let plain = Url::parse("https://www.youtube.com/watch?v=dQw4w9WgXcQ").unwrap();
  let mut page = visible("https://www.youtube.com/watch?v=dQw4w9WgXcQ#t=30");
  page.remember_saved(plain.clone());
  let state = page.replay("catch-id", status);
  assert!(state.candidates[0].already_saved);
  let id = state.candidates[0].id.clone();
  assert_eq!(
    page.document.handoff_candidate(&id).unwrap().url(),
    &plain,
    "the fragment is not part of what is saved"
  );

  // A fragment-only change (a chapter, the comments) keeps the offer and its picture; the
  // address shown follows. Another video is another page.
  let comments = Url::parse("https://www.youtube.com/watch?v=dQw4w9WgXcQ#comments").unwrap();
  assert!(!page.moved_to(&comments));
  let state = page.replay("catch-id", status);
  assert_eq!(state.url, comments.as_str());
  assert_eq!(state.candidates[0].id, id);
  assert!(
    page.moved_to(&Url::parse("https://www.youtube.com/watch?v=AbCdEfGhIjK#comments").unwrap())
  );
  // A page with nothing offered by its address keeps the old rule: any change is a new page.
  let mut app = PageState::new(Url::parse("https://app.example.org/#/video/1").unwrap());
  assert!(app.moved_to(&Url::parse("https://app.example.org/#/video/2").unwrap()));
}

#[test]
fn leaving_a_video_page_in_page_clears_its_offer() {
  let status = BrowserStatus::default();
  let mut page = visible("https://www.youtube.com/shorts/ynQcf3PSsyI");
  youtube_playback(&mut page, "Funny cat #shorts - YouTube");
  assert_eq!(page.replay("catch-id", status).phase, Phase::Found);
  for away in [
    "https://www.youtube.com/@SomeChannel",
    "https://www.youtube.com/results?search_query=cats",
    "https://www.youtube.com/",
  ] {
    page.navigated(Url::parse(away).unwrap());
    let state = page.replay("catch-id", status);
    assert_eq!(state.phase, Phase::Looking, "{away}");
    assert!(state.candidates.is_empty(), "{away}");
    assert!(page.thumbnail_job().is_none(), "{away}");
    page.navigated(Url::parse("https://www.youtube.com/shorts/ynQcf3PSsyI").unwrap());
    assert_eq!(page.replay("catch-id", status).phase, Phase::Found);
  }
}

#[test]
fn a_geo_block_page_shows_while_the_page_is_one_and_anything_that_plays_wins() {
  let mut page = page();
  let phase = |tab: &mut PageState| tab.replay("catch-id", BrowserStatus::default());
  let url = page.url.clone();
  let geo = |status: u16, text: &str| {
    snapshot(json!([
      url.as_str(),
      "Sorry",
      "Agent",
      [],
      ["", ""],
      [status, text]
    ]))
  };
  let sorry = "This video is not available in your country";
  page.apply_drain(&url, &geo(200, "Watch the trailer"));
  assert_eq!(phase(&mut page).phase, Phase::Looking);
  page.apply_drain(&url, &geo(0, sorry));
  assert_eq!(phase(&mut page).phase, Phase::GeoBlocked);
  // The page filled in (it is no longer short, so no text comes): not a block page after all.
  page.apply_drain(&url, &geo(0, ""));
  assert_eq!(phase(&mut page).phase, Phase::Looking);
  page.apply_drain(&url, &geo(0, sorry));
  assert_eq!(phase(&mut page).phase, Phase::GeoBlocked);
  // An ad plays on it: the ad shows, not the block.
  drain(
    &mut page,
    vec![hit(
      "playing",
      Some("https://ads.example.net/spot.mp4"),
      json!({"playing": true, "durationSec": 12.0}),
    )],
  );
  page.apply_drain(&url, &geo(0, sorry));
  assert_eq!(phase(&mut page).phase, Phase::AdPlaying);
  drain(
    &mut page,
    vec![hit(
      "playing",
      Some("https://cdn.example.org/video.mp4"),
      json!({"playing": true, "durationSec": 600.0}),
    )],
  );
  page.apply_drain(&url, &geo(0, sorry));
  assert_eq!(phase(&mut page).phase, Phase::Found);
  page.navigated(Url::parse("https://example.com/other").unwrap());
  assert_eq!(phase(&mut page).phase, Phase::Looking);
  // HTTP 451 alone, for as long as the document is the one that answered it.
  let url = page.url.clone();
  let legal = snapshot(json!([url.as_str(), "", "Agent", [], ["", ""], [451, ""]]));
  page.apply_drain(&url, &legal);
  page.apply_drain(&url, &legal);
  assert_eq!(phase(&mut page).phase, Phase::GeoBlocked);
}

#[test]
fn a_live_stream_on_a_page_with_block_words_is_live() {
  for (live, expected) in [(true, Phase::Live), (false, Phase::GeoBlocked)] {
    assert_eq!(phase(false, false, false, live, true, &[], 0), expected);
  }
}

#[test]
fn the_snapshot_says_whether_the_top_page_shows_a_password_field() {
  let mut page = PageState::new(Url::parse("https://www.facebook.com/").unwrap());
  let url = page.url.clone();
  let with = |flag: Value| {
    snapshot(json!([
      url.as_str(),
      "Facebook",
      "Agent",
      [],
      ["", ""],
      [0, ""],
      flag
    ]))
  };
  page.apply_drain(&url, &with(json!(true)));
  assert!(page.password_visible);
  page.apply_drain(&url, &with(json!(false)));
  assert!(!page.password_visible);
  // Only a real boolean counts; an older hook sends none.
  page.apply_drain(&url, &with(json!("true")));
  assert!(!page.password_visible);
  page.apply_drain(&url, &with(json!(true)));
  page.apply_drain(
    &url,
    &snapshot(json!([url.as_str(), "Facebook", "Agent", []])),
  );
  assert!(!page.password_visible);
  // A new page starts without one.
  page.apply_drain(&url, &with(json!(true)));
  page.navigated(Url::parse("https://www.facebook.com/watch").unwrap());
  assert!(!page.password_visible);
}

#[test]
fn a_redirect_still_loading_never_puts_the_next_pages_password_field_on_this_page() {
  // The engine already reports site B while the page state is still site A's.
  let mut page = PageState::new(Url::parse("https://a.example.com/watch").unwrap());
  let next = Url::parse("https://b.example.org/login").unwrap();
  let data = snapshot(json!([
    next.as_str(),
    "Sign in",
    "Agent",
    [],
    ["", ""],
    [0, ""],
    true
  ]));
  page.apply_drain(&next, &data);
  assert!(!page.password_visible);
}
