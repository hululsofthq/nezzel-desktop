use super::updater_check::UpdateMode;
use crate::net::OwnProxy;
use crate::paths::PathsManager;
use std::io;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::{Result, Update};
use tokio::sync::Mutex;

#[derive(serde::Serialize, Clone)]
pub struct UpdateProgress {
  pub received: usize,
  pub total: Option<u64>,
}

#[derive(Default)]
pub struct UpdateStore {
  pub update: Option<Update>,
  pub bytes: Option<Vec<u8>>,
  /// The proxy setting the stored update was checked with (its requests keep that setting).
  pub checked_with: Option<OwnProxy>,
}

/// A stored update whose check ran under another proxy setting than the live one must not download.
fn stale(checked_with: &Option<OwnProxy>, live: &OwnProxy) -> bool {
  checked_with.as_ref() != Some(live)
}

#[tauri::command]
pub async fn updater_download(
  app: AppHandle,
  store: State<'_, Mutex<UpdateStore>>,
  paths: State<'_, PathsManager>,
) -> Result<()> {
  if !UpdateMode::of(&paths).can_download() {
    return Err(io::Error::other("this build is updated outside the app").into());
  }
  let update = {
    let s = store.lock().await;
    let update = s
      .update
      .clone()
      .ok_or_else(|| io::Error::other("no update available"))?;
    if stale(&s.checked_with, &crate::net::own_proxy()) {
      return Err(io::Error::other("proxy setting changed, check for updates again").into());
    }
    update
  };
  let app_for_progress = app.clone();
  let app_for_error = app.clone();

  crate::usage::count(&app, crate::usage::keys::UPDATE_TAPPED);
  let mut received: usize = 0;
  tracing::info!("[updater] Downloading update: {}", update.version);
  let bytes = update
    .download(
      move |chunk_len, content_len| {
        received += chunk_len;
        let _ = app_for_progress.emit(
          "updater_download_progress",
          UpdateProgress {
            total: content_len,
            received,
          },
        );
      },
      move || {
        tracing::info!("[updater] Update download complete.");
      },
    )
    .await;

  match bytes {
    Ok(bytes) => {
      crate::usage::count(&app, crate::usage::keys::UPDATE_READY);
      let mut s = store.lock().await;
      s.bytes = Some(bytes);
      Ok(())
    }
    Err(e) => {
      tracing::info!("[updater] Update download failed: {e}");
      crate::usage::count(&app, crate::usage::update_failed_key(&e));
      let _ = app_for_error.emit("updater_error", e.to_string());
      Err(e)
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn an_update_checked_under_another_proxy_setting_is_stale() {
    let via = OwnProxy::Via("http://127.0.0.1:8888".into());
    assert!(!stale(&Some(OwnProxy::Direct), &OwnProxy::Direct));
    assert!(!stale(&Some(via.clone()), &via));
    assert!(stale(&Some(OwnProxy::Direct), &via));
    assert!(stale(&Some(via.clone()), &OwnProxy::Direct));
    assert!(stale(
      &Some(via),
      &OwnProxy::Via("http://127.0.0.1:9".into())
    ));
    assert!(stale(&None, &OwnProxy::Direct));
  }
}
