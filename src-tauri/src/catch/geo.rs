//! Whether the page in front is a site's "not available in your country" page, from what the
//! top frame shows: its address, the HTTP status of its document, and the start of its text
//! (the hook sends text only for a short page, `catch-hooks.ts` `geoReader`). The bar then says so
//! instead of "Nothing found", and suggests a VPN without naming any.

use regex::Regex;
use std::sync::LazyLock;
use tauri::Url;

/// HTTP 451 Unavailable For Legal Reasons.
const LEGAL: u16 = 451;

/// A host's first label or, on a short page, a path segment that geo-block pages use.
const MARKS: &[&str] = &[
  "geoblock",
  "geoblocked",
  "geo-block",
  "geo-blocked",
  "geo_block",
  "geoblocking",
  "georestricted",
  "geo-restricted",
  "geo-restriction",
  "region-unavailable",
  "country-unavailable",
  "not-available-in-your-country",
  "not-available-in-your-region",
  "unavailable-in-your-country",
  "unavailable-in-your-region",
];

static TEXT: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(concat!(
    r"(?i)",
    // English: "isn't (currently) available in your area", "not available in your country".
    // Not "may not be available in your region": that is the small print of ordinary pages.
    r"(?:not|n't)\s+(?:currently\s+|yet\s+)?(?:available|offered|accessible)\s+",
    r"(?:in|from|to\s+users\s+in)\s+your\s+(?:area|country|region|location|territory|market)",
    r"|unavailable\s+in\s+your\s+(?:area|country|region|location|territory)",
    r"|blocked\s+in\s+your\s+(?:country|region)",
    // A sentence about the service ("is only available in the US"), not an offer's label.
    r"|(?:is|are)\s+only\s+available\s+in\s+(?:the\s+)?(?:us|u\.s\.|usa|united\s+states|uk|united\s+kingdom)\b",
    // French.
    r"|pas\s+(?:encore\s+)?disponibles?\s+dans\s+votre\s+(?:pays|r[ée]gion|zone)",
    r"|indisponibles?\s+dans\s+votre\s+(?:pays|r[ée]gion|zone)",
    // Spanish, Portuguese, German.
    r"|no\s+est[áa]\s+disponible\s+en\s+(?:tu|su)\s+(?:pa[íi]s|regi[óo]n|zona)",
    r"|n[ãa]o\s+est[áa]\s+dispon[íi]vel\s+(?:no\s+seu\s+pa[íi]s|na\s+sua\s+regi[ãa]o)",
    r"|in\s+(?:deinem|ihrem)\s+land\s+nicht\s+verf[üu]gbar",
    // Arabic.
    r"|غير\s+(?:متاح|متاحة|متوفر|متوفرة)\s+في\s+(?:بلدك|منطقتك|دولتك|بلدك الحالي)",
  ))
  .expect("geo text pattern")
});

/// `status`: the top document's HTTP status when the engine tells it. `text`: the page's title
/// and the start of its text, empty for any page that is not short. An article or a video page
/// about geo-blocking is long, so its address and its words say nothing here.
pub(super) fn blocked(url: &Url, status: Option<u16>, text: &str) -> bool {
  if status == Some(LEGAL) {
    return true;
  }
  let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
  let first = host.split('.').next().unwrap_or_default();
  let marked = |part: &str| MARKS.iter().any(|mark| part.eq_ignore_ascii_case(mark));
  if marked(first) {
    return true;
  }
  if text.is_empty() {
    return false;
  }
  if url
    .path_segments()
    .is_some_and(|mut segments| segments.any(marked))
  {
    return true;
  }
  // Curly apostrophes ("isn’t") read as straight ones.
  let text = text.replace(['\u{2019}', '\u{2018}'], "'");
  TEXT.is_match(&text)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn geo_block_pages_are_told_from_ordinary_ones() {
    let tubi = "We're Sorry. Tubi is not currently available in your area.";
    for (url, status, text, expected) in [
      // The owner's tubi.tv case: redirected to gdpr.tubi.tv with this text.
      ("https://gdpr.tubi.tv/", None, tubi, true),
      ("https://tubitv.com/movies/1", Some(451), "", true),
      ("https://example.com/geoblock", None, "Sorry", true),
      (
        "https://www.example.com/en/geo-blocked/",
        None,
        "Sorry",
        true,
      ),
      ("https://geoblock.example.com/", None, "", true),
      (
        "https://example.com/watch",
        None,
        "This video is not available in your country",
        true,
      ),
      (
        "https://example.com/",
        None,
        "Sorry, this content isn’t available in your region.",
        true,
      ),
      (
        "https://example.com/",
        None,
        "This service is unavailable in your location",
        true,
      ),
      (
        "https://example.com/",
        None,
        "Our service is only available in the US.",
        true,
      ),
      (
        "https://example.fr/",
        None,
        "Ce contenu n'est pas disponible dans votre pays",
        true,
      ),
      (
        "https://example.es/",
        None,
        "Este video no está disponible en tu país",
        true,
      ),
      (
        "https://example.de/",
        None,
        "Dieses Video ist in deinem Land nicht verfügbar",
        true,
      ),
      (
        "https://example.sa/",
        None,
        "عذرًا، هذا المحتوى غير متاح في بلدك",
        true,
      ),
      (
        "https://example.com.br/",
        None,
        "Este conteúdo não está disponível no seu país",
        true,
      ),
      // Ordinary pages and near misses.
      // A long page (no text sent) whose address only mentions the subject.
      ("https://example.com/tag/geoblocking/video", None, "", false),
      ("https://example.com/geoblock", None, "", false),
      // Small print and offers on a short page.
      (
        "https://example.com/",
        None,
        "Some titles may not be available in your region. Terms apply.",
        false,
      ),
      (
        "https://example.com/",
        None,
        "Free shipping only available in the US",
        false,
      ),
      (
        "https://example.com/watch",
        Some(200),
        "Watch the trailer now",
        false,
      ),
      (
        "https://example.com/watch",
        Some(404),
        "Page not found",
        false,
      ),
      (
        "https://gdpr.example.com/",
        None,
        "Manage your cookie choices",
        false,
      ),
      ("https://example.com/geography/maps", None, "", false),
      (
        "https://example.com/",
        None,
        "Now available in your area: same-day delivery",
        false,
      ),
      (
        "https://example.com/",
        None,
        "Available in the US and Canada",
        false,
      ),
      ("https://example.com/", None, "", false),
    ] {
      let parsed = Url::parse(url).unwrap();
      assert_eq!(
        blocked(&parsed, status, text),
        expected,
        "{url} {status:?} {text}"
      );
    }
  }
}
