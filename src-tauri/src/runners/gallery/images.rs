//! Image downloads: transfer, verification and destination naming.
use super::runner::GalleryRunner;
use crate::models::TrackType;
use crate::parsers::gallery::{
  gallery_deadline, stderr_failure, verify_receipt, GalleryAsset, GalleryError, GalleryPost,
};
use crate::runners::{
  gallery_files::{image_type, Staging},
  gallery_process::execute_monitored,
  ytdlp_runner::YtdlpRunner,
};
use crate::scheduling::{
  download_pipeline::DownloadEntry,
  group_state::{is_group_running, subscribe_group},
};
use serde_json::json;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

impl GalleryRunner {
  pub(super) async fn images(
    &self,
    post: &GalleryPost,
    assets: &[&GalleryAsset],
    group: &str,
    stage: &Path,
    progressed: impl FnMut() + Send,
  ) -> Result<Vec<Result<PathBuf, GalleryError>>, GalleryError> {
    if assets.is_empty() {
      return Ok(Vec::new());
    }
    let mut input = String::new();
    for asset in assets {
      let directory = stage.join(asset.index.to_string());
      std::fs::create_dir_all(&directory).map_err(|_| GalleryError::Invalid)?;
      // Per-URL input options isolate filenames and receipts in a single process.
      // JSON encoding prevents a source URL or directory from injecting input directives.
      input.push_str(&format!(
        "-base-directory={}\n-extractor.directlink.transform={}\nhttps://nezzel.invalid/attachment.jpg\n",
        json!(directory),
        json!([[
          r"^https://nezzel\.invalid/attachment\.jpg$",
          asset.item.url.replace('\\', r"\\")
        ]])
      ));
    }
    let input_path = stage.join("images.txt");
    std::fs::write(&input_path, input).map_err(|_| GalleryError::Invalid)?;
    let args = transfer_args(&self.args, &post.source, stage, &input_path);
    if let Some(cookies) = &self._cookies {
      cookies
        .check_live_consent()
        .map_err(GalleryError::Refused)?;
    }
    let output = execute_monitored(
      &self.path,
      &args,
      subscribe_group(group),
      gallery_deadline(assets.len()),
      Some(stage),
      || {
        if let Some(cookies) = &self._cookies {
          cookies.announce_used();
        }
      },
      progressed,
    )
    .await?;
    let failure =
      (output.code != 0).then(|| stderr_failure(&String::from_utf8_lossy(&output.stderr), true));
    Ok(
      assets
        .iter()
        .map(|asset| {
          verified_image(post, asset, group, &stage.join(asset.index.to_string()))
            .map_err(|error| failure.clone().unwrap_or(error))
        })
        .collect(),
    )
  }
}

/// One gallery-dl process transfers every selected image of a post.
fn transfer_args(base: &[String], referer: &str, stage: &Path, input: &Path) -> Vec<String> {
  let mut args = base.to_vec();
  args.extend([
    "-o".into(),
    format!("downloader.http.headers={}", json!({"Referer": referer})),
    "-o".into(),
    "downloader.http.filesize-max=134217728".into(),
    // CDNs such as Flickr's refuse bursts with 429: one retry after 15 s (also retries 5xx and
    // timeouts; 2 x 20 s timeout + 1 s stays under the 60 s stall watch). The later root
    // `--retries` overrides the shared `--retries 0`.
    "--retries".into(),
    "1".into(),
    "-o".into(),
    "downloader.http.retry-codes=[429]".into(),
    "-o".into(),
    "downloader.http.sleep-429=15".into(),
    // Warnings, not just errors, so a final 429 reaches `stderr_failure`; no "[n/N] url" lines.
    "--warning".into(),
    "-o".into(),
    "output.progress=false".into(),
    "--range".into(),
    "1".into(),
    "--child-filter".into(),
    "False".into(),
    "--directory".into(),
    stage.to_string_lossy().into_owned(),
    "--filename".into(),
    "asset.{extension}".into(),
    "-o".into(),
    "extractor.metadata-url=nezzel_url".into(),
    "--write-metadata".into(),
    "--postprocessor-option".into(),
    "filename=receipt.json".into(),
    "--input-file".into(),
    input.to_string_lossy().into_owned(),
  ]);
  args
}

