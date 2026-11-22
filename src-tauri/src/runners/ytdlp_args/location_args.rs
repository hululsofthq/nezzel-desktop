use crate::models::TrackType;
use crate::runners::template_context::TemplateContext;
use crate::state::config_models::OutputSettings;
use crate::state::preferences_models::PathPreferences;
use std::path::{Path, PathBuf};

/// Folder inside the system Downloads folder where Nezzel saves by default (same as the Android app).
pub const DEFAULT_DOWNLOAD_SUBDIR: &str = "Nezzel";

/// Platform Downloads folder, or the conventional home-relative path when unavailable.
pub fn downloads_dir(downloads: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
  downloads.or_else(|| home.map(|home| home.join("Downloads")))
}

/// Default save folder; yt-dlp creates it on the first download.
pub fn default_download_dir() -> PathBuf {
  default_download_dir_with(dirs::download_dir, dirs::home_dir, || {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
  })
}

pub(super) fn default_download_dir_with(
  downloads: impl FnOnce() -> Option<PathBuf>,
  home: impl FnOnce() -> Option<PathBuf>,
  fallback: impl FnOnce() -> PathBuf,
) -> PathBuf {
  downloads_dir(downloads(), home())
    .unwrap_or_else(fallback)
    .join(DEFAULT_DOWNLOAD_SUBDIR)
}

/// Folder a download of `track_type` is saved under (before the directory template and file name).
pub fn output_base_dir(
  track_type: &TrackType,
  output_settings: &OutputSettings,
  path_preferences: &PathPreferences,
  fallback_dir: PathBuf,
) -> PathBuf {
  let global_dir = output_settings
    .download_dir
    .as_ref()
    .map(PathBuf::from)
    .unwrap_or(fallback_dir);
  let preferred_dir = match track_type {
    TrackType::Both | TrackType::Video => path_preferences.video_download_dir.as_ref(),
    TrackType::Audio => path_preferences.audio_download_dir.as_ref(),
  };
  preferred_dir.map(PathBuf::from).unwrap_or(global_dir)
}

/// Layout value of "No subfolders": distinct from the empty default, which means one folder per group.
pub const NO_SUBFOLDERS: &str = ".";

/// Longest group folder name, in UTF-8 bytes: leaves room for the file name inside Windows' path limit.
const MAX_GROUP_DIR_BYTES: usize = 100;

/// Folder name for a link that yields several files; `None` for one file or a layout other than the default.
pub fn group_dir_name(context: &TemplateContext) -> Option<String> {
  let files: usize = context.values.get("nezzel_group_files")?.parse().ok()?;
  if files < 2 {
    return None;
  }
  let title = context
    .values
    .get("nezzel_group_title")
    .map(String::as_str)
    .unwrap_or_default();
  // `%` would be read as a yt-dlp template field, path separators as nesting.
  let cleaned: String = title
    .chars()
    .map(|c| {
      if matches!(c, '%' | '/' | '\\') {
        '_'
      } else {
        c
      }
    })
    .collect();
  let mut name = crate::files::sanitize::sanitize_file_name(&cleaned);
  if name.len() > MAX_GROUP_DIR_BYTES {
    let mut end = MAX_GROUP_DIR_BYTES;
    while !name.is_char_boundary(end) {
      end -= 1;
    }
    name = crate::files::sanitize::sanitize_file_name(&name[..end]);
  }
  Some(name)
}

/// A post that yt-dlp counted as one file turned out to hold `files` items (`None` when not yet
/// known: the gallery listing fills it in). Gives the context the group's title and count; a context
/// that already counts 2+ files is left alone.
pub fn widen_group(
  context: &mut TemplateContext,
  files: Option<usize>,
  title: Option<&str>,
  url: &str,
) {
  let counted: usize = context
    .values
    .get("nezzel_group_files")
    .and_then(|files| files.parse().ok())
    .unwrap_or(0);
  if counted >= 2 {
    return;
  }
  let fallback = || {
    ["playlist_title", "enriched_title"]
      .iter()
      .find_map(|key| {
        context
          .values
          .get(*key)
          .filter(|value| !value.trim().is_empty())
          .cloned()
      })
      .or_else(|| {
        reqwest::Url::parse(url).ok().and_then(|url| {
          url
            .host_str()
            .map(|host| host.trim_start_matches("www.").to_owned())
        })
      })
      .unwrap_or_default()
  };
  let title = title.map(str::to_owned).unwrap_or_else(fallback);
  context
    .values
    .entry("nezzel_group_title".into())
    .or_insert(title);
  if let Some(files) = files.filter(|files| *files >= 2) {
    context
      .values
      .insert("nezzel_group_files".into(), files.to_string());
  }
}

/// Directory under the download folder: the chosen layout, or by default the group folder.
fn directory_layout(layout: &str, context: &TemplateContext) -> PathBuf {
  match layout {
    NO_SUBFOLDERS => PathBuf::new(),
    "" => group_dir_name(context)
      .map(PathBuf::from)
      .unwrap_or_default(),
    chosen => PathBuf::from(chosen),
  }
}

