use crate::models::{parsed::PresetSizes, ParsedMedia, YtdlpInfo};
use crate::parsers::ytdlp_livestream::parse_livestream;
use crate::parsers::ytdlp_playlist::parse_playlist;
use crate::parsers::ytdlp_single::parse_single;
use serde_json::Value;
use std::collections::BTreeMap;

pub enum InfoType {
  Single,
  Playlist,
  Livestream,
}

/// yt-dlp prints nothing, or a playlist without entries, for a video its input filters exclude.
/// With filter arguments on the run, that emptiness is the filter's doing.
pub fn filtered_empty_code<'a>(stdout: &str, error: &'a str, filters_active: bool) -> &'a str {
  if filters_active && (stdout.trim().is_empty() || error == "playlistEmpty") {
    "rejectedByFilter"
  } else {
    error
  }
}

pub fn parse_ytdlp_info(json: &str, id: String) -> Result<ParsedMedia, String> {
  parse_ytdlp_info_for_url(json, id, None)
}

/// The original post URL remains authoritative when an extractor hands off to external media.
pub fn parse_ytdlp_info_for_url(
  json: &str,
  id: String,
  source: Option<&str>,
) -> Result<ParsedMedia, String> {
  let mut value: serde_json::Value =
    serde_json::from_str(json).map_err(|e| format!("JSON parse error: {e}"))?;
  let facebook = source
    .or_else(|| value["webpage_url"].as_str())
    .and_then(|url| reqwest::Url::parse(url).ok())
    .is_some_and(|url| {
      url
        .host_str()
        .is_some_and(|host| host == "facebook.com" || host.ends_with(".facebook.com"))
    });
  if facebook
    && value["title"]
      .as_str()
      .is_some_and(super::gallery::generic_facebook_title)
  {
    value["title"] = serde_json::Value::Null;
    let title = caption_title(&value);
    if !super::gallery::generic_facebook_title(&title) {
      value["title"] = serde_json::json!(title);
    }
  }
  let instagram = source
    .or_else(|| value["webpage_url"].as_str())
    .or_else(|| value["original_url"].as_str())
    .and_then(|url| reqwest::Url::parse(url).ok())
    .is_some_and(|url| {
      url
        .host_str()
        .is_some_and(|host| host == "instagram.com" || host.ends_with(".instagram.com"))
        && matches!(url.path().trim_matches('/').split('/').collect::<Vec<_>>().as_slice(),
          ["p" | "reel", code] | [_, "p" | "reel", code] if !code.is_empty())
    });
  if instagram {
    let title = caption_title(&value);
    if !title.is_empty() {
      value["title"] = serde_json::json!(title);
    }
    if value["description"]
      .as_str()
      .is_none_or(|caption| caption.trim().is_empty())
    {
      if let Some(username) = value["channel"]
        .as_str()
        .map(|name| name.trim().trim_start_matches('@'))
        .filter(|name| !name.is_empty())
      {
        if value["title"].as_str().is_none_or(|title| {
          title.trim().is_empty()
            || title == format!("Video by {username}")
            || title == format!("Post by {username}")
        }) {
          value["title"] = serde_json::json!(format!("Instagram post by @{username}"));
        }
      }
    }
  }
  let info: YtdlpInfo =
    serde_json::from_value(value.clone()).map_err(|e| format!("JSON parse error: {e}"))?;
  let embedded_in = embedding_page(source, &value);
  let source = source
    .or_else(|| value["webpage_url"].as_str())
    .or_else(|| value["original_url"].as_str());
  if !info.is_live.unwrap_or(false) && value["entries"].as_array().is_some_and(|e| !e.is_empty()) {
    if let Some(source) = source.filter(|s| super::post::single_post_url(s)) {
      let ParsedMedia::Single(parent) = parse_single(
        serde_json::from_value(value.clone()).map_err(|e| format!("JSON parse error: {e}"))?,
        id.clone(),
      ) else {
        return Err("Expected a post".into());
      };
      if let Some(post) = super::post::from_entries(&value, source, parent) {
        return Ok(post);
      }
    }
  }
  Ok(match detect_info_type(&info) {
    InfoType::Livestream => parse_livestream(info, id),
    InfoType::Playlist => {
      let media = parse_playlist(info, id);
      if matches!(&media, ParsedMedia::Playlist(list) if list.entries.is_empty()) {
        let account_feed = source
          .and_then(|s| reqwest::Url::parse(s).ok())
          .is_some_and(|url| {
            url
              .host_str()
              .is_some_and(|host| crate::sites::host_is(host, "youtube.com"))
              && url.path().starts_with("/feed/")
          });
        // These roots list albums/videos as page links that yt-dlp does not enumerate.
        let profile_root = source
          .and_then(|s| reqwest::Url::parse(s).ok())
          .is_some_and(|url| {
            let host = url.host_str().unwrap_or_default();
            (crate::sites::host_is(host, "bandcamp.com") && matches!(url.path(), "" | "/"))
              || (crate::sites::host_is(host, "rumble.com")
                && (url.path().starts_with("/user/") || url.path().starts_with("/c/")))
          });
        return Err(
          if account_feed {
            "signInRequired"
          } else if profile_root {
            "profileNotSupported"
          } else {
            "playlistEmpty"
          }
          .into(),
        );
      }
      media
    }
    InfoType::Single => {
      let mut media = parse_single(info, id);
      if let ParsedMedia::Single(single) = &mut media {
        single.media_kind = super::post::media_kind(&value);
        single.embedded_in = embedded_in;
        single.preset_sizes = preset_sizes(&value);
      }
      media
    }
  })
}

