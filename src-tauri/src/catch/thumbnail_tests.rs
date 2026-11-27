use super::*;
use std::sync::Mutex;

fn url(raw: &str) -> Url {
  Url::parse(raw).unwrap()
}

fn png(width: u32, height: u32) -> Vec<u8> {
  let mut bytes = Vec::new();
  image::RgbImage::from_pixel(width, height, image::Rgb([200, 30, 30]))
    .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
    .unwrap();
  bytes
}

#[test]
fn pictures_keep_the_poster_first_and_only_web_addresses() {
  let poster = url("https://cdn.example.com/poster.jpg");
  let page = url("https://example.com/og.jpg");
  assert_eq!(
    pictures(Some(poster.clone()), Some(page.clone())),
    [poster.clone(), page.clone()]
  );
  assert_eq!(
    pictures(None, Some(page.clone())),
    std::slice::from_ref(&page)
  );
  assert_eq!(
    pictures(Some(poster.clone()), Some(poster.clone())),
    [poster]
  );
  assert!(pictures(Some(url("data:image/png;base64,AA==")), None).is_empty());
  assert!(pictures(Some(url("file:///etc/passwd")), None).is_empty());
}

#[tokio::test]
async fn the_poster_wins_then_the_page_picture_then_one_frame() {
  let poster = url("https://cdn.example.com/poster.jpg");
  let page = url("https://example.com/og.jpg");
  // (poster works, page picture works, frame works) → what was asked, in order.
  let cases = [
    ((true, true, true), vec!["poster"]),
    ((false, true, true), vec!["poster", "page"]),
    ((false, false, true), vec!["poster", "page", "frame"]),
    ((false, false, false), vec!["poster", "page", "frame"]),
  ];
  for ((poster_ok, page_ok, frame_ok), expected) in cases {
    let asked = Mutex::new(Vec::new());
    let found = resolve(
      vec![poster.clone(), page.clone()],
      |picture| {
        let ok = if picture == poster {
          poster_ok
        } else {
          page_ok
        };
        asked
          .lock()
          .unwrap()
          .push(if picture == poster { "poster" } else { "page" });
        async move { ok.then(|| png(640, 360)) }
      },
      || {
        asked.lock().unwrap().push("frame");
        async move { frame_ok.then(|| png(320, 180)) }
      },
    )
    .await;
    assert_eq!(*asked.lock().unwrap(), expected);
    let any = poster_ok || page_ok || frame_ok;
    assert_eq!(found.is_some(), any);
    if let Some(found) = found {
      assert!(found.starts_with("data:image/jpeg;base64,"));
    }
  }
}

#[tokio::test]
async fn a_picture_that_does_not_decode_falls_through_to_the_next() {
  let found = resolve(
    vec![url("https://example.com/a.jpg")],
    |_| async { Some(b"<html>not a picture</html>".to_vec()) },
    || async { Some(png(64, 36)) },
  )
  .await;
  assert!(found.is_some(), "the frame took over");
}

#[test]
fn only_images_within_the_cap_are_read() {
  let cap = MAX_PICTURE_BYTES as u64;
  let cases = [
    (Some("image/jpeg"), Some(1_000), true),
    (Some("IMAGE/webp; charset=binary"), None, true),
    (Some("image/png"), Some(cap), true),
    (Some("image/png"), Some(cap + 1), false),
    (Some("image/svg+xml"), Some(10), false),
    (Some("text/html"), Some(10), false),
    (Some("application/octet-stream"), Some(10), false),
    (None, Some(10), false),
  ];
  for (content_type, length, expected) in cases {
    assert_eq!(
      picture_response(content_type, length),
      expected,
      "{content_type:?} {length:?}"
    );
  }
}

#[test]
fn pictures_shrink_to_320_wide_and_oversize_or_non_images_give_nothing() {
  let jpeg = small_jpeg(&png(1280, 720)).unwrap();
  let small = image::load_from_memory(&jpeg).unwrap();
  assert_eq!((small.width(), small.height()), (320, 180));
  // Narrow but tall: the height is capped too, the shape kept.
  let tall = image::load_from_memory(&small_jpeg(&png(320, 8000)).unwrap()).unwrap();
  assert_eq!((tall.width(), tall.height()), (51, 1280));
  assert!(small_jpeg(b"<svg xmlns='http://www.w3.org/2000/svg'/>").is_none());
  let mut oversize = png(16, 16);
  oversize.resize(MAX_PICTURE_BYTES + 1, 0);
  assert!(small_jpeg(&oversize).is_none());
}

