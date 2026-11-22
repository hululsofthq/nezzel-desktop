use crate::commands::UpdateStore;
use std::io;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::Error;
use tokio::sync::Mutex;

/// The code the webview gets when the system package manager could not install the update
/// (password prompt refused or missing, or the manager failed). Matched by `src/stores/updater.ts`.
pub const NEEDS_PASSWORD: &str = "needs_password";

/// A stable code for an install failure of a deb/rpm build; `None` for everything else (its text is passed on).
fn install_failure_code(error: &Error, system_package: bool) -> Option<&'static str> {
  if !system_package {
    return None;
  }
  match error {
    Error::AuthenticationFailed
    | Error::PackageInstallFailed
    | Error::DebInstallFailed
    | Error::Io(_) => Some(NEEDS_PASSWORD),
    _ => None,
  }
}

#[tauri::command]
pub async fn updater_install(
  app: AppHandle,
  store: State<'_, Mutex<UpdateStore>>,
) -> Result<(), String> {
  let (update, bytes) = {
    let mut s = store.lock().await;

    let update = s
      .update
      .clone()
      .ok_or_else(|| io::Error::other("no update available").to_string())?;

    let bytes = s
      .bytes
      .take()
      .ok_or_else(|| io::Error::other("no update downloaded").to_string())?;

    (update, bytes)
  };

  if let Err(e) = update.install(&bytes) {
    crate::usage::count(&app, crate::usage::keys::UPDATE_FAILED_INSTALL);
    return Err(
      match install_failure_code(&e, super::updater_check::is_system_package()) {
        Some(code) => {
          // Nothing was installed: keep the download so Retry does not fetch it again.
          store.lock().await.bytes = Some(bytes);
          code.to_string()
        }
        None => e.to_string(),
      },
    );
  }
  let _ = app.emit("updater_finished", ());
  app.request_restart();
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn package_install_failures_map_to_one_stable_code() {
    for e in [
      Error::AuthenticationFailed,
      Error::PackageInstallFailed,
      Error::DebInstallFailed,
      Error::Io(io::Error::other("sudo: not found")),
    ] {
      assert_eq!(install_failure_code(&e, true), Some(NEEDS_PASSWORD));
      assert_eq!(install_failure_code(&e, false), None);
    }
    assert_eq!(
      install_failure_code(&Error::InvalidUpdaterFormat, true),
      None
    );
    assert_eq!(install_failure_code(&Error::TempDirNotFound, true), None);
  }
}