fn preset_sizes(info: &Value) -> Option<PresetSizes> {
  let formats = info["formats"].as_array()?;
  let positive = |value: &Value| value.as_f64().filter(|number| *number > 0.0);
  let valid_bytes =
    |size: f64| (size >= 1.0 && size < u64::MAX as f64).then_some(size.round() as u64);
  let stream_size = |id: &Value, audio: bool| {
    let id = id.as_str()?;
    let format = formats
      .iter()
      .find(|format| format["format_id"].as_str() == Some(id))?;
    let duration = || positive(&format["duration"]).or_else(|| positive(&info["duration"]));
    let from_kbps = |kbps: f64| valid_bytes(kbps * duration()? * 125.0);
    if audio {
      // Allow 1% for the container around the encoded audio.
      if let Some(target_size) = positive(&info["_nezzel_preset_audio_bitrate"])
        .and_then(|bps| valid_bytes(bps * duration()? / 8.0 * 1.01))
      {
        return Some(target_size);
      }
      if format["vcodec"].as_str() != Some("none") {
        // A muxed source's total size and bitrate include video.
        return positive(&format["abr"]).and_then(from_kbps);
      }
    }
    ["filesize", "filesize_approx"]
      .iter()
      .find_map(|key| format[*key].as_f64().and_then(valid_bytes))
      .or_else(|| {
        let audio_kbps = if audio {
          positive(&format["abr"])
        } else {
          None
        };
        audio_kbps
          .or_else(|| positive(&format["tbr"]))
          .and_then(from_kbps)
      })
  };
  let bytes: BTreeMap<_, _> = ["best", "small", "audio"]
    .into_iter()
    .filter_map(|preset| {
      let streams = info["_nezzel_preset_formats"][preset].as_array()?;
      let audio = preset == "audio";
      let max_streams = if audio { 1 } else { 2 };
      if !(1..=max_streams).contains(&streams.len()) {
        return None;
      }
      let size = streams.iter().try_fold(0_u64, |total, id| {
        total.checked_add(stream_size(id, audio)?)
      })?;
      Some((preset.to_owned(), size))
    })
    .collect();
  (!bytes.is_empty()).then(|| PresetSizes {
    bytes,
    output: info["_nezzel_preset_output"].as_array().cloned(),
  })
}

