use super::*;

#[test]
fn a_caught_hls_request_hands_off_its_successful_browser_redirect() {
  use super::super::state::PageState;
  use serde_json::json;

  const REQUESTED: &str = "https://media.example.com/film.m3u8?token=initial";
  const FINAL: &str = "https://cdn.example.org/manifest.js?token=final";
  const OTHER_FRAME: &str = "https://other.example.com/frame";
  const HTTP: &str = "http://cdn.example.org/manifest.js";
  const LOOPBACK: &str = "https://127.0.0.1/manifest.js";
  let page_url = Url::parse("https://example.com/watch").unwrap();
  let top = page_url.as_str();
  let script = "application/javascript";
  for (method, response_url, frame, content_type, expected) in [
    ("fetch", FINAL, top, script, FINAL),
    ("xhr", FINAL, top, script, FINAL),
    ("perf:fetch", FINAL, top, script, REQUESTED),
    ("xhr", FINAL, OTHER_FRAME, script, REQUESTED),
    ("fetch", FINAL, top, "text/html", REQUESTED),
    ("fetch", HTTP, top, script, REQUESTED),
    ("fetch", LOOPBACK, top, script, REQUESTED),
  ] {
    let mut page = PageState::new(page_url.clone());
    let drain = |page: &mut PageState, hit| {
      let snapshot = json!([top, "Film", "Test browser", [hit]]);
      page.apply_drain(
        &page_url,
        &serde_json::to_string(&snapshot.to_string()).unwrap(),
      );
    };
    drain(
      &mut page,
      json!({
        "method": "playing", "url": REQUESTED, "frame": top,
        "isTop": true, "playing": true, "durationSec": 6738
      }),
    );
    let original_row = page.auto_view(false).mains.remove(0);
    let original_url = Url::parse(REQUESTED).unwrap();
    let original_handoff = page.document.handoff(&original_url);
    drain(
      &mut page,
      json!({
         "method": method, "url": response_url, "requestedUrl": REQUESTED,
         "frame": frame, "isTop": frame == top,
         "msgOrigin": (frame != top).then_some("https://other.example.com"),
         "ct": content_type
      }),
    );
    let rows = page.auto_view(false).mains;
    assert_eq!(rows.len(), 1, "{method} {response_url} {frame}");
    assert_eq!(rows[0].id, original_row.id);
    let candidate = page.document.handoff_candidate(&rows[0].id).unwrap();
    let request = CapturedRequest::new(candidate, context(page_url.as_str()));
    assert_eq!(
      request.url.as_str(),
      expected,
      "{method} {response_url} {frame}"
    );
    assert_eq!(
      Arc::ptr_eq(&original_handoff, &page.document.handoff(&original_url)),
      expected == REQUESTED,
      "{method} {response_url} {frame}"
    );
  }
}

#[test]
fn a_refused_stream_token_gets_a_fresh_catch_like_a_403() {
  for line in [
    "ERROR: [download] Got error: HTTP Error 401: Unauthorized. Giving up after 5 retries",
    "ERROR: unable to download video data: HTTP Error 403: Forbidden",
  ] {
    let error = classify_failure(line);
    assert_eq!(error, HandoffError::Forbidden, "{line}");
    assert_eq!(error.code(), "accessForbidden403", "{line}");
  }
  for line in [
    "ERROR: fragment 9 not found, unable to continue",
    // Not an error line: a warning or progress text that only mentions a status.
    "WARNING: [generic] Falling back on generic information extractor: HTTP Error 403",
    "[download] Got error: HTTP Error 401: Unauthorized. Retrying fragment 9 (3/5)...",
    // A status inside a link or a longer number is not the response status.
    "ERROR: Unable to download https://cdn.example.com/http 401/a.ts: timed out",
    "ERROR: [generic] Unable to download webpage: HTTP 4010 Bad Gateway",
  ] {
    assert_eq!(classify_failure(line), HandoffError::Runner, "{line}");
  }
  assert_eq!(
    classify_failure("[https @ 0x7f] HTTP error 403 Forbidden"),
    HandoffError::Forbidden
  );
}

