use crate::parsers::gallery::GalleryError;
use std::{
  io::{Read, Write},
  path::{Path, PathBuf},
};

static LIVE_STAGING: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<PathBuf>>> =
  std::sync::LazyLock::new(Default::default);
pub struct Staging {
  pub path: PathBuf,
}
impl Staging {
  pub fn new() -> Result<Self, GalleryError> {
    Self::in_root(&std::env::temp_dir())
  }
  pub fn in_root(root: &Path) -> Result<Self, GalleryError> {
    std::fs::create_dir_all(root).map_err(|_| GalleryError::Invalid)?;
    if std::fs::symlink_metadata(root)
      .map_err(|_| GalleryError::Invalid)?
      .file_type()
      .is_symlink()
    {
      return Err(GalleryError::Invalid);
    }
    let mut live = LIVE_STAGING.lock().unwrap_or_else(|e| e.into_inner());
    let path = root
      .canonicalize()
      .map_err(|_| GalleryError::Invalid)?
      .join(format!(".nezzel-images-{}", uuid::Uuid::new_v4()));
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
      use std::os::unix::fs::DirBuilderExt;
      builder.mode(0o700);
    }
    builder.create(&path).map_err(|_| GalleryError::Invalid)?;
    live.insert(path.clone());
    Ok(Self { path })
  }
  pub fn json(&self, filename: &str, value: &serde_json::Value) -> Result<PathBuf, GalleryError> {
    let path = self.path.join(filename);
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
      use std::os::unix::fs::OpenOptionsExt;
      options.mode(0o600);
    }
    let mut file = options.open(&path).map_err(|_| GalleryError::Invalid)?;
    serde_json::to_writer(&mut file, value).map_err(|_| GalleryError::Invalid)?;
    Ok(path)
  }
}
impl Drop for Staging {
  fn drop(&mut self) {
    let mut live = LIVE_STAGING.lock().unwrap_or_else(|e| e.into_inner());
    let _ = std::fs::remove_dir_all(&self.path);
    live.remove(&self.path);
  }
}

/// Startup only: skip current-session staging, foreign names, and symlink roots/entries.
pub(crate) fn sweep_image_staging(roots: &[PathBuf]) {
  let live = LIVE_STAGING.lock().unwrap_or_else(|e| e.into_inner());
  for root in roots {
    if !std::fs::symlink_metadata(root).is_ok_and(|m| m.file_type().is_dir()) {
      continue;
    }
    let Ok(root) = root.canonicalize() else {
      continue;
    };
    let Ok(entries) = std::fs::read_dir(root) else {
      continue;
    };
    for entry in entries.flatten() {
      let path = entry.path();
      let valid = entry
        .file_name()
        .to_str()
        .and_then(|s| s.strip_prefix(".nezzel-images-"))
        .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok());
      if !valid || live.contains(&path) || !entry.file_type().is_ok_and(|t| t.is_dir()) {
        continue;
      }
      #[cfg(unix)]
      {
        use std::os::unix::fs::MetadataExt;
        if !entry
          .metadata()
          .is_ok_and(|m| m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0)
        {
          continue;
        }
      }
      let _ = std::fs::remove_dir_all(path);
    }
  }
}

const STALE_COPY_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// `.nezzel-copy-<uuid>.part` (cross-volume publish) and `.nezzel-<uuid>.part` (gallery copy).
/// The direct-file engine's `<name>.nezzel-part` resume files never match: they have no UUID.
fn is_copy_part_name(name: &str) -> bool {
  name
    .strip_prefix(".nezzel-copy-")
    .or_else(|| name.strip_prefix(".nezzel-"))
    .and_then(|s| s.strip_suffix(".part"))
    .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok())
}

/// Startup only: remove partial-copy files a crash left directly inside the given folders.
/// Regular files only (no symlinks), own files on unix, older than 24 hours. No recursion.
pub(crate) fn sweep_stale_copy_parts(roots: &[PathBuf]) {
  sweep_stale_copy_parts_at(roots, std::time::SystemTime::now());
}