/// The pasted page only embeds another site's video (a film page with a YouTube trailer): yt-dlp's
/// generic extractor found the embed and handed it to that site's extractor, so the result may not be
/// the video the user meant. In yt-dlp's JSON `original_url` is the pasted link, `webpage_url` the
/// media's own page and `extractor_key` the extractor that read it. Returns the pasted page, so the UI
/// can offer to catch what that page itself plays.
///
/// Not an embed: the page's own extractor (`Generic` reading the page itself), any host of a site
/// Nezzel knows (`youtu.be`, a Reddit post linking YouTube: that site's extractor chose the media),
/// a link-shortener (`t.co/AbC12`), or the same registrable domain (`m.example.com`).
pub(crate) fn embedding_page(source: Option<&str>, raw: &serde_json::Value) -> Option<String> {
  let pasted = source.or_else(|| raw["original_url"].as_str())?;
  let page = reqwest::Url::parse(pasted).ok()?;
  let media = reqwest::Url::parse(raw["webpage_url"].as_str()?).ok()?;
  if !matches!(page.scheme(), "http" | "https")
    || raw["extractor_key"]
      .as_str()
      .is_none_or(|key| key.eq_ignore_ascii_case("generic"))
  {
    return None;
  }
  let (page_host, media_host) = (page.host_str()?, media.host_str()?);
  if crate::sites::site_of(page_host).is_some() || short_link(&page) {
    return None;
  }
  let domain = |host: &str| psl::domain_str(host).unwrap_or(host).to_ascii_lowercase();
  (domain(page_host) != domain(media_host)).then(|| pasted.to_owned())
}

/// A link-shortener's shape: a short domain and one short alphanumeric code (`bit.ly/3xYz`).
fn short_link(url: &reqwest::Url) -> bool {
  let domain = url.host_str().and_then(psl::domain_str).unwrap_or_default();
  let mut segments = url.path().split('/').filter(|s| !s.is_empty());
  domain.len() <= 11
    && matches!((segments.next(), segments.next()), (Some(code), None)
      if code.len() <= 16 && code.chars().all(|c| c.is_ascii_alphanumeric()))
}

pub fn detect_info_type(info: &YtdlpInfo) -> InfoType {
  if info.is_live.unwrap_or(false) {
    InfoType::Livestream
  } else if info.type_.as_deref() == Some("playlist")
    || info.entries.as_ref().is_some_and(|e| !e.is_empty())
  {
    InfoType::Playlist
  } else {
    InfoType::Single
  }
}

/// yt-dlp's generated social title is a fallback; a post's caption is more useful on the card.
pub(crate) fn caption_title(raw: &serde_json::Value) -> String {
  let title = raw["title"].as_str().unwrap_or("").trim();
  let generic = title.is_empty()
    || ["uploader", "uploader_id", "channel"].iter().any(|key| {
      raw[*key].as_str().is_some_and(|user| {
        title == format!("Video by {user}") || title == format!("Post by {user}")
      })
    })
    || title
      .strip_prefix("Video ")
      .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
  let noise =
    |c: char| c.is_whitespace() || matches!(c, '\u{200e}' | '\u{200f}' | '\u{061c}' | '\u{200b}');
  let caption = ["description", "caption"].iter().find_map(|key| {
    raw[*key]
      .as_str()?
      .lines()
      .map(|line| line.trim_matches(noise))
      .find(|line| {
        line.split_whitespace().any(|word| {
          let word = word.trim_matches(noise);
          !word.is_empty() && !word.starts_with('#') && !word.starts_with('@')
        })
      })
  });
  if generic {
    caption.unwrap_or(title).chars().take(300).collect()
  } else {
    raw["title"]
      .as_str()
      .unwrap_or("")
      .chars()
      .take(300)
      .collect()
  }
}