#[test]
fn a_refused_stream_gets_one_fresh_catch_then_ends_refused() {
  let original = CapturedRequest {
    url: Url::parse("https://media.example.com/video.m3u8?token=old").unwrap(),
    context: context("https://player.example.com"),
    first_seen: Instant::now() - Duration::from_secs(1),
    protected: false,
    site: None,
  };
  let mut next = original.clone();
  next.first_seen = Instant::now();
  next.url.set_query(Some("token=new"));
  let fresh = next.clone();
  let mut capture = QueuedCapture {
    request: original,
    refresh: Some(Arc::new(move || Some(fresh.clone()))),
    selector: "b".into(),
  };
  assert!(retry_refused(&mut capture));
  assert_eq!(capture.request.url, next.url);
  // The refresh is spent: a second refusal ends the ladder (`stream_refused`).
  assert!(capture.refresh.is_none());
  assert!(!retry_refused(&mut capture));
}

#[test]
fn a_page_probe_keeps_the_browser_page_accept_header() {
  let page = Url::parse("https://films.example/watch/1").unwrap();
  let mut context = context("https://films.example");
  context.frame_url = page.to_string();
  let args = request_args(&page, &context, "generic").unwrap();
  assert!(
    !args.iter().any(|arg| arg.starts_with("Accept:")),
    "{args:?}"
  );
}

#[test]
fn drm_error_uses_the_protected_failure_kind() {
  let error = classify_failure("ERROR: [generic] This video is DRM protected");
  assert_eq!(error, HandoffError::Protected);
  assert_eq!(
    error.reason(),
    crate::capture::preview::FailReason::Protected
  );
}

#[tokio::test]
async fn a_hostname_resolving_to_loopback_is_refused() {
  let url = Url::parse("https://media.example.com/a.m3u8").unwrap();
  let result = admit_with(&url, |host, port| {
    assert_eq!((host, port), ("media.example.com", 443));
    std::future::ready(Ok(vec!["127.0.0.1:443".parse().unwrap()]))
  })
  .await;
  assert_eq!(result, Err(HandoffError::Refused));
}

#[test]
fn format_picker_has_one_video_per_height_and_one_audio_choice() {
  let info = parse_info(include_bytes!("fixtures/ytdlp-catch.json")).unwrap();
  let options: Vec<_> = info
    .formats
    .iter()
    .map(|f| {
      (
        f.id.as_str(),
        f.height,
        f.ext.as_str(),
        f.size_bytes,
        f.best,
      )
    })
    .collect();
  assert_eq!(
    options,
    [
      ("video-720", Some(720), "mp4", Some(2_000), true),
      ("video-360", Some(360), "mp4", None, false),
      ("audio", None, "m4a", Some(100), false),
    ]
  );
  assert_eq!(info.title.as_deref(), Some("Ocean waves"));
}

#[test]
fn catch_save_admits_a_captured_download_with_safe_row_metadata() {
  let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
  let sender = DownloadSender(send);
  let info = parse_info(include_bytes!("fixtures/ytdlp-catch.json")).unwrap();
  let request = CapturedRequest {
    url: Url::parse("https://media.example.com/video.m3u8?token=secret").unwrap(),
    context: context("https://player.example.com"),
    first_seen: Instant::now(),
    protected: false,
    site: None,
  };
  let page = Url::parse("https://example.org/watch/private?session=secret").unwrap();
  let saved = enqueue(
    &sender,
    &info,
    &request,
    &page,
    None,
    SaveSelection {
      format_id: Some("video-720"),
      download_id: Some("caught-row-id".into()),
      thumbnail: Some("data:image/jpeg;base64,AA==".into()),
    },
    None,
  )
  .unwrap();
  let DispatchRequest::Pipeline(DownloadRequest::Captured(entry)) = receive.try_recv().unwrap()
  else {
    panic!("expected the normal queue's captured download")
  };
  assert_eq!(saved.download_id, "caught-row-id");
  // The queue row's announcement carries the caught picture, once.
  assert_eq!(
    take_thumbnail("caught-row-id").as_deref(),
    Some("data:image/jpeg;base64,AA==")
  );
  assert_eq!(take_thumbnail("caught-row-id"), None);
  assert_eq!(entry.group_id, saved.download_id);
  assert_eq!(entry.id, saved.download_id);
  assert_eq!(entry.url, "https://example.org");
  assert_eq!(entry.format.height, Some(720));
  let private = entry.catch.unwrap();
  assert_eq!(private.request.url, request.url);
  assert_eq!(private.request.context.cookies[0].name(), "allow");
  let p = "[protocol~='^(https?|m3u8(_native)?|http_dash_segments)$']";
  assert_eq!(
    private.selector,
    format!("bv*[height=720]{p}+ba{p}/bv*[height=720]{p}")
  );
  remove_group(&saved.download_id);
}

