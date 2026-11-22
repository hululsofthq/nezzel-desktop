use super::updater_check::UpdateMode;
use crate::net::OwnProxy;
use crate::paths::PathsManager;
use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::Update;
use tokio::sync::{watch, Mutex, Notify};

/// The codes the webview gets when the download is stopped on purpose. Matched by `src/stores/updater.ts`.
pub const STALLED: &str = "stalled";
pub const CANCELLED: &str = "cancelled";
/// No bytes for this long ends the download (a stall limit, not a cap on the total time).
const STALL_AFTER: Duration = Duration::from_secs(60);

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
  /// Wakes the running download so `updater_cancel` can stop it.
  pub cancel: Arc<Notify>,
}

#[derive(Debug, PartialEq)]
enum Stop {
  Stalled,
  Cancelled,
}

/// Runs `fut` until it ends, `cancel` fires, or `stall` passes without `chunks` changing.
/// Stopping drops `fut`, which closes the connection: nothing keeps downloading.
async fn guarded<T>(
  fut: impl Future<Output = T>,
  mut chunks: watch::Receiver<usize>,
  stall: Duration,
  cancel: &Notify,
) -> Result<T, Stop> {
  tokio::pin!(fut);
  loop {
    tokio::select! {
      out = &mut fut => return Ok(out),
      _ = cancel.notified() => return Err(Stop::Cancelled),
      _ = tokio::time::sleep(stall) => return Err(Stop::Stalled),
      // Fresh bytes restart the stall clock; the caller keeps a sender alive, so this only wakes on a chunk.
      _ = chunks.changed() => {}
    }
  }
}

#[tauri::command]
pub async fn updater_cancel(store: State<'_, Mutex<UpdateStore>>) -> Result<(), String> {
  store.lock().await.cancel.notify_waiters();
  Ok(())
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
) -> Result<(), String> {
  if !UpdateMode::of(&paths).can_download() {
    return Err(io::Error::other("this build is updated outside the app").to_string());
  }
  let (update, cancel) = {
    let s = store.lock().await;
    let update = s
      .update
      .clone()
      .ok_or_else(|| io::Error::other("no update available").to_string())?;
    if stale(&s.checked_with, &crate::net::own_proxy()) {
      return Err(io::Error::other("proxy setting changed, check for updates again").to_string());
    }
    (update, s.cancel.clone())
  };
  let app_for_progress = app.clone();
  let app_for_error = app.clone();

  crate::usage::count(&app, crate::usage::keys::UPDATE_TAPPED);
  let mut received: usize = 0;
  tracing::info!("[updater] Downloading update: {}", update.version);
  let (chunk_tx, chunk_rx) = watch::channel(0usize);
  let chunk_tx = Arc::new(chunk_tx);
  let chunk_tx_cb = chunk_tx.clone();
  let download = update.download(
    move |chunk_len, content_len| {
      received += chunk_len;
      chunk_tx_cb.send_replace(received);
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
  );
  let outcome = guarded(download, chunk_rx, STALL_AFTER, &cancel).await;
  drop(chunk_tx);

  match outcome {
    Ok(Ok(bytes)) => {
      crate::usage::count(&app, crate::usage::keys::UPDATE_READY);
      let mut s = store.lock().await;
      s.bytes = Some(bytes);
      Ok(())
    }
    Ok(Err(e)) => {
      tracing::info!("[updater] Update download failed: {e}");
      crate::usage::count(&app, crate::usage::update_failed_key(&e));
      let _ = app_for_error.emit("updater_error", e.to_string());
      Err(e.to_string())
    }
    Err(Stop::Stalled) => {
      tracing::info!("[updater] Update download stalled, stopped.");
      crate::usage::count(&app, crate::usage::keys::UPDATE_FAILED_NETWORK);
      let _ = app_for_error.emit("updater_error", STALLED);
      Err(STALLED.to_string())
    }
    Err(Stop::Cancelled) => {
      tracing::info!("[updater] Update download cancelled.");
      Err(CANCELLED.to_string())
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

  #[tokio::test(start_paused = true)]
  async fn a_download_that_keeps_receiving_bytes_outlives_the_stall_limit() {
    let (tx, rx) = watch::channel(0usize);
    let cancel = Notify::new();
    let fut = async {
      for i in 1..=10usize {
        tokio::time::sleep(Duration::from_secs(30)).await;
        tx.send_replace(i);
      }
      "done"
    };
    let out = guarded(fut, rx, Duration::from_secs(60), &cancel).await;
    assert_eq!(out, Ok("done")); // 300 s in total
  }

  #[tokio::test(start_paused = true)]
  async fn no_bytes_for_the_stall_limit_stops_and_drops_the_download() {
    struct Dropped(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for Dropped {
      fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
      }
    }
    let (tx, rx) = watch::channel(0usize);
    let cancel = Notify::new();
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let guard = Dropped(dropped.clone());
    let fut = async move {
      let _guard = guard;
      tx.send_replace(1);
      tokio::time::sleep(Duration::from_secs(50)).await;
      tx.send_replace(2);
      std::future::pending::<()>().await;
    };
    let out = guarded(fut, rx, Duration::from_secs(60), &cancel).await;
    assert_eq!(out, Err(Stop::Stalled));
    assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
  }

  #[tokio::test(start_paused = true)]
  async fn cancel_stops_the_download() {
    let (_tx, rx) = watch::channel(0usize);
    let cancel = Arc::new(Notify::new());
    let c = cancel.clone();
    tokio::spawn(async move {
      tokio::time::sleep(Duration::from_secs(5)).await;
      c.notify_waiters();
    });
    let out = guarded(
      std::future::pending::<()>(),
      rx,
      Duration::from_secs(60),
      &cancel,
    )
    .await;
    assert_eq!(out, Err(Stop::Cancelled));
  }
}