#[cfg(test)]
mod tests {
  #[test]
  fn empty_listings_fail_and_youtube_feeds_request_login() {
    for (source, code) in [
      ("https://www.youtube.com/feed/history", "signInRequired"),
      (
        "https://www.youtube.com/feed/subscriptions",
        "signInRequired",
      ),
      ("https://artist.bandcamp.com/", "profileNotSupported"),
      (
        "https://rumble.com/user/SpiderPlayers",
        "profileNotSupported",
      ),
      ("https://artist.bandcamp.com/album/none", "playlistEmpty"),
      (
        "https://youtube.com.evil.test/feed/history",
        "playlistEmpty",
      ),
    ] {
      for entries in [serde_json::json!([]), serde_json::json!([{}])] {
        let info = serde_json::json!({"_type":"playlist", "entries":entries});
        assert_eq!(
          parse_ytdlp_info_for_url(&info.to_string(), "row".into(), Some(source)).unwrap_err(),
          code,
          "{source}"
        );
      }
    }
  }

  use super::*;
  use serde_json::{json, Value};

  #[test]
  fn options_sheet_estimates_follow_only_the_streams_selected_for_each_preset() {
    let raw: serde_json::Value =
      serde_json::from_str(include_str!("fixtures/ytdlp-preset-sizes.json")).unwrap();
    for (preset, ids, expected) in [
      (
        "best",
        Some(serde_json::json!(["299", "140"])),
        Some(31_380_100),
      ),
      (
        "small",
        Some(serde_json::json!(["298", "140"])),
        Some(18_565_100),
      ),
      ("audio", Some(serde_json::json!(["140"])), Some(1_480_100)),
      ("best", Some(serde_json::json!(["18"])), Some(4_800_000)),
      ("best", Some(serde_json::json!(["299", "missing"])), None),
      ("best", Some(serde_json::json!(["sb0"])), None),
      ("best", None, None),
    ] {
      let mut dump = raw.clone();
      if let Some(ids) = ids {
        dump["_nezzel_preset_formats"][preset] = ids;
      } else {
        dump["_nezzel_preset_formats"]
          .as_object_mut()
          .unwrap()
          .remove(preset);
      }
      let ParsedMedia::Single(item) = parse_ytdlp_info(&dump.to_string(), "row".into()).unwrap()
      else {
        panic!("expected one video");
      };
      let wire = serde_json::to_value(item).unwrap();
      assert_eq!(wire["presetSizes"][preset].as_u64(), expected, "{preset}");
    }
  }

  #[test]
  fn options_sheet_audio_estimates_prefer_target_bitrate_and_fall_back_to_source() {
    let raw: Value =
      serde_json::from_str(include_str!("fixtures/ytdlp-preset-sizes.json")).unwrap();
    for (bitrate, duration, audio_ids, expected) in [
      (json!(192_000), json!(85), json!(["140"]), Some(2_060_400)),
      (json!(192_000), Value::Null, json!(["140"]), Some(1_480_100)),
      (Value::Null, json!(85), json!(["140"]), Some(1_480_100)),
      (Value::Null, json!(85), json!(["18"]), None),
      (Value::Null, json!(85), json!(["missing"]), None),
    ] {
      let mut dump = raw.clone();
      dump["duration"] = duration.clone();
      dump["_nezzel_preset_audio_bitrate"] = bitrate.clone();
      dump["_nezzel_preset_formats"]["audio"] = audio_ids.clone();
      let ParsedMedia::Single(item) = parse_ytdlp_info(&dump.to_string(), "row".into()).unwrap()
      else {
        panic!("expected one video");
      };
      let wire = serde_json::to_value(item).unwrap();
      assert_eq!(
        wire["presetSizes"]["audio"].as_u64(),
        expected,
        "bitrate {bitrate}, duration {duration}, source {audio_ids}"
      );
    }
  }