#[test]
fn catch_audio_and_formats_stay_on_proxied_protocols() {
  let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
  let sender = DownloadSender(send);
  // Generic direct audio: unknown codecs, which `ba` alone never matches.
  let info = parse_info(br#"{"formats": null, "ext": "mp3", "protocol": "https"}"#).unwrap();
  assert_eq!(info.formats[0].id, "audio");
  let request = CapturedRequest {
    url: Url::parse("https://media.example.com/a.mp3").unwrap(),
    context: context("https://player.example.com"),
    first_seen: Instant::now(),
    protected: false,
    site: None,
  };
  let page = Url::parse("https://example.org/listen").unwrap();
  let saved = enqueue(
    &sender,
    &info,
    &request,
    &page,
    None,
    SaveSelection::default(),
    None,
  )
  .unwrap();
  let DispatchRequest::Pipeline(DownloadRequest::Captured(entry)) = receive.try_recv().unwrap()
  else {
    panic!("expected a captured download")
  };
  let p = "[protocol~='^(https?|m3u8(_native)?|http_dash_segments)$']";
  assert_eq!(entry.catch.unwrap().selector, format!("ba{p}/b{p}"));
  remove_group(&saved.download_id);

  let mixed = br#"{"formats": [
    {"ext": "mp4", "height": 1080, "protocol": "ftp"},
    {"ext": "flv", "height": 720, "protocol": "rtmp"},
    {"ext": "mp4", "height": 480},
    {"ext": "mp4", "height": 360, "protocol": "m3u8_native"}
  ]}"#;
  let heights: Vec<_> = parse_info(mixed)
    .unwrap()
    .formats
    .iter()
    .map(|f| f.height)
    .collect();
  assert_eq!(heights, [Some(360)]);
  let unsafe_only = br#"{"formats": [{"ext": "mp4", "protocol": "ftp"}]}"#;
  assert!(matches!(
    parse_info(unsafe_only),
    Err(HandoffError::NoFormats)
  ));
}

#[test]
fn a_top_level_drm_flag_refuses_even_with_clear_formats() {
  let info = br#"{"has_drm": true, "formats": [{"ext": "mp4", "protocol": "https"}]}"#;
  assert!(matches!(parse_info(info), Err(HandoffError::Protected)));
}

#[tokio::test]
async fn a_non_http_candidate_is_refused_before_lookup() {
  for raw in ["ftp://8.8.8.8/a.mp4", "rtmp://8.8.8.8:1935/live"] {
    let url = Url::parse(raw).unwrap();
    let result = admit_with(&url, |_, _| std::future::ready(Ok(vec![]))).await;
    assert_eq!(result, Err(HandoffError::Refused), "{raw}");
  }
}

struct Scratch(std::path::PathBuf);