pub(super) fn verified_image(
  post: &GalleryPost,
  asset: &GalleryAsset,
  group: &str,
  stage: &Path,
) -> Result<PathBuf, GalleryError> {
  let receipt = stage.join("receipt.json");
  let metadata = std::fs::symlink_metadata(&receipt).map_err(|_| GalleryError::Invalid)?;
  if !metadata.is_file() || metadata.len() > 1024 * 1024 {
    return Err(GalleryError::Invalid);
  }
  verify_receipt(
    &std::fs::read(receipt).map_err(|_| GalleryError::Invalid)?,
    post,
    asset,
  )?;
  let mut files = std::fs::read_dir(stage)
    .map_err(|_| GalleryError::Invalid)?
    .filter_map(Result::ok)
    .filter(|entry| {
      entry.file_name().to_string_lossy().starts_with("asset.")
        && entry.file_type().is_ok_and(|kind| kind.is_file())
        && !entry.file_name().to_string_lossy().ends_with(".part")
    });
  let path = files
    .next()
    .ok_or(GalleryError::Refused("imageSourceUnavailable"))?
    .path();
  if files.next().is_some() || !is_group_running(group) {
    return Err(GalleryError::Invalid);
  }
  Ok(path)
}

pub(super) async fn filename(
  app: &AppHandle,
  entry: &DownloadEntry,
  asset: &GalleryAsset,
  bytes: &[u8],
  multiple: bool,
  post: &GalleryPost,
) -> Result<PathBuf, GalleryError> {
  let extension = image_type(bytes)?;
  // Both gallery-dl and yt-dlp photo posts use this destination renderer. Parent video
  // dimensions/frame rates must not leak into the selected photo's name.
  let mut context = entry.template_context.clone();
  context.insert("media_kind", "image");
  context.insert(
    "height",
    asset
      .item
      .height
      .filter(|h| *h > 0)
      .map(|h| h.to_string())
      .unwrap_or_default(),
  );
  context.insert("fps", "");
  let stage = Staging::new()?;
  let info = stage.json("image.json", &json!({"id":asset.item.id,"title":post.title.as_deref().unwrap_or(&asset.item.id),"uploader":post.uploader,"webpage_url":post.source,"url":asset.item.url,"ext":extension,"width":asset.item.width,"height":asset.item.height,
    "formats":[{"format_id":"image","url":asset.item.url,"ext":extension,"protocol":"https","vcodec":"unknown","acodec":"none"}]}))?;
  let output = YtdlpRunner::new(app)
    .with_location_args(&TrackType::Video, &context, entry.overrides.as_ref())
    .with_args([
      "--skip-download".into(),
      "--simulate".into(),
      "--print".into(),
      "filename".into(),
      "--load-info-json".into(),
      info.to_string_lossy().into_owned(),
    ])
    .output(&entry.group_id)
    .await
    .map_err(|_| GalleryError::Invalid)?
    .ok_or(GalleryError::Cancelled)?;
  if output.code != 0 || output.stdout.len() > 32768 {
    return Err(GalleryError::Invalid);
  }
  let rendered = std::str::from_utf8(&output.stdout)
    .map_err(|_| GalleryError::Invalid)?
    .trim_end_matches(['\r', '\n']);
  if rendered.is_empty() || rendered.contains(['\r', '\n']) {
    return Err(GalleryError::Invalid);
  }
  image_destination(PathBuf::from(rendered), asset.index, multiple, extension)
}

/// The same photo saved flat before groups got their own folder: reused instead of saved a second time.
pub(super) fn flat_copy_before_group_folders(
  destination: &Path,
  group_dir: Option<&str>,
  bytes: &[u8],
) -> Option<PathBuf> {
  let parent = destination.parent()?;
  if parent.file_name()?.to_str()? != group_dir? {
    return None;
  }
  let flat = parent.parent()?.join(destination.file_name()?);
  // A regular file inside the folder itself: a symlink to an identical file elsewhere is not "saved here".
  if !std::fs::symlink_metadata(&flat).ok()?.is_file() {
    return None;
  }
  if !flat
    .canonicalize()
    .ok()?
    .starts_with(parent.parent()?.canonicalize().ok()?)
  {
    return None;
  }
  (std::fs::read(&flat).ok()? == bytes).then_some(flat)
}