  #[test]
  fn empty_output_under_input_filters_is_a_filter_rejection() {
    let listed = json!({"_type":"playlist", "entries":[]}).to_string();
    let listing_error = parse_ytdlp_info_for_url(&listed, "row".into(), None).unwrap_err();
    assert_eq!(listing_error, "playlistEmpty");
    let silent_error = parse_ytdlp_info_for_url("", "row".into(), None).unwrap_err();
    for (stdout, error) in [
      ("", silent_error.as_str()),
      ("\n", silent_error.as_str()),
      (listed.as_str(), listing_error.as_str()),
    ] {
      assert_eq!(filtered_empty_code(stdout, error, true), "rejectedByFilter");
      assert_eq!(filtered_empty_code(stdout, error, false), error);
    }
    let video = json!({"id":"a", "title":"A", "url":"https://x.test/a.mp4"}).to_string();
    assert_eq!(
      filtered_empty_code(&video, "unableToExtractIdOrInfo", true),
      "unableToExtractIdOrInfo"
    );
  }

  #[test]
  fn media_found_on_another_site_than_the_pasted_page_is_marked_embedded() {
    let youtube = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
    let movie = "https://www.cinema-house.example/movies/the-long-night-2024/";
    let article = "https://news.example.org/2024/05/storm-video";
    for (pasted, webpage_url, extractor_key, embedded) in [
      // Generic found a trailer embedded in a film page and handed it to YouTube.
      (movie, youtube, "Youtube", true),
      // News articles embedding Vimeo or a post from X.
      (article, "https://vimeo.com/76979871", "Vimeo", true),
      (article, "https://x.com/user/status/123", "Twitter", true),
      // The media site's own short link, own page or a sister host: no hint.
      ("https://youtu.be/dQw4w9WgXcQ", youtube, "Youtube", false),
      (youtube, youtube, "Youtube", false),
      (
        "https://m.youtube.com/watch?v=dQw4w9WgXcQ",
        youtube,
        "Youtube",
        false,
      ),
      (
        "https://redd.it/abc123",
        "https://www.reddit.com/r/videos/comments/abc123/x/",
        "Reddit",
        false,
      ),
      (
        "https://fb.watch/abcDEF12/",
        "https://www.facebook.com/watch/?v=123",
        "Facebook",
        false,
      ),
      (
        "https://twitter.com/user/status/123",
        "https://x.com/user/status/123",
        "Twitter",
        false,
      ),
      // A known site's extractor chose the media (a Reddit post linking YouTube).
      (
        "https://www.reddit.com/r/videos/comments/abc/x/",
        youtube,
        "Youtube",
        false,
      ),
      // Link shorteners redirect to the media page.
      (
        "https://t.co/AbC123xyz",
        "https://x.com/user/status/123",
        "Twitter",
        false,
      ),
      ("https://bit.ly/3xYzAb", youtube, "Youtube", false),
      // The page's own video (generic read it) or a subdomain of the same site.
      (movie, movie, "Generic", false),
      (
        movie,
        "https://cdn.cinema-house.example/v/1",
        "Generic",
        false,
      ),
      (
        article,
        "https://video.news.example.org/v/1",
        "NewsPlayer",
        false,
      ),
      // Only web pages.
      ("ftp://files.example/v.mp4", youtube, "Youtube", false),
    ] {
      let info = json!({"id":"a", "title":"Trailer", "webpage_url":webpage_url, "original_url":pasted, "extractor_key":extractor_key, "formats":[]});
      let ParsedMedia::Single(media) =
        parse_ytdlp_info_for_url(&info.to_string(), "row".into(), Some(pasted)).unwrap()
      else {
        panic!("single expected");
      };
      assert_eq!(
        media.embedded_in.as_deref(),
        embedded.then_some(pasted),
        "{pasted} → {webpage_url}"
      );
    }
    // Without the pasted link, yt-dlp's `original_url` names the page.
    let info =
      json!({"id":"a", "webpage_url":youtube, "original_url":movie, "extractor_key":"Youtube"});
    assert_eq!(embedding_page(None, &info).as_deref(), Some(movie));
    // Facebook's own title fallback is unchanged, and Facebook is never an embed of itself.
    let info = json!({"id":"1", "title":"www.facebook.com", "description":"Caption", "webpage_url":"https://www.facebook.com/watch/?v=1", "extractor_key":"Facebook"});
    let ParsedMedia::Single(media) = parse_ytdlp_info_for_url(
      &info.to_string(),
      "row".into(),
      Some("https://www.facebook.com/user/videos/1/"),
    )
    .unwrap() else {
      panic!("single expected");
    };
    assert_eq!(
      (media.title.as_deref(), media.embedded_in),
      (Some("Caption"), None)
    );
  }