impl Scratch {
  fn new() -> Self {
    let path = std::env::temp_dir().join(format!("catch-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path).unwrap();
    Self(path)
  }

  fn cookies(&self) -> CookieContext {
    CookieContext {
      directory: self.0.join("browser-cookies"),
      exporter: "yt-dlp".into(),
    }
  }
}

impl Drop for Scratch {
  fn drop(&mut self) {
    let _ = std::fs::remove_dir_all(&self.0);
  }
}

fn context(origin: &str) -> RequestContext {
  RequestContext {
    frame_url: "https://player.example.com/watch?id=42".into(),
    frame_origin: origin.into(),
    user_agent: "Catch test browser".into(),
    cookies: vec![Cookie::build(("allow", "fixture"))
      .domain(".example.com")
      .path("/")
      .http_only(true)
      .build()],
  }
}

#[test]
fn webview_context_is_forwarded_as_headers_and_a_private_cookie_file() {
  let scratch = Scratch::new();
  let url = Url::parse("https://media.example.com/master.m3u8?token=abc").unwrap();
  for origin in ["https://player.example.com", "null"] {
    let mut context = context(origin);
    context.cookies.push(
      Cookie::build(("other", "cookie"))
        .domain("sibling.example.com")
        .build(),
    );
    let args = request_args(&url, &context, "generic").unwrap();
    let value = |flag| {
      args
        .windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].as_str())
    };
    assert_eq!(value("--referer"), Some(context.frame_url.as_str()));
    assert_eq!(value("--user-agent"), Some(context.user_agent.as_str()));
    assert_eq!(value("--"), Some(url.as_str()));
    let headers: Vec<_> = args
      .windows(2)
      .filter(|pair| pair[0] == "--add-header")
      .map(|pair| pair[1].as_str())
      .collect();
    let origin_header = format!("Origin:{origin}");
    let mut expected = vec!["Accept:*/*"];
    if origin != "null" {
      expected.push(&origin_header);
    }
    assert_eq!(headers, expected);
    assert!(args
      .iter()
      .all(|arg| !arg.starts_with("Cookie:") && arg != "--allow-unplayable-formats"));

    let access = cookie_args(&scratch.cookies(), "media.example.com", &context).unwrap();
    let [flag, path] = access.args.as_slice() else {
      panic!("missing cookie flag");
    };
    assert_eq!(flag, "--cookies");
    let path = std::path::PathBuf::from(path);
    assert_eq!(
      std::fs::read_to_string(&path).unwrap(),
      "# Netscape HTTP Cookie File\n#HttpOnly_media.example.com\tFALSE\t/\tFALSE\t0\tallow\tfixture\n"
    );
    #[cfg(unix)]
    {
      use std::os::unix::fs::PermissionsExt;
      assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
      );
    }
    drop(access);
    assert!(!path.exists());
  }
}

#[test]
fn a_cookie_write_error_removes_the_private_jar() {
  let scratch = Scratch::new();
  let config = scratch.cookies();
  let mut context = context("null");
  context.cookies.push(
    Cookie::build(("invalid", "one\ntwo"))
      .domain("media.example.com")
      .build(),
  );
  assert!(matches!(
    cookie_args(&config, "media.example.com", &context),
    Err(HandoffError::InvalidContext)
  ));
  assert_eq!(std::fs::read_dir(config.directory).unwrap().count(), 0);
}

#[test]
fn a_refresh_requires_a_newer_url_in_the_same_frame() {
  let mut original = CapturedRequest {
    url: Url::parse("https://media.example.com/video.m3u8?token=old").unwrap(),
    context: context("https://player.example.com"),
    first_seen: Instant::now(),
    protected: false,
    site: None,
  };
  let mut next = original.clone();
  original.first_seen -= Duration::from_secs(1);
  next.url.set_query(Some("token=new"));
  let refresh: Refresh = Arc::new({
    let next = next.clone();
    move || Some(next.clone())
  });
  assert_eq!(refreshed(&original, Some(&refresh)).unwrap().url, next.url);
  next.context.frame_url.push_str("&frame=other");
  let other_frame: Refresh = Arc::new(move || Some(next.clone()));
  assert!(refreshed(&original, Some(&other_frame)).is_none());
}

