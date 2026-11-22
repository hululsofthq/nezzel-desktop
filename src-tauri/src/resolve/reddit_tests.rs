use super::*;
use super::{reply, FakeTransport};

const CANONICAL: &str = "https://www.reddit.com/comments/abc123/";

#[tokio::test]
async fn reddit_normalizes_post_aliases_before_fallback() {
  let permalink = "https://www.reddit.com/r/test/comments/abc123/title/";
  for (source, page) in [
    (
      "https://www.reddit.com/r/test/comments/abc123/title/?utm_source=share",
      permalink,
    ),
    (
      "https://old.reddit.com/r/test/comments/abc123/title/",
      permalink,
    ),
    ("https://new.reddit.com/comments/abc123/", CANONICAL),
    ("https://sh.reddit.com/comments/abc123/", CANONICAL),
    ("https://redd.it/abc123", CANONICAL),
    ("https://www.reddit.com/gallery/abc123", CANONICAL),
    (
      "https://www.reddit.com/r/test/comments/abc123/title/#lightbox",
      permalink,
    ),
  ] {
    let transport = FakeTransport(std::sync::Mutex::new(
      [(page, reply(403, None, b""))].into(),
    ));
    assert_eq!(
      read_site(source, &transport).await.resolution(source),
      Resolution::Refused {
        url: CANONICAL.into()
      },
      "{source}"
    );
  }
}

#[tokio::test]
async fn reddit_share_redirect_resolves_to_the_post_before_fallback() {
  let source = "https://www.reddit.com/r/test/s/Share123/";
  let transport = FakeTransport(std::sync::Mutex::new(
    [
      (
        source,
        reply(
          301,
          Some("/r/test/comments/abc123/title/?share_id=tracking"),
          b"",
        ),
      ),
      (
        "https://www.reddit.com/r/test/comments/abc123/title/",
        reply(403, None, b""),
      ),
    ]
    .into(),
  ));
  assert_eq!(
    read_site(source, &transport).await.resolution(source),
    Resolution::Refused {
      url: CANONICAL.into()
    }
  );
}

#[tokio::test]
async fn reddit_share_image_preserves_its_readable_permalink_for_download_refresh() {
  let source = "https://www.reddit.com/r/test/s/Share123/";
  let permalink = "https://www.reddit.com/r/test/comments/abc123/title/";
  let photo = b"<shreddit-post id='t3_abc123' post-type='image' content-href='https://i.redd.it/full.jpg'></shreddit-post>";
  let transport = FakeTransport(std::sync::Mutex::new(
    [
      (source, reply(301, Some(permalink), b"")),
      (permalink, reply(200, None, photo)),
    ]
    .into(),
  ));
  let Verdict::Verified(Resolution::Attachments { metadata }) = read_site(source, &transport).await
  else {
    panic!("image post expected");
  };
  let media: serde_json::Value = serde_json::from_str(&metadata).unwrap();
  assert_eq!(media["url"], permalink);
  let refresh = FakeTransport(std::sync::Mutex::new(
    [(permalink, reply(200, None, photo))].into(),
  ));
  assert!(matches!(
    read_site(media["url"].as_str().unwrap(), &refresh).await,
    Verdict::Verified(Resolution::Attachments { .. })
  ));
}

#[tokio::test]
async fn reddit_routes_only_the_requested_posts_media_from_fixtures() {
  let cases: serde_json::Value =
    serde_json::from_str(include_str!("../../tests/fixtures/reddit/posts.json")).unwrap();
  for case in cases.as_array().unwrap() {
    let body = case["html"].as_str().unwrap();
    let transport = FakeTransport(std::sync::Mutex::new(
      [(CANONICAL, reply(200, None, body.as_bytes()))].into(),
    ));
    let result = read_site(CANONICAL, &transport).await.resolution(CANONICAL);
    if let Some(target) = case["target"].as_str() {
      let actual = match result {
        Resolution::PostMedia {
          url,
          title,
          uploader,
          ..
        } => {
          assert_eq!(title.as_deref(), Some("Synthetic post"));
          assert_eq!(uploader.as_deref(), Some("synthetic"));
          url
        }
        _ => panic!("target expected"),
      };
      assert_eq!(actual, target, "{}", case["name"]);
    } else {
      let Resolution::Attachments { metadata } = result else {
        panic!("attachment post expected: {}", case["name"]);
      };
      let media: serde_json::Value = serde_json::from_str(&metadata).unwrap();
      assert_eq!(
        (
          media["mediaKind"].clone(),
          media["attachments"].as_array().unwrap().len()
        ),
        (
          case["kind"].clone(),
          case["count"].as_u64().unwrap() as usize
        ),
        "{}",
        case["name"]
      );
    }
  }
}

