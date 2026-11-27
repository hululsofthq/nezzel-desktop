//! A page whose own address names one video that Nezzel's normal download path reads: the bar
//! offers that address at once, whatever the player streams. YouTube feeds its player through
//! MediaSource from requests no download can reuse, so its video never shows up as a stream.
//! Another site joins with one arm below, for page shapes that always hold exactly one video.

use crate::sites::SiteId;
use tauri::Url;

pub(super) struct PageVideo {
  /// The site's own picture of the video, for the queued row.
  pub(super) picture: Option<Url>,
  /// The yt-dlp extractors that read this page (`--use-extractors`).
  pub(super) extractors: &'static str,
}

pub(super) fn page_video(url: &Url) -> Option<PageVideo> {
  if !super::state::is_http(url) {
    return None;
  }
  // The same gate the main window's link field uses: a site the normal path reads, not its feed.
  crate::capture::supported_url(url.as_str())?;
  if crate::capture::is_feed_page(url.as_str()) {
    return None;
  }
  let host = crate::net::host(url)?
    .trim_end_matches('.')
    .to_ascii_lowercase();
  let segments: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
  match crate::sites::site_by_domain(&host)? {
    SiteId::YouTube => {
      let id = match segments[..] {
        ["watch"] => url
          .query_pairs()
          .find(|(key, _)| key == "v")
          .map(|(_, id)| id.into_owned())?,
        ["shorts", id] => id.to_owned(),
        _ => return None,
      };
      youtube_id(&id).then(|| PageVideo {
        picture: Url::parse(&format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg")).ok(),
        extractors: "youtube",
      })
    }
    _ => None,
  }
}

fn youtube_id(id: &str) -> bool {
  id.len() == 11
    && id
      .bytes()
      .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn only_a_supported_single_video_address_is_offered() {
    let cases = [
      ("https://www.youtube.com/shorts/ynQcf3PSsyI", true),
      (
        "https://m.youtube.com/shorts/ynQcf3PSsyI?feature=share",
        true,
      ),
      ("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42s", true),
      ("https://music.youtube.com/watch?v=dQw4w9WgXcQ", true),
      // Channels, feeds, lists and odd ids are not one video.
      ("https://www.youtube.com/@SomeChannel", false),
      ("https://www.youtube.com/@SomeChannel/shorts", false),
      ("https://www.youtube.com/", false),
      ("https://www.youtube.com/feed/subscriptions", false),
      ("https://www.youtube.com/playlist?list=PL123", false),
      ("https://www.youtube.com/watch", false),
      ("https://www.youtube.com/watch?v=short", false),
      ("https://www.youtube.com/shorts/ynQcf3PSsyI/extra", false),
      ("https://www.youtube.com/shorts/ynQcf3PSsy%22", false),
      // Lookalike hosts and sites without a page shape here.
      ("https://notyoutube.com/watch?v=dQw4w9WgXcQ", false),
      (
        "https://youtube.com.evil.example/watch?v=dQw4w9WgXcQ",
        false,
      ),
      // Sign-in parts, odd ports and look-alike letters name another place.
      (
        "https://www.youtube.com@evil.example/watch?v=dQw4w9WgXcQ",
        false,
      ),
      (
        "https://user:pass@www.youtube.com/watch?v=dQw4w9WgXcQ",
        false,
      ),
      ("https://www.youtube.com:8443/watch?v=dQw4w9WgXcQ", false),
      ("https://www.xn--youtub-5ua.com/watch?v=dQw4w9WgXcQ", false),
      ("https://www.youtub\u{e9}.com/watch?v=dQw4w9WgXcQ", false),
      ("ftp://www.youtube.com/watch?v=dQw4w9WgXcQ", false),
      // Pages of the site that are no single watch page here.
      ("https://www.youtube.com/results?search_query=cats", false),
      (
        "https://www.youtube.com/results?search_query=cats&v=dQw4w9WgXcQ",
        false,
      ),
      ("https://www.youtube.com/embed/dQw4w9WgXcQ", false),
      ("https://www.youtube.com/live/dQw4w9WgXcQ", false),
      ("https://www.youtube.com/watch/?v=dQw4w9WgXcQ", true),
      ("https://www.youtube.com./watch?v=dQw4w9WgXcQ", false),
      ("https://vimeo.com/76979871", false),
      ("https://example.com/watch?v=dQw4w9WgXcQ", false),
      ("about:blank", false),
    ];
    for (raw, offered) in cases {
      let url = Url::parse(raw).unwrap();
      assert_eq!(page_video(&url).is_some(), offered, "{raw}");
    }
    let picture = page_video(&Url::parse("https://www.youtube.com/shorts/ynQcf3PSsyI").unwrap())
      .and_then(|video| video.picture)
      .unwrap();
    assert_eq!(
      picture.as_str(),
      "https://i.ytimg.com/vi/ynQcf3PSsyI/hqdefault.jpg"
    );
  }
}