#[tokio::test]
#[ignore = "requires yt-dlp and ffmpeg on PATH and loopback sockets"]
async fn protected_hls_saves_via_yt_dlp_with_webview_credentials() {
  use crate::runners::tool_process::async_command;
  use std::path::Path;
  use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
  };

  async fn reply(stream: TcpStream, root: &Path) {
    let mut request = BufReader::new(stream);
    let mut line = String::new();
    request.read_line(&mut line).await.unwrap();
    let mut parts = line.split_whitespace();
    let head = parts.next() == Some("HEAD");
    let name = parts.next().unwrap().trim_start_matches('/').to_owned();
    let mut headers = Vec::new();
    loop {
      line.clear();
      if request.read_line(&mut line).await.unwrap() == 0 || line == "\r\n" {
        break;
      }
      headers.push(line.trim().to_ascii_lowercase());
    }
    let required = [
      "referer: https://player.example.com/watch?id=42",
      "origin: https://player.example.com",
      "user-agent: catch test browser",
      "cookie: allow=fixture",
    ];
    let (status, content) = if !headers.iter().any(|h| h == "accept: */*") {
      // A streaming proxy refuses anything but a player's fetch of its segment links.
      ("401 Unauthorized", vec![])
    } else if required
      .iter()
      .any(|h| !headers.iter().any(|value| value == h))
    {
      ("403 Forbidden", vec![])
    } else if name.contains(['/', '\\']) {
      ("404 Not Found", vec![])
    } else {
      match tokio::fs::read(root.join(&name)).await {
        Ok(content) => ("200 OK", content),
        Err(_) => ("404 Not Found", vec![]),
      }
    };
    let mime = if name.ends_with(".m3u8") {
      "application/vnd.apple.mpegurl"
    } else {
      "video/mp2t"
    };
    let header = format!(
      "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: {mime}\r\nConnection: close\r\n\r\n",
      content.len()
    );
    let stream = request.get_mut();
    stream.write_all(header.as_bytes()).await.unwrap();
    if !head {
      stream.write_all(&content).await.unwrap();
    }
  }

  let scratch = Scratch::new();
  for (width, height) in [(160, 90), (320, 180)] {
    let result = async_command("ffmpeg")
      .unwrap()
      .kill_on_drop(true)
      .current_dir(&scratch.0)
      .args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        &format!("testsrc2=size={width}x{height}:rate=24"),
        "-t",
        "1",
        "-an",
        "-c:v",
        "mpeg2video",
        "-threads",
        "1",
        "-f",
        "hls",
        &format!("{height}.m3u8"),
      ])
      .output()
      .await
      .unwrap();
    assert!(
      result.status.success(),
      "{}",
      String::from_utf8_lossy(&result.stderr)
    );
  }
  std::fs::write(scratch.0.join("master.m3u8"),
    "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=150000,RESOLUTION=160x90\n90.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=400000,RESOLUTION=320x180\n180.m3u8\n",
  ).unwrap();
  let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = server.local_addr().unwrap();
  let root = scratch.0.clone();
  let service = tokio::spawn(async move {
    while let Ok((stream, _)) = server.accept().await {
      reply(stream, &root).await;
    }
  });
  let host = "media.example.com";
  // This controlled fixture pins loopback directly. Production admission rejects loopback.
  let route = Route::start(host.into(), address.port(), vec![address])
    .await
    .unwrap();
  let url = Url::parse(&format!("http://{host}:{}/master.m3u8", address.port())).unwrap();
  let context = context("https://player.example.com");
  let access = cookie_args(&scratch.cookies(), host, &context).unwrap();
  let mut command = async_command("yt-dlp").unwrap();
  command.kill_on_drop(true).current_dir(&scratch.0).args([
    "--ignore-config",
    "--no-plugin-dirs",
    "--proxy",
    &route.proxy,
  ]);
  let args = request_args(&url, &context, "generic").unwrap();
  let denied = command.args(["-J"]).args(&args).output().await.unwrap();
  assert!(!denied.status.success());
  assert_eq!(
    classify_failure(&String::from_utf8_lossy(&denied.stderr)),
    HandoffError::Forbidden
  );
  // With every credential but a page load's Accept, the proxy answers 401: also a refusal.
  let page_accept: Vec<String> = args
    .iter()
    .map(|arg| {
      if arg == "Accept:*/*" {
        "Accept:text/html".to_owned()
      } else {
        arg.clone()
      }
    })
    .collect();
  let unauthorized = async_command("yt-dlp")
    .unwrap()
    .kill_on_drop(true)
    .current_dir(&scratch.0)
    .args([
      "--ignore-config",
      "--no-plugin-dirs",
      "--proxy",
      &route.proxy,
    ])
    .args(&access.args)
    .args(["-J"])
    .args(&page_accept)
    .output()
    .await
    .unwrap();
  let stderr = String::from_utf8_lossy(&unauthorized.stderr);
  assert!(stderr.contains("HTTP Error 401"), "{stderr}");
  assert_eq!(classify_failure(&stderr), HandoffError::Forbidden);

  let run = |extra: Vec<&str>| {
    let mut command = async_command("yt-dlp").unwrap();
    command
      .kill_on_drop(true)
      .current_dir(&scratch.0)
      .args([
        "--ignore-config",
        "--no-plugin-dirs",
        "--proxy",
        &route.proxy,
      ])
      .args(&access.args)
      .args(extra)
      .args(&args);
    async move {
      tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .unwrap()
        .unwrap()
    }
  };
  let metadata = run(vec!["-J"]).await;
  assert!(
    metadata.status.success(),
    "{}",
    String::from_utf8_lossy(&metadata.stderr)
  );
  let info = parse_info(&metadata.stdout).unwrap();
  assert_eq!(
    info.formats.iter().map(|f| f.height).collect::<Vec<_>>(),
    [Some(180), Some(90)]
  );
  let saved = run(vec![
    "-f",
    "bv*[height=180]+ba/bv*[height=180]",
    "-o",
    "saved.%(ext)s",
  ])
  .await;
  assert!(
    saved.status.success(),
    "{}",
    String::from_utf8_lossy(&saved.stderr)
  );
  assert!(
    std::fs::metadata(scratch.0.join("saved.mp4"))
      .unwrap()
      .len()
      > 0
  );
  service.abort();
}