#[tokio::test]
async fn reddit_never_admits_another_posts_or_blurred_media() {
  for html in [
    "<shreddit-post id='t3_wrong' post-type='image' content-href='https://i.redd.it/decoy.jpg'></shreddit-post>",
    "<shreddit-post id='t3_abc123' post-type='image' nsfw content-href='https://i.redd.it/blur.jpg'></shreddit-post>",
    "<shreddit-post id='t3_abc123' post-type='image' content-href='https://preview.redd.it/image.jpg?blur=40'></shreddit-post>",
    "<shreddit-post id='t3_abc123' post-type='image'><shreddit-post id='t3_other' post-type='image' content-href='https://i.redd.it/decoy.jpg'><img src='https://i.redd.it/decoy.jpg'></shreddit-post></shreddit-post>",
  ] {
    let transport = FakeTransport(std::sync::Mutex::new([(CANONICAL, reply(200, None, html.as_bytes()))].into()));
    assert_eq!(read_site(CANONICAL, &transport).await.resolution(CANONICAL), Resolution::Passthrough(CANONICAL.into()));
  }
}

#[tokio::test]
async fn reddit_redirects_reject_external_hosts_and_changed_post_ids() {
  for target in [
    "https://evil.example/comments/abc123/",
    "http://www.reddit.com/comments/abc123/",
    "https://www.reddit.com/login",
    "https://www.reddit.com/comments/other123/",
  ] {
    let transport = FakeTransport(std::sync::Mutex::new(
      [(CANONICAL, reply(302, Some(target), b""))].into(),
    ));
    assert_eq!(
      read_site(CANONICAL, &transport).await.resolution(CANONICAL),
      Resolution::Passthrough(CANONICAL.into())
    );
  }
}

#[tokio::test]
async fn reddit_challenge_is_answered_once_on_the_same_page_without_cookies() {
  let body = br#"<form method="get" action="/comments/abc123/"><input name="jsc_token" value="token"><input name="solution" value=""></form><script>const x=await(async e=>e+e)("abcdef12")</script>"#;
  let photo = b"<shreddit-post id='t3_abc123' post-type='image' content-href='https://i.redd.it/full.jpg'></shreddit-post>";
  let transport = FakeTransport(std::sync::Mutex::new(
    [
      (CANONICAL, reply(200, None, body)),
      (
        "https://www.reddit.com/comments/abc123/?jsc_token=token&solution=abcdef12abcdef12",
        reply(200, None, photo),
      ),
    ]
    .into(),
  ));
  assert!(matches!(
    read_site(CANONICAL, &transport).await,
    Verdict::Verified(Resolution::Attachments { .. })
  ));
}