pub fn build_location_args(
  track_type: &TrackType,
  template_context: &TemplateContext,
  output_settings: &OutputSettings,
  path_preferences: &PathPreferences,
  fallback_dir: PathBuf,
) -> Vec<String> {
  let (prefix_dir, filename) = match track_type {
    TrackType::Both | TrackType::Video => (
      path_preferences.video_directory_template.as_str(),
      output_settings.file_name_template.as_str(),
    ),
    TrackType::Audio => (
      path_preferences.audio_directory_template.as_str(),
      output_settings.audio_file_name_template.as_str(),
    ),
  };
  // Upgrade only the old shipped defaults; leave custom templates alone.
  let filename = match filename {
    "%(title).200s-(%(height)sp%(fps).0d).%(ext)s"
    | "%(title).200B-(%(height)sp%(fps).0d).%(ext)s" => {
      if template_context
        .values
        .get("media_kind")
        .map(String::as_str)
        == Some("image")
      {
        if template_context
          .values
          .get("height")
          .is_some_and(|h| !h.is_empty())
        {
          "%(title).200B-(%(height)sp).%(ext)s"
        } else {
          "%(title).200B.%(ext)s"
        }
      } else {
        match template_context.values.get("height") {
          Some(height) if height.is_empty() || height == "0" => "%(title).200B.%(ext)s",
          None => "%(title).200B%(nezzel_quality|)s.%(ext)s",
          _ => "%(title).200B-(%(height)sp%(fps|).0d).%(ext)s",
        }
      }
    }
    "%(title).200s-(%(abr)dk).%(ext)s" => "%(title).200B-(%(abr)dk).%(ext)s",
    custom => custom,
  };
  let base_dir = output_base_dir(track_type, output_settings, path_preferences, fallback_dir);
  let output_path = base_dir
    .join(directory_layout(prefix_dir, template_context))
    .join(filename);
  let rendered_output = template_context.render_template(output_path.to_string_lossy().as_ref());

  vec!["-o".into(), rendered_output]
}

/// Run after format selection, before yt-dlp prepares any filenames.
pub fn filename_metadata_args(source: &str, context: &TemplateContext) -> Vec<String> {
  let mut args = vec![
    "--parse-metadata".into(),
    "video:-(%(height|)sp%(fps|).0d):%(nezzel_quality)s".into(),
    "--replace-in-metadata".into(),
    "video:nezzel_quality".into(),
    r"^-\((?:p.*|0p.*)\)$".into(),
    "".into(),
  ];
  let host = reqwest::Url::parse(source)
    .ok()
    .and_then(|url| url.host_str().map(str::to_owned));
  let instagram = host
    .as_deref()
    .is_some_and(|host| host == "instagram.com" || host.ends_with(".instagram.com"));
  let facebook = host.as_deref().is_some_and(|host| {
    host == "facebook.com" || host.ends_with(".facebook.com") || host == "fb.watch"
  });
  let title = context
    .values
    .get("enriched_title")
    .map(|title| title.chars()
      .filter(|c| !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}'..='\u{200f}'))
      .collect::<String>());
  if let Some(title) = title.filter(|title| (instagram || facebook) && !title.trim().is_empty()) {
    args.extend([
      "--replace-in-metadata".into(),
      "video:title,fulltitle".into(),
      if instagram {
        r"(?i)^\s*(?:Video by .+|Post by .+|Video \d+|instagram(?:-\w+)? video #\S+)?\s*$"
      } else {
        r"(?i)^\s*(?:Facebook|(?:facebook(?:-\w+)?|generic) video #\S+|(?:www\.)?facebook\.com|https?://\S+|فيديو)?\s*$"
      }
      .into(),
      title.replace('\\', "\\\\"),
    ]);
  }
  args
}

/// A folder picked for one download wins over the per-track (video/audio) folders from Preferences.
pub fn path_preferences_for(
  path_preferences: &PathPreferences,
  download_dir_override: Option<&str>,
) -> PathPreferences {
  let mut prefs = path_preferences.clone();
  if download_dir_override.is_some_and(|dir| !dir.is_empty()) {
    prefs.video_download_dir = None;
    prefs.audio_download_dir = None;
  }
  prefs
}

/// Absolute -o bypasses yt-dlp's temporary path. Keep the same final destination relative to its
/// filesystem anchor, so nested/custom templates still expand in -o, while parts stay private.
pub struct TemporaryLocation {
  pub args: Vec<String>,
  pub anchor: PathBuf,
}

pub fn temporary_location_args(output: &str, temporary: &Path) -> TemporaryLocation {
  let path = Path::new(output);
  let absolute = if path.is_absolute() {
    path.to_path_buf()
  } else {
    std::env::current_dir()
      .unwrap_or_else(|_| PathBuf::from("."))
      .join(path)
  };
  let mut normalized = PathBuf::new();
  for part in absolute.components() {
    match part {
      std::path::Component::ParentDir => {
        normalized.pop();
      }
      std::path::Component::CurDir => {}
      _ => normalized.push(part.as_os_str()),
    }
  }
  let anchor = normalized.ancestors().last().unwrap().to_path_buf();
  let relative = normalized.strip_prefix(&anchor).unwrap();
  let args = vec![
    "--paths".into(),
    format!("home:{}", temporary.join("output").display()),
    "--paths".into(),
    format!("temp:{}", temporary.join("parts").display()),
    "-o".into(),
    relative.to_string_lossy().into_owned(),
  ];
  TemporaryLocation { args, anchor }
}
