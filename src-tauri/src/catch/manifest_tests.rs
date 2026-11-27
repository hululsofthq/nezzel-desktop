use super::*;

#[test]
fn inspects_hls_and_dash_playback_properties() {
  let cases = [
    ("master", include_str!("fixtures/master.m3u8"), false, false, None, false),
    ("AES-128 identity", include_str!("fixtures/short-aes128.m3u8"), false, false, Some(50.25), true),
    ("AES-128 default key format", "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"https://example.org/key\"\n#EXTINF:60,\na.ts\n#EXT-X-ENDLIST\n", false, false, Some(60.0), false),
    ("SAMPLE-AES with a published key", "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"key\"\n#EXTINF:12,\na.ts\n", false, true, None, false),
    ("FairPlay", "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"skd://key\",KEYFORMAT=\"com.apple.streamingkeydelivery\"\n#EXTINF:12,\na.ts\n", true, true, None, false),
    ("FairPlay key URI without a key format", "#EXTM3U\n#EXT-X-KEY:METHOD=SaMpLe-AeS,URI=\"SKD://key\"\n#EXTINF:12,\na.ts\n", true, true, None, false),
    ("Widevine CENC", "#EXTM3U\n#EXT-X-KEY:METHOD=ISO-23001-7,URI=\"data:text/plain;base64,AA==\",KEYFORMAT=\"urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed\"\n#EXTINF:12,\na.ts\n#EXT-X-ENDLIST\n", true, false, Some(12.0), true),
    ("PlayReady session key", "#EXTM3U\n#EXT-X-SESSION-KEY:METHOD=SAMPLE-AES-CTR,URI=\"data:,x\",KEYFORMAT=\"com.microsoft.playready\"\n#EXT-X-STREAM-INF:BANDWIDTH=100\nv.m3u8\n", true, false, None, false),
    ("METHOD=NONE ignores a key format", "#EXTM3U\n#EXT-X-KEY:METHOD=NONE,KEYFORMAT=\"com.apple.streamingkeydelivery\"\n#EXTINF:12,\na.ts\n#EXT-X-ENDLIST\n", false, false, Some(12.0), true),
    ("mixed-case identity", "#EXTM3U\n#EXT-X-KEY:METHOD=aes-128,URI=\"key\",KEYFORMAT=\"IdEnTiTy\"\n#EXTINF:12,\na.ts\n#EXT-X-ENDLIST\n", false, false, Some(12.0), true),
    ("SAMPLE-AES-CTR identity session key", "#EXTM3U\n#EXT-X-SESSION-KEY:URI=\"key\",METHOD=SAMPLE-AES-CTR,KEYFORMAT=\"identity\"\n#EXT-X-STREAM-INF:BANDWIDTH=100\nv.m3u8\n", false, false, None, false),
    ("nonidentity after quoted comma", "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key?x=a,b\",KEYFORMAT=\"urn:example:drm\"\n#EXTINF:12,\na.ts\n#EXT-X-ENDLIST\n", true, false, Some(12.0), true),
    ("live LL-HLS", "\u{feff}#EXTM3U\r\n#EXT-X-TARGETDURATION:4\r\n#EXT-X-PART:DURATION=0.5,URI=\"p.m4s\"\r\n", false, true, None, false),
    ("VOD with a nonzero media sequence", "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:10,\na.ts\n#EXT-X-ENDLIST\n", false, false, Some(10.0), true),
    ("endlist with malformed duration", "#EXTM3U\n#EXTINF:NaN,\na.ts\n#EXT-X-ENDLIST\n", false, false, None, false),
    ("clear DASH with a cenc declaration", include_str!("fixtures/static.mpd"), false, false, Some(60.25), false),
    ("dynamic protected DASH", include_str!("fixtures/protected.mpd"), true, true, None, false),
    ("PlayReady scheme in upper case", "<MPD mediaPresentationDuration='PT36S'><Period><ContentProtection schemeIdUri='URN:UUID:9A04F079-9840-4286-AB92-E65BE0885F95'/></Period></MPD>", true, false, Some(36.0), true),
    ("bare ContentProtection", "<MPD mediaPresentationDuration='PT36S'><Period><ContentProtection/></Period></MPD>", false, false, Some(36.0), true),
    ("common encryption without a DRM system", "<MPD xmlns:cenc='urn:mpeg:cenc:2013'><Period><ContentProtection schemeIdUri='urn:mpeg:dash:mp4protection:2011' cenc:default_KID='fake'/></Period></MPD>", false, false, None, false),
    ("ClearKey", "<MPD><Period><ContentProtection schemeIdUri='urn:uuid:e2719d58-a985-b3c9-781a-b030af78d30e'/><ContentProtection schemeIdUri='urn:uuid:1077efec-c0b2-4d02-ace3-3c1e52e2fb4b'/></Period></MPD>", false, false, None, false),
    ("cenc alias element", "<MPD xmlns:x='urn:mpeg:cenc:2013'><Period><x:pssh>AA==</x:pssh></Period></MPD>", false, false, None, false),
    ("DRM scheme on a foreign element", "<MPD><Period><Label schemeIdUri='urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed'/></Period></MPD>", false, false, None, false),
    ("comment and text are not DRM signals", "<MPD mediaPresentationDuration='P1DT2H3M4.5S'><!-- <ContentProtection/> --><Period>cenc:test</Period></MPD>", false, false, Some(93784.5), false),
    ("final period declares its span", "<MPD><Period start='PT2S'/><Period start='PT10S' duration='PT42S'/></MPD>", false, false, Some(52.0), true),
    ("contiguous periods", "<MPD><Period duration='PT10S'/><Period duration='PT25S'/></MPD>", false, false, Some(35.0), true),
    ("last period missing duration", "<MPD><Period duration='PT10S'/><Period start='PT10S'/></MPD>", false, false, None, false),
    ("dynamic with time window", "<MPD type='dynamic' mediaPresentationDuration='PT30S' timeShiftBufferDepth='PT18S'/>", false, true, None, false),
    ("invalid ISO duration", "<MPD mediaPresentationDuration='PT1M2H'/>", false, false, None, false),
  ];
  for (name, markup, drm, is_live, duration, other) in cases {
    let details = inspect(markup).unwrap_or_else(|| panic!("{name}"));
    assert_eq!(
      (
        details.has_drm,
        details.is_live,
        details.duration_seconds,
        details.is_other_video()
      ),
      (drm, is_live, duration, other),
      "{name}"
    );
  }
}

#[test]
fn reads_master_variants_with_url_resolution_and_bandwidth_metadata() {
  let info = inspect(include_str!("fixtures/master.m3u8")).unwrap();
  assert_eq!(
    info.variants,
    [
      Variant {
        uri: "360/playlist.m3u8?token=fake#player".into(),
        resolution: Some((640, 360)),
        bandwidth: Some(800_000)
      },
      Variant {
        uri: "https://example.org/hls/720.m3u8".into(),
        resolution: Some((1280, 720)),
        bandwidth: Some(2_400_000)
      },
    ]
  );

  let info = inspect("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=42\n# comment\naudio.m3u8\n").unwrap();
  assert_eq!(
    info.variants,
    [Variant {
      uri: "audio.m3u8".into(),
      resolution: None,
      bandwidth: Some(42)
    }]
  );
}

#[test]
fn limits_combined_variant_types_and_reads_drm_after_the_limit() {
  use std::fmt::Write;

  let mut text = String::from("#EXTM3U\n");
  for i in 0..32 {
    writeln!(text, "#EXT-X-STREAM-INF:BANDWIDTH=42\nstream-{i}.m3u8").unwrap();
    writeln!(
      text,
      "#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=10,URI=\"frame-{i}.m3u8\""
    )
    .unwrap();
  }
  text.push_str("#EXT-X-STREAM-INF:BANDWIDTH=42\nextra.m3u8\n");
  text.push_str("#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=10,URI=\"extra-frame.m3u8\"\n");
  text.push_str("#EXT-X-SESSION-KEY:METHOD=SAMPLE-AES,URI=\"key\",KEYFORMAT=\"com.apple.streamingkeydelivery\"\n");

  let info = inspect(&text).unwrap();
  assert_eq!(info.variants.len(), 64);
  assert_eq!(info.variants[0].uri, "stream-0.m3u8");
  assert_eq!(info.variants[63].uri, "frame-31.m3u8");
  assert!(info.has_drm);
}

#[test]
fn rejects_non_manifests_and_malformed_xml() {
  for markup in [
    "",
    "#EXTM3Uexample\n#EXTINF:1,\n",
    "#EXTM3U\n",
    "<html><MPD type='dynamic'/></html>",
    "<MPD><Period></MPD>",
    "<MPD><Period/>",
    "<MPD/><MPD/>",
    "<!DOCTYPE MPD SYSTEM 'https://example.org/dash.dtd'><MPD/>",
  ] {
    assert!(inspect(markup).is_none(), "{markup}");
  }
}