#[test]
fn catch_cookies_match_media_host_path_and_transport() {
  use tauri::webview::Cookie;
  let cookie = |domain, path, secure| {
    Cookie::build(("name", "value"))
      .domain(domain)
      .path(path)
      .secure(secure)
      .build()
  };
  let https = Url::parse("https://cdn.example.org/media/video.m3u8").unwrap();
  let http = Url::parse("http://cdn.example.org/media/video.m3u8").unwrap();
  assert!(cookie_matches_url(
    &cookie(".example.org", "/media", true),
    &https
  ));
  assert!(cookie_matches_url(
    &cookie("cdn.example.org", "/", false),
    &https
  ));
  assert!(!cookie_matches_url(
    &cookie("example.org", "/", false),
    &https
  ));
  assert!(!cookie_matches_url(
    &cookie(".cdn.example.org.evil.com", "/", false),
    &https
  ));
  assert!(!cookie_matches_url(
    &cookie(".example.org", "/med", false),
    &https
  ));
  assert!(!cookie_matches_url(
    &cookie(".example.org", "/", true),
    &http
  ));
}

#[test]
fn manifest_redirects_reenter_url_admission() {
  let url = Url::parse("https://cdn.example.org/stream.m3u8").unwrap();
  assert!(manifest_redirect(&url, "http://127.0.0.1/live.m3u8").is_none());
  assert!(manifest_redirect(&url, "https://user:secret@other.example.org/x").is_none());
  assert_eq!(
    manifest_redirect(&url, "/next.m3u8").unwrap().as_str(),
    "https://cdn.example.org/next.m3u8"
  );
}