#[tokio::test]
async fn reddit_challenge_is_answered_again_after_a_redirect_to_another_page() {
  let challenge = |path: &str| {
    format!(
      r#"<form method="get" action="{path}"><input name="jsc_token" value="token"><input name="solution" value=""></form><script>const x=await(async e=>e+e)("abcdef12")</script>"#
    )
  };
  let slug = "/r/test/comments/abc123/title/";
  let photo = b"<shreddit-post id='t3_abc123' post-type='image' content-href='https://i.redd.it/full.jpg'></shreddit-post>";
  let transport = FakeTransport(std::sync::Mutex::new(
    [
      (
        CANONICAL,
        reply(200, None, challenge("/comments/abc123/").as_bytes()),
      ),
      (
        "https://www.reddit.com/comments/abc123/?jsc_token=token&solution=abcdef12abcdef12",
        reply(301, Some(slug), b""),
      ),
      (
        "https://www.reddit.com/r/test/comments/abc123/title/",
        reply(200, None, challenge(slug).as_bytes()),
      ),
      ("https://www.reddit.com/r/test/comments/abc123/title/?jsc_token=token&solution=abcdef12abcdef12", reply(200, None, photo)),
    ]
    .into(),
  ));
  assert!(matches!(
    read_site(CANONICAL, &transport).await,
    Verdict::Verified(Resolution::Attachments { .. })
  ));
}

#[tokio::test]
async fn reddit_collections_carousel_gallery_yields_its_pages() {
  let body = "<shreddit-post id='t3_abc123' post-type='gallery' post-title='Synthetic ad' author='synthetic' content-href='https://www.reddit.com/gallery/abc123'><collections-carousel post-id='t3_abc123'><ul>\
    <li slot='page-0'><figure><img src='https://preview.redd.it/one.png?width=640&amp;s=x' srcset='https://preview.redd.it/one.png?width=320&amp;s=a 320w, https://preview.redd.it/one.png?width=1080&amp;s=b 1080w'></figure></li>\
    <li slot='page-1'><figure><img src='https://preview.redd.it/two.png?width=640&amp;s=y'></figure></li>\
    </ul></collections-carousel></shreddit-post>";
  let transport = FakeTransport(std::sync::Mutex::new(
    [(CANONICAL, reply(200, None, body.as_bytes()))].into(),
  ));
  let Verdict::Verified(Resolution::Attachments { metadata }) =
    read_site(CANONICAL, &transport).await
  else {
    panic!("collections carousel was not read");
  };
  let media: serde_json::Value = serde_json::from_str(&metadata).unwrap();
  let items = media["attachments"].as_array().unwrap();
  assert_eq!(items.len(), 2);
  assert!(
    items[0]["url"].as_str().unwrap().contains("width=1080"),
    "{media}"
  );
}