/// A one-answer HTTP server on loopback that serves a real picture.
async fn picture_server() -> (u16, tokio::task::JoinHandle<bool>) {
  use tokio::io::{AsyncReadExt, AsyncWriteExt};
  let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let port = listener.local_addr().unwrap().port();
  let served = tokio::spawn(async move {
    let Ok(Ok((mut socket, _))) =
      tokio::time::timeout(Duration::from_secs(2), listener.accept()).await
    else {
      return false;
    };
    let mut request = [0; 1024];
    let _ = socket.read(&mut request).await;
    let body = png(8, 8);
    let head = format!(
      "HTTP/1.1 200 OK\r\ncontent-type: image/png\r\ncontent-length: {}\r\n\r\n",
      body.len()
    );
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(&body).await;
    true
  });
  (port, served)
}

#[tokio::test]
async fn a_picture_on_a_private_address_is_never_requested() {
  let (port, served) = picture_server().await;
  let headers = Headers {
    user_agent: "Test".into(),
    ..Headers::default()
  };
  let picture = url(&format!("http://127.0.0.1:{port}/poster.png"));
  assert!(fetch_picture(picture, &headers).await.is_none());
  assert!(
    !served.await.unwrap(),
    "the loopback server was never asked"
  );
  for private in [
    "http://10.0.0.7/p.jpg",
    "http://[::1]/p.jpg",
    "http://192.168.1.1/p.jpg",
  ] {
    assert!(
      fetch_picture(url(private), &headers).await.is_none(),
      "{private}"
    );
  }
}

const VOD: &str = "#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXTINF:10.0,\nseg0.ts\n#EXTINF:10.0,\nseg1.ts\n#EXTINF:10.0,\nseg2.ts\n#EXTINF:10.0,\nseg3.ts\n#EXTINF:10.0,\nseg4.ts\n#EXTINF:10.0,\nseg5.ts\n#EXTINF:10.0,\nseg6.ts\n#EXTINF:10.0,\nseg7.ts\n#EXTINF:10.0,\nseg8.ts\n#EXTINF:10.0,\nseg9.ts\n#EXT-X-ENDLIST\n";

#[test]
fn the_frame_segment_is_a_tenth_in_and_never_live_or_encrypted() {
  let base = url("https://cdn.example.com/v/index.m3u8");
  let piece = segment_at(VOD, &base, 0.1).unwrap();
  assert_eq!(piece.url.as_str(), "https://cdn.example.com/v/seg1.ts");
  assert_eq!(piece.init, None);
  assert_eq!(
    segment_at(VOD, &base, 0.0).unwrap().url.as_str(),
    "https://cdn.example.com/v/seg0.ts"
  );
  let fmp4 = VOD.replace(
    "#EXT-X-TARGETDURATION:10\n",
    "#EXT-X-TARGETDURATION:10\n#EXT-X-MAP:URI=\"init.mp4\"\n",
  );
  assert_eq!(
    segment_at(&fmp4, &base, 0.1)
      .unwrap()
      .init
      .unwrap()
      .as_str(),
    "https://cdn.example.com/v/init.mp4"
  );
  let live = VOD.replace("#EXT-X-ENDLIST\n", "");
  assert!(segment_at(&live, &base, 0.1).is_none(), "live");
  let encrypted = VOD.replace(
    "#EXT-X-TARGETDURATION:10\n",
    "#EXT-X-TARGETDURATION:10\n#EXT-X-KEY:METHOD=AES-128,URI=\"k\"\n",
  );
  assert!(segment_at(&encrypted, &base, 0.1).is_none(), "encrypted");
  let clear = VOD.replace(
    "#EXT-X-TARGETDURATION:10\n",
    "#EXT-X-TARGETDURATION:10\n#EXT-X-KEY:METHOD=NONE\n",
  );
  assert!(segment_at(&clear, &base, 0.1).is_some());
}