#[test]
fn a_caught_hls_row_is_named_after_the_page_not_the_playlist_file() {
  let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
  let sender = DownloadSender(send);
  // yt-dlp's generic extractor names a direct playlist after its file name.
  let info = parse_info(
    br#"{"title": "index-v1-a1", "formats": [{"ext": "mp4", "protocol": "m3u8_native"}]}"#,
  )
  .unwrap();
  let request = CapturedRequest {
    url: Url::parse("https://cdn.example.com/s/index-v1-a1.json?t=secret").unwrap(),
    context: context("https://player.example.com"),
    first_seen: Instant::now(),
    protected: false,
    site: None,
  };
  let page = Url::parse("https://movies.example.org/watch/1").unwrap();
  let title = "Watch The Intouchables (2011) Online Free | Movies4U";
  let saved = enqueue(
    &sender,
    &info,
    &request,
    &page,
    Some(title),
    SaveSelection::default(),
    None,
  )
  .unwrap();
  let DispatchRequest::Pipeline(DownloadRequest::Captured(entry)) = receive.try_recv().unwrap()
  else {
    panic!("expected a captured download")
  };
  let values = &entry.template_context.values;
  assert_eq!(values.get("title").map(String::as_str), Some(title));
  assert_eq!(values.get("fulltitle").map(String::as_str), Some(title));
  remove_group(&saved.download_id);

  // No page title: the probe's title is still better than nothing.
  let saved = enqueue(
    &sender,
    &info,
    &request,
    &page,
    None,
    SaveSelection::default(),
    None,
  )
  .unwrap();
  let DispatchRequest::Pipeline(DownloadRequest::Captured(entry)) = receive.try_recv().unwrap()
  else {
    panic!("expected a captured download")
  };
  assert_eq!(
    entry
      .template_context
      .values
      .get("title")
      .map(String::as_str),
    Some("index-v1-a1")
  );
  remove_group(&saved.download_id);
}

#[test]
fn a_long_hls_download_survives_one_failed_fragment() {
  let url = Url::parse("https://media.example.com/master.m3u8").unwrap();
  let args = request_args(&url, &context("https://player.example.com"), "generic").unwrap();
  let count = |flag: &str| {
    args
      .windows(2)
      .find(|pair| pair[0] == flag)
      .and_then(|pair| pair[1].parse::<u32>().ok())
  };
  // A two-hour film has well over 600 fragments; one hiccup must not discard the download.
  assert!(count("--fragment-retries").is_some_and(|n| n > 0));
  assert!(count("--retries").is_some_and(|n| n > 0));
  // Every retry backs off, and a gap is still never saved.
  assert!(args
    .windows(2)
    .any(|pair| pair[0] == "--retry-sleep" && pair[1].starts_with("fragment:")));
  assert!(args
    .iter()
    .any(|arg| arg == "--abort-on-unavailable-fragments"));
}

#[test]
fn a_failure_detail_keeps_the_error_and_drops_links() {
  let lines = [
    "WARNING: Live HLS streams are not supported by the native downloader.",
    "[download] Got error: HTTP Error 403: Forbidden. Retrying fragment 412 (1/5)...",
    "ERROR: [download] Got error: HTTP Error 403: Forbidden. Giving up after 5 retries",
    "ERROR: unable to download https://cdn.example.com/s/seg-412.ts?t=rbFcO6tj1gdh66_TZX&s=179",
  ]
  .map(String::from);
  let detail = failure_detail(&lines);
  assert!(detail.contains("HTTP Error 403: Forbidden. Giving up after 5 retries"));
  assert!(!detail.contains("cdn.example.com"));
  assert!(!detail.contains("t=rbFc"));
  assert!(!detail.contains("seg-412.ts"));

  // The catch route's own proxy address carries its secret, and this computer's names are private.
  let mut lines =
    vec!["ERROR: Unable to connect to proxy socks5h://catch:fixturesecret@127.0.0.1:50123".into()];
  let names = crate::diagnostics::redact::local_names();
  lines.extend(
    names
      .iter()
      .map(|name| format!("ERROR: no access for {name} here")),
  );
  let detail = failure_detail(&lines);
  assert!(detail.contains("Unable to connect to proxy"));
  assert!(
    !detail.contains("fixturesecret") && !detail.contains("127.0.0.1"),
    "{detail}"
  );
  for name in names {
    assert!(!detail.contains(name.as_str()), "{detail}");
  }
}