#[tokio::test]
async fn reddit_profile_gallery_served_from_the_cf_preview_cdn_yields_its_pages() {
  // Some profile-post pages host their carousel thumbnails on cf.preview.redd.it.
  let body = "<shreddit-post id='t3_abc123' post-type='gallery' post-title='Synthetic' author='synthetic' content-href='https://www.reddit.com/gallery/abc123'><collections-carousel post-id='t3_abc123'><ul>\
    <li slot='page-0'><figure><img src='https://cf.preview.redd.it/one.png?width=640&amp;crop=smart&amp;s=x' srcset='https://cf.preview.redd.it/one.png?width=320&amp;s=a 320w, https://cf.preview.redd.it/one.png?width=1080&amp;s=b 1080w'></figure></li>\
    <li slot='page-1'><figure><img src='https://cf.preview.redd.it/two.png?width=640&amp;s=y'></figure></li>\
    </ul></collections-carousel></shreddit-post>";
  let transport = FakeTransport(std::sync::Mutex::new(
    [(CANONICAL, reply(200, None, body.as_bytes()))].into(),
  ));
  let Verdict::Verified(Resolution::Attachments { metadata }) =
    read_site(CANONICAL, &transport).await
  else {
    panic!("cf.preview carousel was not read");
  };
  let media: serde_json::Value = serde_json::from_str(&metadata).unwrap();
  assert_eq!(media["attachments"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn reddit_devvit_custom_post_is_app_post_only_with_its_own_devvit_surface() {
  for (inner, outside, expected) in [
    // Real shape of an app post (r/ruinit, 2026-11-22): app attributes, its own permalink as
    // content-href, and no text-body slot; the app draws itself in the browser.
    (
      "<devvit2-surface slot='post-media-container'></devvit2-surface>",
      "",
      true,
    ),
    ("<devvit2-custom-post></devvit2-custom-post>", "", true),
    ("<devvit2-surface></devvit2-surface>", "", true),
    ("", "", false),
    // A devvit element that belongs to another post proves nothing about this one.
    (
      "",
      "<shreddit-post id='t3_other'><devvit2-surface></devvit2-surface></shreddit-post>",
      false,
    ),
  ] {
    let body = format!(
      "<shreddit-post id='t3_abc123' post-type='customPost' app-name='ruinin-it' app-slug='ruinin-it' domain='self.ruinit' content-href='https://www.reddit.com/r/ruinit/comments/abc123/title/'>{inner}</shreddit-post>{outside}"
    );
    let transport = FakeTransport(std::sync::Mutex::new(
      [(CANONICAL, reply(200, None, body.as_bytes()))].into(),
    ));
    let result = read_site(CANONICAL, &transport).await.resolution(CANONICAL);
    assert_eq!(
      result,
      if expected {
        Resolution::Unsupported {
          reason: Reason::AppPost,
        }
      } else {
        Resolution::Passthrough(CANONICAL.into())
      },
      "{body}"
    );
  }
}

#[tokio::test]
async fn reddit_user_profile_share_follows_its_redirect_to_the_post() {
  let source = "https://www.reddit.com/u/someone/s/Share123";
  let location = "https://www.reddit.com/user/someone/comments/abc123/title/?action_bar_refresh_eligible=1&campaignId=1000000000000000001&impressionid=2000000000000000002&p=1&share_id=tracking&utm_content=share_button&utm_medium=web3x&utm_name=web3xcss&utm_source=share&utm_term=1";
  let photo = b"<shreddit-post id='t3_abc123' post-type='image' content-href='https://i.redd.it/full.jpg'></shreddit-post>";
  let transport = FakeTransport(std::sync::Mutex::new(
    [
      (
        "https://www.reddit.com/u/someone/s/Share123/",
        reply(301, Some(location), b""),
      ),
      (
        "https://www.reddit.com/user/someone/comments/abc123/title/",
        reply(200, None, photo),
      ),
    ]
    .into(),
  ));
  assert!(matches!(
    read_site(source, &transport).await,
    Verdict::Verified(Resolution::Attachments { .. })
  ));
}

#[tokio::test]
async fn reddit_video_shortlink_binds_its_media_id_after_the_post_redirect() {
  let video = "https://www.reddit.com/video/abcdefgh1234/";
  let cases: serde_json::Value =
    serde_json::from_str(include_str!("../../tests/fixtures/reddit/posts.json")).unwrap();
  let html = cases
    .as_array()
    .unwrap()
    .iter()
    .find(|c| c["name"] == "video")
    .unwrap()["html"]
    .as_str()
    .unwrap();
  let transport = FakeTransport(std::sync::Mutex::new(
    [
      (video, reply(302, Some(CANONICAL), b"")),
      (CANONICAL, reply(200, None, html.as_bytes())),
    ]
    .into(),
  ));
  assert!(
    matches!(read_site("https://v.redd.it/abcdefgh1234", &transport).await, Verdict::Verified(Resolution::PostMedia { url, .. }) if url == "https://v.redd.it/abcdefgh1234/DASHPlaylist.mpd")
  );
}

#[tokio::test]
async fn reddit_video_without_a_post_title_gets_a_neutral_title_not_the_manifest_name() {
  let cases: serde_json::Value =
    serde_json::from_str(include_str!("../../tests/fixtures/reddit/posts.json")).unwrap();
  let html = cases
    .as_array()
    .unwrap()
    .iter()
    .find(|c| c["name"] == "video")
    .unwrap()["html"]
    .as_str()
    .unwrap();
  for (html, want) in [
    (html.to_owned(), "Synthetic post"),
    (
      html.replace(" post-title=\"Synthetic post\"", ""),
      "Reddit video abcdefgh1234",
    ),
  ] {
    let transport = FakeTransport(std::sync::Mutex::new(
      [
        (
          "https://www.reddit.com/video/abcdefgh1234/",
          reply(302, Some(CANONICAL), b""),
        ),
        (CANONICAL, reply(200, None, html.as_bytes())),
      ]
      .into(),
    ));
    let verdict = read_site("https://v.redd.it/abcdefgh1234", &transport).await;
    assert!(
      matches!(&verdict, Verdict::Verified(Resolution::PostMedia { title, .. }) if title.as_deref() == Some(want)),
      "{want}"
    );
  }
}

#[tokio::test]
async fn reddit_requires_public_media_dns_before_handoff() {
  struct Refusing(FakeTransport);
  impl http::Transport for Refusing {
    async fn get(
      &self,
      url: &reqwest::Url,
      cap: usize,
    ) -> Result<http::Response, http::FetchError> {
      http::Transport::get(&self.0, url, cap).await
    }
    async fn admit_media(&self, _: &reqwest::Url) -> Result<(), http::FetchError> {
      Err(http::FetchError::Address)
    }
  }
  let cases: serde_json::Value =
    serde_json::from_str(include_str!("../../tests/fixtures/reddit/posts.json")).unwrap();
  for case in cases.as_array().unwrap() {
    let transport = Refusing(FakeTransport(std::sync::Mutex::new(
      [(
        CANONICAL,
        reply(200, None, case["html"].as_str().unwrap().as_bytes()),
      )]
      .into(),
    )));
    assert_eq!(
      read_site(CANONICAL, &transport).await.resolution(CANONICAL),
      Resolution::Passthrough(CANONICAL.into())
    );
  }
}

#[tokio::test]
async fn reddit_does_not_answer_altered_challenges_or_repeat_a_challenge() {
  const BODY: &str = r#"<form method="get" action="/comments/abc123/"><input name="jsc_token" value="token"><input name="solution" value=""></form><script>const x=await(async e=>e+e)("abcdef12")</script>"#;
  for body in [
    BODY.replace("e+e", "e+f"),
    BODY.replace(
      "action=\"/comments/abc123/\"",
      "action=\"https://evil.example/comments/abc123/\"",
    ),
    BODY.replace("method=\"get\"", "method=\"post\""),
  ] {
    let transport = FakeTransport(std::sync::Mutex::new(
      [(CANONICAL, reply(200, None, body.as_bytes()))].into(),
    ));
    assert_eq!(
      read_site(CANONICAL, &transport).await.resolution(CANONICAL),
      Resolution::Passthrough(CANONICAL.into())
    );
  }
  let transport = FakeTransport(std::sync::Mutex::new(
    [
      (CANONICAL, reply(200, None, BODY.as_bytes())),
      (
        "https://www.reddit.com/comments/abc123/?jsc_token=token&solution=abcdef12abcdef12",
        reply(200, None, BODY.as_bytes()),
      ),
    ]
    .into(),
  ));
  assert_eq!(
    read_site(CANONICAL, &transport).await.resolution(CANONICAL),
    Resolution::Passthrough(CANONICAL.into())
  );
}

#[tokio::test]
async fn reddit_body_cap_preserves_the_login_fallback() {
  let transport = FakeTransport(std::sync::Mutex::new(
    [(
      CANONICAL,
      reply(200, None, &vec![b'x'; 4 * 1024 * 1024 + 1]),
    )]
    .into(),
  ));
  assert_eq!(
    read_site(CANONICAL, &transport).await.resolution(CANONICAL),
    Resolution::Passthrough(CANONICAL.into())
  );
}

#[test]
fn reddit_identities_reject_unsafe_and_non_post_urls() {
  for source in [
    "http://www.reddit.com/comments/abc123/",
    "https://user@www.reddit.com/comments/abc123/",
    "https://www.reddit.com:444/comments/abc123/",
    "https://www.reddit.com.evil.example/comments/abc123/",
    "https://www.reddit.com/r/test/",
    "https://www.reddit.com/user/test/",
  ] {
    assert!(!crate::resolve::reddit::is_post_url(source), "{source}");
  }
  // The fragment never reaches the server; share links end in it.
  assert!(crate::resolve::reddit::is_post_url(
    "https://www.reddit.com/r/test/comments/abc123/title/#lightbox"
  ));
}

#[tokio::test]
async fn reddit_only_bound_link_and_text_posts_prove_no_media() {
  for (kind, id, href, expected) in [
    ("customPost", "abc123", "", false),
    ("link", "abc123", "https://www.nytimes.com/article", true),
    ("link", "abc123", "https://www.reddit.com/r/games", false),
    (
      "link",
      "abc123",
      "https://www.reddit.com/gallery/abc123",
      false,
    ),
    (
      "link",
      "abc123",
      "https://cdn.example.org/clip.MP4?key=1",
      false,
    ),
    ("link", "abc123", "https://cdn.example.org/photo.jpg", false),
    ("text", "abc123", "", true),
    ("link", "abc123", "http://example.org/article", true),
    ("link", "abc123", "", true),
    ("link", "other123", "https://www.nytimes.com/article", false),
    ("unknown", "abc123", "", false),
  ] {
    let body = format!(
      "<shreddit-post id='t3_{id}' post-type='{kind}' content-href='{href}'></shreddit-post>"
    );
    let transport = FakeTransport(std::sync::Mutex::new(
      [(CANONICAL, reply(200, None, body.as_bytes()))].into(),
    ));
    let result = read_site(CANONICAL, &transport).await.resolution(CANONICAL);
    assert_eq!(
      result,
      if kind == "text" {
        Resolution::Unsupported {
          reason: Reason::TextOnly,
        }
      } else if kind == "link" && href == "https://www.nytimes.com/article" && expected {
        Resolution::Unsupported {
          reason: Reason::LinkOnly,
        }
      } else if expected {
        Resolution::Unsupported {
          reason: Reason::NoMedia,
        }
      } else {
        Resolution::Passthrough(CANONICAL.into())
      }
    );
  }
}

#[tokio::test]
async fn reddit_text_post_keeps_inline_body_images() {
  let body = b"<shreddit-post id='t3_abc123' post-type='text'><div slot='text-body'><img src='https://i.redd.it/one.jpg'><img src='https://i.redd.it/two.png'></div></shreddit-post>";
  let transport = FakeTransport(std::sync::Mutex::new(
    [(CANONICAL, reply(200, None, body))].into(),
  ));
  let Resolution::Attachments { metadata } =
    read_site(CANONICAL, &transport).await.resolution(CANONICAL)
  else {
    panic!("inline images must remain downloadable");
  };
  let media: serde_json::Value = serde_json::from_str(&metadata).unwrap();
  assert_eq!(
    media["attachments"]
      .as_array()
      .unwrap()
      .iter()
      .map(|i| i["url"].as_str().unwrap())
      .collect::<Vec<_>>(),
    ["https://i.redd.it/one.jpg", "https://i.redd.it/two.png"]
  );
}

#[tokio::test]
async fn reddit_link_posts_still_forward_known_media_hosts() {
  let body = b"<shreddit-post id='t3_abc123' post-type='link' content-href='https://www.youtube.com/watch?v=abc123'></shreddit-post>";
  let transport = FakeTransport(std::sync::Mutex::new(
    [(CANONICAL, reply(200, None, body))].into(),
  ));
  assert!(
    matches!(read_site(CANONICAL, &transport).await.resolution(CANONICAL),
    Resolution::PostMedia { url, .. } if url == "https://www.youtube.com/watch?v=abc123")
  );
}

#[tokio::test]
async fn reddit_media_wrapper_resolves_directly_and_after_share_redirects() {
  let share = "https://www.reddit.com/r/test/s/Share123/";
  let wrapper = "https://www.reddit.com/media?url=https%3A%2F%2Fi.redd.it%2Fphoto.jpg";
  for source in [wrapper, share] {
    let transport = FakeTransport(std::sync::Mutex::new(
      [(share, reply(302, Some(wrapper), b""))].into(),
    ));
    let result = read_site(source, &transport).await.resolution(source);
    let Resolution::Attachments { metadata } = result else {
      panic!("expected photo attachment, got {result:?}");
    };
    let media: crate::models::ParsedSingleVideo = serde_json::from_str(&metadata).unwrap();
    assert_eq!(media.attachments[0].url, "https://i.redd.it/photo.jpg");
    assert_eq!(media.title.as_deref(), Some("photo.jpg"));
  }
}

#[tokio::test]
async fn reddit_media_wrapper_keeps_the_host_and_public_address_gates() {
  for wrapper in [
    "https://www.reddit.com/media?url=https://evil.test/a.jpg",
    "https://www.reddit.com/media?url=https://i.redd.it.evil.test/a.jpg",
    "https://www.reddit.com/media?url=https://redd.it/a.jpg",
    "https://www.reddit.com/media?url=https://i.redd.it@evil.test/a.jpg",
    "https://www.reddit.com/media?url=http://i.redd.it/a.jpg",
    "https://www.reddit.com/media?url=https://i.redd.it:8443/a.jpg",
    "https://www.reddit.com/media?url=https://i.redd.it/a.jpg&url=https://evil.test/a.jpg",
    "https://www.reddit.com/media?url=https://i.redd.it/a.html",
    "https://reddit.com.evil.test/media?url=https://i.redd.it/a.jpg",
  ] {
    assert!(
      super::super::reddit::media_target(wrapper).is_none(),
      "{wrapper}"
    );
  }
  struct Refused;
  impl http::Transport for Refused {
    async fn admit_media(&self, _: &reqwest::Url) -> Result<(), http::FetchError> {
      Err(http::FetchError::InvalidUrl)
    }
    async fn get(&self, _: &reqwest::Url, _: usize) -> Result<http::Response, http::FetchError> {
      panic!("wrapper must not fetch before admitting the image")
    }
  }
  let wrapper = "https://www.reddit.com/media?url=https://preview.redd.it/a.png?width=1080";
  assert_eq!(
    read_site(wrapper, &Refused).await.resolution(wrapper),
    Resolution::Passthrough(wrapper.into())
  );
}

#[test]
fn reddit_media_redirect_is_recovered_from_ytdlp_errors_only_for_reddit_sources() {
  let wrapper = "https://www.reddit.com/media?url=https%3A%2F%2Fi.redd.it%2Fa.jpg";
  let stderr = format!("ERROR: Unsupported URL: {wrapper}");
  assert_eq!(
    reddit_media_redirect("https://reddit.com/r/test/s/Share123", &stderr).as_deref(),
    Some(wrapper)
  );
  assert_eq!(reddit_media_redirect("https://evil.test/a", &stderr), None);
  assert_eq!(
    reddit_media_redirect(
      "https://reddit.com/r/test/s/Share123",
      &stderr.replace("ERROR:", "WARNING:")
    ),
    None
  );
}

/// #68: the installed app logged nothing when the reader handed a post on to yt-dlp. The line is
/// written on drop (`ReadLog`); these are its fields.
#[tokio::test]
async fn reddit_reader_records_its_verdict_with_the_link_shape_only() {
  use std::sync::Mutex;
  struct Hanging;
  impl http::Transport for Hanging {
    async fn get(&self, _: &reqwest::Url, _: usize) -> Result<http::Response, http::FetchError> {
      std::future::pending().await
    }
  }
  const CHALLENGE: &str = r#"<form method="get" action="/r/test/comments/abc123/title/"><input name="jsc_token" value="token"><input name="solution" value=""></form><script>const x=await(async e=>e+f)("abcdef12")</script>"#;
  let permalink = "https://www.reddit.com/r/test/comments/abc123/title/";
  let lightbox = "https://www.reddit.com/r/test/comments/abc123/title/#lightbox";
  let share = "https://www.reddit.com/u/someone/s/Share123";
  let cases = [
    (
      lightbox,
      vec![(permalink, reply(200, None, b"<html>another layout</html>"))],
      ("parseNone", Some(200), 0, false),
    ),
    (
      lightbox,
      vec![(permalink, reply(200, None, CHALLENGE.as_bytes()))],
      ("challengeUnanswered", Some(200), 0, false),
    ),
    (
      share,
      vec![
        (
          "https://www.reddit.com/u/someone/s/Share123/",
          reply(301, Some(permalink), b""),
        ),
        (permalink, reply(403, None, b"")),
      ],
      ("status", Some(403), 1, false),
    ),
  ];
  for (source, replies, expected) in cases {
    let transport = FakeTransport(Mutex::new(replies.into()));
    let mut log = crate::resolve::reddit::ReadLog::new(source);
    let verdict = crate::resolve::reddit::read_logged(source, &transport, &mut log).await;
    assert!(matches!(
      verdict,
      Verdict::Verified(Resolution::Passthrough(_) | Resolution::Refused { .. })
    ));
    assert_eq!(
      (
        log.verdict,
        log.status,
        log.redirects,
        log.challenge_answered
      ),
      expected,
      "{source}"
    );
    for private in ["abc123", "title", "someone", "Share123", "lightbox"] {
      assert!(!log.link.contains(private), "{source}: {}", log.link);
    }
  }
  assert_eq!(
    crate::resolve::reddit::ReadLog::new(lightbox).link,
    "reddit.com/r/{id}/comments/{id}/{id}"
  );
  // A read the resolve timeout drops is logged as interrupted, not lost.
  let mut log = crate::resolve::reddit::ReadLog::new(lightbox);
  let limited = tokio::time::timeout(
    std::time::Duration::from_millis(20),
    crate::resolve::reddit::read_logged(lightbox, &Hanging, &mut log),
  )
  .await;
  assert!(limited.is_err());
  assert_eq!((log.verdict, log.status), ("interrupted", None));
  assert!(crate::resolve::reddit::log_skipped(
    "https://www.reddit.com/r/test/"
  ));
  assert!(!crate::resolve::reddit::log_skipped(
    "https://example.com/r/test/"
  ));
}

#[tokio::test]
async fn only_a_429_or_an_empty_403_marks_the_reader_refused() {
  let page: &[u8] = b"<html>private community</html>";
  for (status, body, refused) in [
    (403, &b""[..], true),
    (429, &b""[..], true),
    (429, page, true),
    (403, page, false),
    (404, &b""[..], false),
    (500, &b""[..], false),
  ] {
    let transport = FakeTransport(std::sync::Mutex::new(
      [(CANONICAL, reply(status, None, body))].into(),
    ));
    let expected = if refused {
      Resolution::Refused {
        url: CANONICAL.into(),
      }
    } else {
      Resolution::Passthrough(CANONICAL.into())
    };
    assert_eq!(
      read_site(CANONICAL, &transport).await.resolution(CANONICAL),
      expected,
      "{status} {}",
      body.len()
    );
  }
}

/// #68 end to end as far as no AppHandle allows: HTTP answer, reader verdict, recorded attempt,
/// terminal code. Not covered here: `run_attempts` calling the helper and the chain's stage loop.
#[tokio::test]
async fn empty_403_reaches_the_row_as_the_network_block_after_a_sign_in_failure() {
  use crate::extract::{chain::StageResult, outcome::*, precedence::terminal_code};
  let transport = FakeTransport(std::sync::Mutex::new(
    [(CANONICAL, reply(403, None, b""))].into(),
  ));
  let resolution = read_site(CANONICAL, &transport).await.resolution(CANONICAL);
  assert!(matches!(resolution, Resolution::Refused { .. }));
  let mut stage = StageResult::new(StageId::Ytdlp, Confidence::Medium);
  crate::runners::ytdlp_info::record_reader_refusal(&mut stage, std::time::Instant::now());
  stage.record_code("signInRequired", "", false, std::time::Instant::now());
  assert_eq!(terminal_code(&stage.attempts), Some("redditNetworkBlocked"));
}