fn sweep_stale_copy_parts_at(roots: &[PathBuf], now: std::time::SystemTime) {
  for root in roots {
    if !std::fs::symlink_metadata(root).is_ok_and(|m| m.file_type().is_dir()) {
      continue;
    }
    let Ok(root) = root.canonicalize() else {
      continue;
    };
    let Ok(entries) = std::fs::read_dir(root) else {
      continue;
    };
    for entry in entries.flatten() {
      if !entry.file_name().to_str().is_some_and(is_copy_part_name) {
        continue;
      }
      let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
        continue;
      };
      if !meta.file_type().is_file() {
        continue;
      }
      #[cfg(unix)]
      {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != unsafe { libc::geteuid() } {
          continue;
        }
      }
      let old = meta
        .modified()
        .ok()
        .and_then(|m| now.duration_since(m).ok())
        .is_some_and(|age| age >= STALE_COPY_AGE);
      if old {
        let _ = std::fs::remove_file(entry.path());
      }
    }
  }
}

pub fn image_type(data: &[u8]) -> Result<&'static str, GalleryError> {
  if data.len() > 128 * 1024 * 1024 {
    return Err(GalleryError::Invalid);
  }
  if data.starts_with(b"\x89PNG\r\n\x1a\n") {
    let mut offset = 8;
    let mut header = false;
    let mut pixels = false;
    while offset + 12 <= data.len() {
      let size = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
      let end = offset
        .checked_add(12 + size)
        .filter(|end| *end <= data.len())
        .ok_or(GalleryError::Invalid)?;
      let kind = &data[offset + 4..offset + 8];
      let expected = u32::from_be_bytes(data[end - 4..end].try_into().unwrap());
      let mut crc = !0_u32;
      for byte in &data[offset + 4..end - 4] {
        crc ^= *byte as u32;
        for _ in 0..8 {
          crc = (crc >> 1) ^ (0xedb88320 & (0_u32.wrapping_sub(crc & 1)));
        }
      }
      if !crc != expected {
        return Err(GalleryError::Invalid);
      }
      if kind == b"IHDR" {
        if header
          || offset != 8
          || size != 13
          || data[offset + 8..offset + 12] == [0; 4]
          || data[offset + 12..offset + 16] == [0; 4]
        {
          return Err(GalleryError::Invalid);
        }
        header = true;
      }
      if kind == b"IDAT" && size > 0 {
        pixels = true;
      }
      if kind == b"IEND" {
        return if header && pixels && size == 0 && end == data.len() {
          Ok("png")
        } else {
          Err(GalleryError::Invalid)
        };
      }
      offset = end;
    }
  } else if data.starts_with(&[0xff, 0xd8]) && data.ends_with(&[0xff, 0xd9]) {
    let mut offset = 2;
    while offset + 4 <= data.len() && data[offset] == 0xff {
      let marker = data[offset + 1];
      let size = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
      if size < 2 || offset + 2 + size > data.len() {
        break;
      }
      if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf)
        && size >= 8
        && data[offset + 5..offset + 7] != [0, 0]
        && data[offset + 7..offset + 9] != [0, 0]
      {
        return Ok("jpg");
      }
      offset += size + 2;
    }
  } else if data.len() > 20
    && data.starts_with(b"RIFF")
    && &data[8..12] == b"WEBP"
    && u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize + 8 == data.len()
    && matches!(&data[12..16], b"VP8 " | b"VP8L" | b"VP8X")
  {
    return Ok("webp");
  } else if data.len() > 13
    && (data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a"))
    && data.last() == Some(&0x3b)
    && data[6..8] != [0, 0]
    && data[8..10] != [0, 0]
  {
    return Ok("gif");
  } else if data.len() >= 54
    && data.starts_with(b"BM")
    && u32::from_le_bytes(data[2..6].try_into().unwrap()) as usize == data.len()
  {
    return Ok("bmp");
  } else if data.len() > 32 && &data[4..8] == b"ftyp" {
    let ftyp = u32::from_be_bytes(data[..4].try_into().unwrap()) as usize;
    if ftyp >= 16 && ftyp <= data.len() && data[ftyp..].windows(4).any(|v| v == b"mdat") {
      if data[8..ftyp]
        .as_chunks::<4>()
        .0
        .iter()
        .any(|v| v == b"avif" || v == b"avis")
      {
        return Ok("avif");
      }
      if data[8..ftyp]
        .as_chunks::<4>()
        .0
        .iter()
        .any(|v| matches!(v, b"heic" | b"heix" | b"mif1"))
      {
        return Ok("heic");
      }
    }
  }
  Err(GalleryError::Invalid)
}