pub(super) fn image_destination(
  path: PathBuf,
  index: usize,
  multiple: bool,
  extension: &str,
) -> Result<PathBuf, GalleryError> {
  if !path.is_absolute() || path.file_name().is_none() {
    return Err(GalleryError::Invalid);
  }
  let path = if multiple {
    path.with_file_name(format!(
      "{}-{:03}.{}",
      path.file_stem().unwrap().to_string_lossy(),
      index,
      extension
    ))
  } else {
    path.with_extension(extension)
  };
  let name = path
    .file_name()
    .ok_or(GalleryError::Invalid)?
    .to_string_lossy();
  let name = if crate::files::sanitize::is_windows_reserved(&name) {
    format!("_{name}")
  } else {
    name.into_owned()
  };
  Ok(path.with_file_name(crate::files::sanitize::limit_media_name(&name)))
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Real `-v` stderr of a Flickr batch (#68; 2 of 30 originals refused with 429), trimmed.
  const FLICKR_429_VERBOSE: &str = "\
[gallery-dl][debug] Starting DownloadJob for 'https://nezzel.invalid/attachment.jpg'
[directlink][debug] Using DirectlinkExtractor for 'https://nezzel.invalid/attachment.jpg'
[directlink][debug] Active postprocessor modules: [MetadataPP]
[urllib3.connectionpool][debug] https://live.staticflickr.com:443 \"GET /65535/55504534895_5878517e78_o.jpg HTTP/1.1\" 429 None
[downloader.http][warning] '429 Too Many Requests' for 'https://live.staticflickr.com/65535/55504534895_5878517e78_o.jpg'
[download][error] Failed to download asset.jpg
[28/30] https://nezzel.invalid/attachment.jpg
[gallery-dl][debug] Starting DownloadJob for 'https://nezzel.invalid/attachment.jpg'
[urllib3.connectionpool][debug] Resetting dropped connection: live.staticflickr.com
[urllib3.connectionpool][debug] https://live.staticflickr.com:443 \"GET /65535/55504256113_302e6cb0a4_o.jpg HTTP/1.1\" 429 None
[downloader.http][warning] '429 Too Many Requests' for 'https://live.staticflickr.com/65535/55504256113_302e6cb0a4_o.jpg'
[download][error] Failed to download asset.jpg
[29/30] https://nezzel.invalid/attachment.jpg";

  /// gallery-dl prints a `[logger][level]` line only at or above the level its argv asks for.
  fn printed_at_level(args: &[String], stderr: &str) -> String {
    let rank = |level: &str| {
      ["debug", "info", "warning", "error"]
        .iter()
        .position(|l| *l == level)
    };
    let min = args
      .iter()
      .rev()
      .find_map(|arg| match arg.as_str() {
        "-q" | "--quiet" => Some(3),
        "-w" | "--warning" => Some(2),
        "-v" | "--verbose" => Some(0),
        _ => None,
      })
      .unwrap_or(1);
    stderr
      .lines()
      .filter(|line| {
        line
          .strip_prefix('[')
          .and_then(|line| line.split_once("]["))
          .and_then(|(_, rest)| rest.split_once(']'))
          .and_then(|(level, _)| rank(level))
          .is_some_and(|level| level >= min)
      })
      .collect::<Vec<_>>()
      .join("\n")
  }

  #[test]
  fn flat_photo_from_before_group_folders_is_reused_only_for_identical_bytes() {
    let dir = std::env::temp_dir().join(format!("nezzel-flat-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = {
      let mut b = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
      b.extend_from_slice(&[0; 32]);
      b
    };
    std::fs::write(dir.join("Trip-001.png"), &png).unwrap();
    let grouped = dir.join("Trip/Trip-001.png");
    assert_eq!(
      flat_copy_before_group_folders(&grouped, Some("Trip"), &png),
      Some(dir.join("Trip-001.png"))
    );
    assert_eq!(
      flat_copy_before_group_folders(&grouped, Some("Trip"), b"x"),
      None
    );
    assert_eq!(flat_copy_before_group_folders(&grouped, None, &png), None);
    #[cfg(unix)]
    {
      let outside = std::env::temp_dir().join(format!("nezzel-out-{}", uuid::Uuid::new_v4()));
      std::fs::write(&outside, &png).unwrap();
      let linked = dir.join("Link-001.png");
      std::os::unix::fs::symlink(&outside, &linked).unwrap();
      assert_eq!(
        flat_copy_before_group_folders(&dir.join("Trip/Link-001.png"), Some("Trip"), &png),
        None
      );
      std::fs::remove_file(&outside).unwrap();
    }
    std::fs::remove_dir_all(&dir).unwrap();
  }

  #[test]
  fn image_step_retries_cdn_429s_and_keeps_the_warning_that_names_them() {
    let base = super::super::runner::arguments(&[], Path::new("/config.json"));
    let args = transfer_args(
      &base,
      "https://www.flickr.com/photos/gsfc/",
      Path::new("/stage"),
      Path::new("/stage/images.txt"),
    );
    assert_eq!(
      stderr_failure(&printed_at_level(&args, FLICKR_429_VERBOSE), true),
      GalleryError::Refused("rateLimited429")
    );
    let at = |pair: [&str; 2]| {
      args
        .windows(2)
        .rposition(|w| w[0] == pair[0] && w[1] == pair[1])
    };
    // `--retries` is a root option and gallery-dl's root value beats downloader.http.*, so the
    // image step's later `--retries 1` must follow the shared `--retries 0`.
    assert!(at(["--retries", "1"]) > at(["--retries", "0"]), "{args:?}");
    assert!(
      at(["-o", "downloader.http.retry-codes=[429]"]).is_some(),
      "{args:?}"
    );
    assert!(
      at(["-o", "downloader.http.sleep-429=15"]).is_some(),
      "{args:?}"
    );
    assert!(at(["-o", "output.progress=false"]).is_some(), "{args:?}");
    assert!(
      !args.iter().any(|arg| arg == "--quiet" || arg == "-q"),
      "{args:?}"
    );
  }

  #[test]
  fn image_names_fit_200_bytes_with_quality_and_carousel_suffix() {
    let path = std::env::temp_dir().join(format!("{}-(1080p).jpg", "صورة".repeat(40)));
    let result = image_destination(path, 3, true, "jpg").unwrap();
    let name = result.file_name().unwrap().to_str().unwrap();
    assert!(name.len() <= 200);
    assert!(name.ends_with("-(1080p)-003.jpg"));
  }

  #[test]
  fn image_naming_preserves_resolved_folders_and_original_carousel_position() {
    let folder = std::env::temp_dir().join("Nezzel/صور");
    let path = image_destination(folder.join("photo.custom"), 3, true, "jpg").unwrap();
    assert_eq!(path, folder.join("photo-003.jpg"));
    assert!(image_destination(PathBuf::from("relative.jpg"), 1, false, "jpg").is_err());
    for stem in [
      "CON",
      "PrN",
      "AUX",
      "nul",
      "COM1",
      "com9",
      "LPT1",
      "lpt9",
      "CON.photo",
    ] {
      assert_eq!(
        image_destination(folder.join(format!("{stem}.old")), 1, false, "jpg").unwrap(),
        folder.join(format!("_{stem}.jpg"))
      );
    }
    // Bug 5 fixed: the shared files::sanitize check also reserves these.
    for stem in [
      "COM0",
      "LPT0",
      "CONIN$",
      "CONOUT$",
      "COM\u{b9}",
      "LPT\u{b3}",
    ] {
      assert_eq!(
        image_destination(folder.join(format!("{stem}.old")), 1, false, "jpg").unwrap(),
        folder.join(format!("_{stem}.jpg"))
      );
    }
    for stem in ["COM10", "LPT10", "CONSOLE"] {
      assert_eq!(
        image_destination(folder.join(format!("{stem}.old")), 1, false, "jpg").unwrap(),
        folder.join(format!("{stem}.jpg"))
      );
    }
  }
}