/// Automatic catch: a sign-in copied into the hidden page (domain cookie of the link's site)
/// is what the page sends to the media address, and the hand-off writes it into yt-dlp's jar.
#[test]
fn a_sign_in_copied_into_a_hidden_page_reaches_the_caught_media_download() {
  use crate::catch::hidden_signin::{merge, site_only};
  use tauri::webview::cookie::{time::OffsetDateTime, Expiration};
  let now = 1_800_000_000;
  let signed = |name: &str, domain: &str| {
    Cookie::build((name.to_owned(), "signed-in".to_owned()))
      .domain(domain.to_owned())
      .path("/")
      .secure(true)
      .expires(Expiration::DateTime(
        OffsetDateTime::from_unix_timestamp(now + 600).unwrap(),
      ))
      .build()
  };
  let copied = merge(
    site_only(vec![signed("sid", ".example.com")], "www.example.com", now),
    site_only(vec![signed("other", ".other.org")], "www.example.com", now),
  );
  let media = Url::parse("https://media.example.com/master.m3u8").unwrap();
  let mut context = context("https://www.example.com");
  // `media_cookies`: what the hidden page's store sends to the media address.
  context.cookies = copied
    .into_iter()
    .filter(|cookie| cookie_matches_url(cookie, &media))
    .collect();
  let scratch = Scratch::new();
  let access = cookie_args(&scratch.cookies(), "media.example.com", &context).unwrap();
  let jar = std::fs::read_to_string(&access.args[1]).unwrap();
  assert!(
    jar.contains("media.example.com\tFALSE\t/\tTRUE\t1800000600\tsid\tsigned-in"),
    "{jar}"
  );
  assert!(!jar.contains("\tother\t"));
}

#[test]
fn an_offered_site_page_is_read_by_its_site_and_a_caught_address_never_is() {
  let extractors = |args: &[String]| {
    args
      .windows(2)
      .find(|pair| pair[0] == "--use-extractors")
      .map(|pair| pair[1].clone())
  };
  let short = Url::parse("https://www.youtube.com/shorts/ynQcf3PSsyI").unwrap();
  let youtube = context("https://www.youtube.com");
  let video = super::super::page_video::page_video(&short).unwrap();
  let offered = Candidate::site_page(short.clone(), video.extractors);
  let args = CapturedRequest::new(&offered, youtube.clone())
    .args()
    .unwrap();
  assert_eq!(extractors(&args).as_deref(), Some("youtube"));
  assert_eq!(args.last().map(String::as_str), Some(short.as_str()));

  // The same address from the retry ladder or from a page's own request was vetted by nobody.
  let ladder = Candidate::page(short.clone());
  let mut set = super::super::candidate::CandidateSet::default();
  let caught = set
    .observe(super::super::candidate::Observation {
      url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ&x=.mp4",
      requested_url: None,
      content_type: Some("video/mp4"),
      frame_url: "https://evil.example/",
      frame_origin: "https://evil.example",
      source: super::super::candidate::Source::Fetch,
      first_seen: Instant::now(),
      first_bytes: &[],
      is_byte_range: false,
      is_hls_segment: false,
      loaded: false,
      eme_probe: false,
      eme_active: false,
    })
    .expect("admitted as a media address");
  for candidate in [&ladder, caught] {
    let args = CapturedRequest::new(candidate, youtube.clone())
      .args()
      .unwrap();
    assert_eq!(extractors(&args).as_deref(), Some("generic"));
  }
}

#[test]
fn a_site_page_save_is_named_by_yt_dlp_not_by_the_tab_title() {
  let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
  let sender = DownloadSender(send);
  let info = parse_info(
    br#"{"title": "Second Short", "formats": [{"ext": "mp4", "protocol": "https", "height": 720, "vcodec": "avc1", "acodec": "none"}]}"#,
  )
  .unwrap();
  let short = Url::parse("https://www.youtube.com/shorts/AbCdEfGhIjK").unwrap();
  let request = CapturedRequest::new(
    &Candidate::site_page(short.clone(), "youtube"),
    context("https://www.youtube.com"),
  );
  // The tab still shows the previous Short's title right after the in-page address change.
  let saved = enqueue(
    &sender,
    &info,
    &request,
    &short,
    Some("First Short"),
    SaveSelection::default(),
    None,
  )
  .unwrap();
  let DispatchRequest::Pipeline(DownloadRequest::Captured(entry)) = receive.try_recv().unwrap()
  else {
    panic!("expected a captured download")
  };
  assert_eq!(
    entry
      .template_context
      .values
      .get("title")
      .map(String::as_str),
    Some("Second Short")
  );
  remove_group(&saved.download_id);
}