pub fn read_image(path: &Path) -> Result<Vec<u8>, GalleryError> {
  let metadata = std::fs::symlink_metadata(path).map_err(|_| GalleryError::Invalid)?;
  if !metadata.is_file() || metadata.len() > 128 * 1024 * 1024 {
    return Err(GalleryError::Invalid);
  }
  let mut bytes = Vec::new();
  std::fs::File::open(path)
    .map_err(|_| GalleryError::Invalid)?
    .take(128 * 1024 * 1024 + 1)
    .read_to_end(&mut bytes)
    .map_err(|_| GalleryError::Invalid)?;
  image_type(&bytes)?;
  Ok(bytes)
}

/// "Download again": the next free `<stem>-NNN.<ext>` name (the scheme saved videos use) when `destination` is taken.
pub fn next_free_name(destination: &Path) -> PathBuf {
  let (Some(stem), Some(parent)) = (destination.file_stem(), destination.parent()) else {
    return destination.to_path_buf();
  };
  let extension = destination
    .extension()
    .map(|e| format!(".{}", e.to_string_lossy()))
    .unwrap_or_default();
  let mut path = destination.to_path_buf();
  for number in 1..10_000 {
    if std::fs::symlink_metadata(&path).is_err() {
      break;
    }
    path = parent.join(format!("{}-{number:03}{extension}", stem.to_string_lossy()));
  }
  path
}

/// "Download again": saves a new copy under the next free numbered name and returns its path. A name taken
/// between choosing and saving (a second "Download again" running) is skipped, never reported as reused.
pub fn finalize_fresh(
  source: &Path,
  destination: &Path,
  cancelled: impl Fn() -> bool,
) -> Result<PathBuf, GalleryError> {
  finalize_fresh_with(source, destination, cancelled, next_free_name)
}

fn finalize_fresh_with(
  source: &Path,
  destination: &Path,
  cancelled: impl Fn() -> bool,
  first: impl FnOnce(&Path) -> PathBuf,
) -> Result<PathBuf, GalleryError> {
  let mut candidate = first(destination);
  for _ in 0..100 {
    match finalize(source, &candidate, &cancelled) {
      Ok(false) => return Ok(candidate),
      Ok(true) | Err(GalleryError::Refused("imageFileConflict")) => {
        candidate = next_free_name(destination);
      }
      Err(error) => return Err(error),
    }
  }
  Err(GalleryError::Refused("imageFileConflict"))
}

pub fn finalize(
  source: &Path,
  destination: &Path,
  cancelled: impl Fn() -> bool,
) -> Result<bool, GalleryError> {
  finalize_with_link(source, destination, cancelled, |from, to| {
    std::fs::hard_link(from, to)
  })
}