  fn playlist(source: &str) -> Value {
    json!({"_type":"playlist", "webpage_url":source, "title":"Generating 3D with AI is easy…", "entries":[
      {"id":"clip-a", "url":"https://storage.example/video-generator-usage.mp4", "ext":"mp4", "title":"video-generator-usage", "thumbnail":"https://storage.example/a.jpg"},
      {"id":"clip-b", "url":"https://storage.example/Rodin.webm", "ext":"webm", "title":"Rodin", "thumbnail":"https://storage.example/b.jpg"}
    ]})
  }

  #[test]
  fn facebook_generic_titles_use_captions_or_allow_the_filename_fallback() {
    for title in [
      "www.facebook.com",
      "https://www.facebook.com/photo/?fbid=123",
    ] {
      for description in [Some("Photo caption"), None] {
        let raw = json!({"id":"123", "title":title, "description":description, "url":"https://cdn.example/photo.jpg", "ext":"jpg"});
        let ParsedMedia::Single(media) = parse_ytdlp_info_for_url(
          &raw.to_string(),
          "row".into(),
          Some("https://www.facebook.com/photo/?fbid=123"),
        )
        .unwrap() else {
          panic!("single expected")
        };
        assert_eq!(media.title.as_deref(), description);
      }
    }
  }

  #[test]
  fn instagram_ytdlp_fallback_titles_use_the_handle_without_replacing_captions() {
    for (title, description, expected) in [
      (None, None, "Instagram post by @creator"),
      (Some("Video by creator"), None, "Instagram post by @creator"),
      (
        Some("Post by creator"),
        Some(""),
        "Instagram post by @creator",
      ),
      (Some("A real title"), None, "A real title"),
      (Some("Video by creator"), Some("A caption"), "A caption"),
    ] {
      let info = json!({"webpage_url":"https://www.instagram.com/p/abc/", "channel":"creator", "uploader":"Display Name", "title":title, "description":description});
      let ParsedMedia::Single(media) = parse_ytdlp_info(&info.to_string(), "row".into()).unwrap()
      else {
        panic!("single post expected");
      };
      assert_eq!(media.title.as_deref(), Some(expected));
    }
    for (url, expected) in [
      ("https://www.instagram.com/creator/reel/abc/", "A caption"),
      ("https://www.instagram.com/creator/", "Video by creator"),
    ] {
      let info = json!({"channel":"creator", "title":"Video by creator", "description":"A caption\nMore details"});
      let ParsedMedia::Single(media) =
        parse_ytdlp_info_for_url(&info.to_string(), "row".into(), Some(url)).unwrap()
      else {
        panic!("single post expected");
      };
      assert_eq!(media.title.as_deref(), Some(expected));
    }
  }

  #[test]
  fn single_post_entries_become_one_selectable_post() {
    for source in [
      "https://www.reddit.com/comments/abc123/",
      "https://www.reddit.com/r/test/s/Share123",
      "https://x.com/user/status/123",
      "https://www.threads.com/@user/post/abc",
      "https://www.facebook.com/user/posts/123",
      "https://user.tumblr.com/post/123/title",
      "https://bsky.app/profile/user.test/post/abc",
    ] {
      let media = parse_ytdlp_info(&playlist(source).to_string(), "row".into()).unwrap();
      let ParsedMedia::Single(post) = media else {
        panic!("post expected: {source}");
      };
      assert_eq!(post.attachments.len(), 2);
    }
  }