#[test]
fn a_master_playlist_gives_its_smallest_variant() {
  let master = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=5000000,RESOLUTION=1920x1080\nhi.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=400000,RESOLUTION=426x240\nlo.m3u8\n";
  let base = url("https://cdn.example.com/v/master.m3u8");
  assert_eq!(
    lowest_variant(master, &base).unwrap().as_str(),
    "https://cdn.example.com/v/lo.m3u8"
  );
  assert!(lowest_variant(VOD, &base).is_none());
}

#[test]
fn a_request_carries_only_the_media_cookies_that_match_it() {
  let cookie = |name: &'static str, secure: bool, path: &'static str| {
    Cookie::build((name, "1"))
      .domain("media.example.com")
      .secure(secure)
      .path(path)
      .build()
  };
  let jar = vec![
    cookie("plain", false, "/"),
    cookie("secure", true, "/"),
    cookie("scoped", false, "/v/"),
  ];
  let headers = Headers {
    cookies: media_cookies(&url("https://Media.Example.com/v/master.m3u8"), jar),
    ..Headers::default()
  };
  let sent = |raw: &str| cookie_for(&headers, &url(raw));
  assert_eq!(
    sent("https://media.example.com/v/seg1.ts").as_deref(),
    Some("plain=1; secure=1; scoped=1")
  );
  // An HLS piece or redirect over plain http never gets the Secure cookie.
  assert_eq!(
    sent("http://media.example.com/v/seg1.ts").as_deref(),
    Some("plain=1; scoped=1")
  );
  // A path cookie stays on its path.
  assert_eq!(
    sent("https://media.example.com/other/seg1.ts").as_deref(),
    Some("plain=1; secure=1")
  );
  // Another host gets nothing, even one the cookie's domain would cover.
  assert_eq!(sent("https://cdn.example.net/v/seg1.ts"), None);
  assert_eq!(sent("https://sub.media.example.com/v/seg1.ts"), None);
  assert!(media_cookies(&url("https://media.example.com/v.m3u8"), Vec::new()).is_none());
  assert_eq!(
    cookie_for(&Headers::default(), &url("https://media.example.com/")),
    None
  );
}

#[tokio::test(start_paused = true)]
async fn a_stalled_picture_never_holds_the_thumbnail_past_the_cap() {
  let resolved = tokio::time::timeout(
    TOTAL_CAP + Duration::from_secs(1),
    resolve(
      vec![url("https://cdn.example.com/poster.jpg")],
      |_| std::future::pending::<Option<Vec<u8>>>(),
      || async { Some(png(320, 180)) },
    ),
  )
  .await;
  assert_eq!(resolved, Ok(None), "resolve gives up by itself");
}

#[test]
fn a_bad_page_agent_falls_back_to_chrome() {
  assert_eq!(user_agent(Some("Agent/1.0")), "Agent/1.0");
  assert_eq!(user_agent(Some("Agent\u{1}")), net::CHROME_UA);
  assert_eq!(user_agent(Some("")), net::CHROME_UA);
  assert_eq!(user_agent(None), net::CHROME_UA);
}

#[test]
fn a_picture_request_names_only_the_page_origin() {
  assert_eq!(
    picture_referer(&url("https://example.com:8443/watch/42?session=secret#t")).as_deref(),
    Some("https://example.com:8443/")
  );
  assert_eq!(picture_referer(&url("data:text/plain,x")), None);
}

#[test]
fn thumbnail_redirects_only_send_the_referer_to_the_original_origin() {
  let original = url("https://media.example.com/poster.jpg");
  let referer = "https://example.com/watch/42?session=fixture";
  let headers = Headers {
    referer: Some(referer.into()),
    ..Headers::default()
  };
  for (target, expected) in [
    ("https://media.example.com/next.jpg", Some(referer)),
    ("https://cdn.example.com/next.jpg", None),
    ("https://media.example.com:8443/next.jpg", None),
    ("http://media.example.com/next.jpg", None),
  ] {
    assert_eq!(referer_for(&headers, &original, &url(target)), expected);
  }
}