fn finalize_with_link(
  source: &Path,
  destination: &Path,
  cancelled: impl Fn() -> bool,
  link: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<bool, GalleryError> {
  if cancelled() {
    return Err(GalleryError::Cancelled);
  }
  let parent = destination.parent().ok_or(GalleryError::Invalid)?;
  std::fs::create_dir_all(parent)
    .map_err(|error| GalleryError::Refused(crate::files::save_error_code(&error)))?;
  let temp = parent.join(format!(".nezzel-{}.part", uuid::Uuid::new_v4()));
  let result = (|| {
    let bytes = read_image(source)?;
    let mut file = std::fs::OpenOptions::new()
      .write(true)
      .create_new(true)
      .open(&temp)
      .map_err(|error| GalleryError::Refused(crate::files::save_error_code(&error)))?;
    file
      .write_all(&bytes)
      .and_then(|_| file.sync_all())
      .map_err(|error| GalleryError::Refused(crate::files::save_error_code(&error)))?;
    if cancelled() {
      return Err(GalleryError::Cancelled);
    }
    // An atomic create without replacement: an existing file/symlink is never overwritten.
    match link(&temp, destination) {
      Ok(()) => Ok(false),
      Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
        // A retry can recognize its own completed file without changing any existing bytes.
        if std::fs::symlink_metadata(destination)
          .is_ok_and(|meta| meta.is_file() && meta.len() == bytes.len() as u64)
          && read_image(destination).is_ok_and(|previous| previous == bytes)
        {
          Ok(true)
        } else {
          Err(GalleryError::Refused("imageFileConflict"))
        }
      }
      Err(_) => {
        // FAT/exFAT and some network shares have no hard-link support. Claim
        // the final name exclusively; never copy into an existing file.
        let mut final_file = match std::fs::OpenOptions::new()
          .write(true)
          .create_new(true)
          .open(destination)
        {
          Ok(file) => file,
          Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return if std::fs::symlink_metadata(destination)
              .is_ok_and(|m| m.is_file() && m.len() == bytes.len() as u64)
              && read_image(destination).is_ok_and(|previous| previous == bytes)
            {
              Ok(true)
            } else {
              Err(GalleryError::Refused("imageFileConflict"))
            };
          }
          Err(error) => return Err(GalleryError::Refused(crate::files::save_error_code(&error))),
        };
        let copied = (|| {
          for chunk in bytes.chunks(64 * 1024) {
            if cancelled() {
              return Err(GalleryError::Cancelled);
            }
            final_file
              .write_all(chunk)
              .map_err(|error| GalleryError::Refused(crate::files::save_error_code(&error)))?;
          }
          final_file
            .sync_all()
            .map_err(|error| GalleryError::Refused(crate::files::save_error_code(&error)))?;
          if cancelled() {
            return Err(GalleryError::Cancelled);
          }
          Ok(false)
        })();
        drop(final_file);
        if copied.is_err() {
          let _ = std::fs::remove_file(destination);
        }
        copied
      }
    }
  })();
  let _ = std::fs::remove_file(&temp);
  result
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn startup_sweep_removes_only_old_partial_copy_files() {
    let root = Staging::new().unwrap();
    let id = uuid::Uuid::new_v4();
    let a = root.path.join(format!(".nezzel-copy-{id}.part"));
    let b = root
      .path
      .join(format!(".nezzel-{}.part", uuid::Uuid::new_v4()));
    let keep_names = [
      ".nezzel-copy-notauuid.part".to_string(),
      format!(".nezzel-copy-{id}.part.bak"),
      "movie.mp4.nezzel-part".to_string(),
      ".nezzel-part".to_string(),
    ];
    for f in [&a, &b] {
      std::fs::write(f, b"x").unwrap();
    }
    for n in &keep_names {
      std::fs::write(root.path.join(n), b"x").unwrap();
    }
    let dir = root
      .path
      .join(format!(".nezzel-{}.part", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    #[cfg(unix)]
    let link = {
      let link = root
        .path
        .join(format!(".nezzel-copy-{}.part", uuid::Uuid::new_v4()));
      std::os::unix::fs::symlink(&keep_names[2], &link).unwrap();
      link
    };
    let roots = std::slice::from_ref(&root.path);
    let now = std::time::SystemTime::now();
    sweep_stale_copy_parts_at(roots, now + std::time::Duration::from_secs(3600));
    assert!(a.exists() && b.exists(), "fresh files are kept");
    sweep_stale_copy_parts_at(roots, now + std::time::Duration::from_secs(25 * 3600));
    assert!(!a.exists() && !b.exists());
    for n in &keep_names {
      assert!(root.path.join(n).exists(), "{n}");
    }
    assert!(dir.is_dir());
    #[cfg(unix)]
    assert!(std::fs::symlink_metadata(&link).is_ok());
  }

  #[test]
  fn gallery_staging_is_hidden() {
    let root = Staging::new().unwrap();
    let stage = Staging::in_root(&root.path).unwrap();
    assert!(stage
      .path
      .file_name()
      .unwrap()
      .to_str()
      .unwrap()
      .starts_with(".nezzel-images-"));
  }

  #[test]
  fn gallery_startup_sweep_removes_crash_orphans_and_preserves_live_and_foreign_paths() {
    let root = Staging::new().unwrap();
    let live = Staging::in_root(&root.path).unwrap();
    let orphan = root
      .path
      .join(format!(".nezzel-images-{}", uuid::Uuid::new_v4()));
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
      use std::os::unix::fs::DirBuilderExt;
      builder.mode(0o700);
    }
    builder.create(&orphan).unwrap();
    std::fs::write(orphan.join("images.txt"), b"abandoned").unwrap();
    let foreign = root.path.join(".nezzel-images-user-folder");
    std::fs::create_dir(&foreign).unwrap();
    #[cfg(unix)]
    {
      let link = root
        .path
        .join(format!(".nezzel-images-{}", uuid::Uuid::new_v4()));
      std::os::unix::fs::symlink(&foreign, &link).unwrap();
      let root_link = root.path.join("linked-root");
      std::os::unix::fs::symlink(&root.path, &root_link).unwrap();
      sweep_image_staging(&[root_link]);
      assert!(orphan.exists());
    }
    sweep_image_staging(std::slice::from_ref(&root.path));
    assert!(!orphan.exists());
    assert!(live.path.exists());
    assert!(foreign.exists());
    #[cfg(unix)]
    assert_eq!(std::fs::read_dir(&root.path).unwrap().count(), 4);
  }
  #[test]
  fn image_staging_cleans_success_failure_and_cancellation() {
    for outcome in ["success", "failure", "cancelled"] {
      let root = Staging::new().unwrap();
      let destination = root.path.join("final.png");
      {
        let stage = Staging::in_root(&root.path).unwrap();
        std::fs::create_dir(stage.path.join("1")).unwrap();
        let source = stage.path.join("1/asset.png");
        std::fs::write(
          &source,
          include_bytes!("../../../src/assets/placeholder.png"),
        )
        .unwrap();
        std::fs::write(stage.path.join("images.txt"), b"manifest").unwrap();
        if outcome == "success" {
          finalize(&source, &destination, || false).unwrap();
        }
        if outcome == "cancelled" {
          assert!(finalize(&source, &destination, || true).is_err());
        }
      }
      let files: Vec<_> = std::fs::read_dir(&root.path)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
      assert_eq!(
        files,
        if outcome == "success" {
          vec![destination]
        } else {
          vec![]
        },
        "{outcome}"
      );
    }
  }

  #[test]
  fn validates_bytes_and_never_overwrites_an_existing_file() {
    let stage = Staging::new().unwrap();
    let source = stage.path.join("source.png");
    let destination = stage.path.join("saved.png");
    let png = include_bytes!("../../../src/assets/placeholder.png");
    std::fs::write(&source, png).unwrap();
    assert_eq!(image_type(png).unwrap(), "png");
    assert!(!finalize(&source, &destination, || false).unwrap());
    assert_eq!(std::fs::read(&destination).unwrap(), png);
    // An identical existing file is reported as reused, not as a new save.
    assert!(finalize(&source, &destination, || false).unwrap());
    assert_eq!(std::fs::read(&destination).unwrap(), png);
    std::fs::write(&destination, b"an unrelated existing file").unwrap();
    assert!(finalize(&source, &destination, || false).is_err());
    assert_eq!(
      std::fs::read(&destination).unwrap(),
      b"an unrelated existing file"
    );
    assert!(finalize(&source, &stage.path.join("cancelled.png"), || true).is_err());
    assert!(!stage.path.join("cancelled.png").exists());
    assert!(image_type(b"<html>login required</html>").is_err());
    let mut corrupted = png.to_vec();
    corrupted[30] ^= 1;
    assert!(image_type(&corrupted).is_err());
  }

  #[test]
  fn download_again_takes_the_next_free_numbered_name() {
    let stage = Staging::new().unwrap();
    let saved = stage.path.join("Post-001.png");
    assert_eq!(next_free_name(&saved), saved);
    std::fs::write(&saved, b"x").unwrap();
    assert_eq!(next_free_name(&saved), stage.path.join("Post-001-001.png"));
    std::fs::write(stage.path.join("Post-001-001.png"), b"x").unwrap();
    assert_eq!(next_free_name(&saved), stage.path.join("Post-001-002.png"));
  }

  #[test]
  fn download_again_skips_a_name_taken_after_it_was_chosen() {
    let stage = Staging::new().unwrap();
    let source = stage.path.join("source.png");
    let destination = stage.path.join("saved.png");
    let png = include_bytes!("../../../src/assets/placeholder.png");
    std::fs::write(&source, png).unwrap();
    std::fs::write(&destination, png).unwrap();
    // Another "Download again" already wrote the name this one chose: identical bytes, then different bytes.
    let taken = stage.path.join("saved-001.png");
    std::fs::write(&taken, png).unwrap();
    let saved = finalize_fresh_with(&source, &destination, || false, |_| taken.clone()).unwrap();
    assert_eq!(saved, stage.path.join("saved-002.png"));
    let other = stage.path.join("saved-003.png");
    std::fs::write(&other, b"another file").unwrap();
    let saved = finalize_fresh_with(&source, &destination, || false, |_| other.clone()).unwrap();
    assert_eq!(saved, stage.path.join("saved-004.png"));
    assert_eq!(std::fs::read(&other).unwrap(), b"another file");
    assert_eq!(std::fs::read(&taken).unwrap(), png);
  }

  #[test]
  fn filesystems_without_hard_links_can_save_without_replacing_existing_files() {
    let stage = Staging::new().unwrap();
    let source = stage.path.join("source.png");
    let destination = stage.path.join("saved.png");
    let png = include_bytes!("../../../src/assets/placeholder.png");
    std::fs::write(&source, png).unwrap();
    let no_links = |_: &Path, _: &Path| Err(std::io::Error::from(std::io::ErrorKind::Unsupported));
    assert!(!finalize_with_link(&source, &destination, || false, no_links).unwrap());
    assert_eq!(std::fs::read(&destination).unwrap(), png);
    assert!(finalize_with_link(&source, &destination, || false, no_links).unwrap());
    std::fs::write(&destination, b"unrelated").unwrap();
    assert!(finalize_with_link(&source, &destination, || false, no_links).is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"unrelated");
    let checks = std::cell::Cell::new(0);
    let cancelled = stage.path.join("cancelled-copy.png");
    assert_eq!(
      finalize_with_link(
        &source,
        &cancelled,
        || {
          checks.set(checks.get() + 1);
          checks.get() >= 3
        },
        no_links
      ),
      Err(GalleryError::Cancelled)
    );
    assert!(!cancelled.exists());
  }

  #[cfg(unix)]
  #[test]
  fn photo_save_failures_report_their_own_cause_not_permission_denied() {
    let stage = Staging::new().unwrap();
    let source = stage.path.join("source.png");
    let png = include_bytes!("../../../src/assets/placeholder.png");
    std::fs::write(&source, png).unwrap();
    let too_long = stage.path.join(format!("{}.png", "a".repeat(300)));
    assert_eq!(
      finalize_with_link(
        &source,
        &too_long,
        || false,
        |from, to| std::fs::hard_link(from, to)
      ),
      Err(GalleryError::Refused("outputNameTooLong"))
    );
    // A folder that is really a file is not a permission problem either.
    let under_file = source.join("saved.png");
    assert_eq!(
      finalize_with_link(
        &source,
        &under_file,
        || false,
        |from, to| std::fs::hard_link(from, to)
      ),
      Err(GalleryError::Refused("unknown"))
    );
  }
}