  #[test]
  fn collection_urls_keep_playlist_range_semantics() {
    for source in [
      "https://www.youtube.com/playlist?list=PL123",
      "https://www.reddit.com/r/test/",
      "https://x.com/user",
      "https://user.tumblr.com/",
      "https://bsky.app/profile/user.test",
    ] {
      assert!(
        matches!(
          parse_ytdlp_info(&playlist(source).to_string(), "row".into()).unwrap(),
          ParsedMedia::Playlist(_)
        ),
        "{source}"
      );
    }
  }

  #[test]
  fn post_entries_keep_parent_title_and_their_own_thumbnails() {
    let ParsedMedia::Single(post) = parse_ytdlp_info(
      &playlist("https://www.reddit.com/comments/abc123/").to_string(),
      "row".into(),
    )
    .unwrap() else {
      panic!("post expected");
    };
    assert_eq!(
      post
        .attachments
        .iter()
        .map(|a| (a.alt.as_deref(), a.thumbnail.as_deref()))
        .collect::<Vec<_>>(),
      vec![
        (
          Some("Generating 3D with AI is easy… (1/2)"),
          Some("https://storage.example/a.jpg")
        ),
        (
          Some("Generating 3D with AI is easy… (2/2)"),
          Some("https://storage.example/b.jpg")
        )
      ]
    );
  }

  #[test]
  fn ytdlp_post_entries_drop_non_https_thumbnails() {
    let source = "https://www.reddit.com/comments/abc123/";
    let mut info = playlist(source);
    info["thumbnail"] = json!("data:image/png;base64,fixture");
    info["entries"][0]["thumbnail"] = json!("http://cdn.example/cover.jpg");
    let ParsedMedia::Single(post) = parse_ytdlp_info(&info.to_string(), "row".into()).unwrap()
    else {
      panic!("post expected");
    };
    assert_eq!(
      (post.thumbnail, post.attachments[0].thumbnail.clone()),
      (None, None)
    );
  }

  #[test]
  fn bilibili_http_thumbnail_is_emitted_as_https() {
    // Recorded Bilibili thumbnail: the hdslb.com CDN is https-capable but yt-dlp reports http.
    let thumb = "http://i2.hdslb.com/bfs/archive/49267ce20bc246be6304bf369a3ded0256854c23.jpg";
    let mut info = json!({"id":"BV1", "title":"clip", "url":"https://cdn.example/v.mp4", "ext":"mp4", "thumbnail":thumb});
    let single = parse_ytdlp_info(&info.to_string(), "row".into()).unwrap();
    info["_type"] = json!("playlist");
    info["webpage_url"] = json!("https://www.bilibili.com/video/BV1");
    info["entries"] = json!([{"id":"a", "url":"https://cdn.example/a.mp4", "thumbnail":thumb}]);
    let playlist = parse_ytdlp_info(&info.to_string(), "row".into()).unwrap();
    let expected = thumb.replacen("http://", "https://", 1);
    assert_eq!(
      serde_json::to_value(&single).unwrap()["thumbnail"],
      expected
    );
    assert_eq!(
      serde_json::to_value(&playlist).unwrap()["entries"][0]["thumbnail"],
      expected
    );
  }

  #[test]
  fn direct_video_containers_without_codecs_are_video_but_explicit_audio_stays_audio() {
    for (ext, codec, expected) in [
      ("mp4", None, "video"),
      ("webm", None, "video"),
      ("mov", Some("unknown"), "video"),
      ("mp4", Some("none"), "audio"),
      ("m4a", None, "audio"),
      ("mp3", None, "audio"),
      ("opus", None, "audio"),
    ] {
      let value =
        json!({"url":format!("https://cdn.example/clip.{ext}"), "ext":ext, "vcodec":codec});
      let media = parse_ytdlp_info(&value.to_string(), "row".into()).unwrap();
      assert_eq!(
        serde_json::to_value(media).unwrap()["mediaKind"],
        expected,
        "{ext}/{codec:?}"
      );
    }
  }
}
