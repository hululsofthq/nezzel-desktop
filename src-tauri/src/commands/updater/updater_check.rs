use crate::commands::UpdateStore;
use crate::net::OwnProxy;
use crate::paths::PathsManager;
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Config, Error, Result as UpdaterResult, Update, UpdaterExt};
use tokio::sync::Mutex;

/// Where a build that cannot update itself gets its updates (the UI says so instead of offering a download).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
  Store,
  Snap,
}

impl UpdateChannel {
  /// Every value, as the webview's `UpdateChannel` (`crate::contract`).
  #[cfg_attr(not(test), allow(dead_code))]
  pub const ALL: [Self; 2] = [Self::Store, Self::Snap];

  #[cfg_attr(not(test), allow(dead_code))]
  pub const fn as_str(self) -> &'static str {
    match self {
      Self::Store => "store",
      Self::Snap => "snap",
    }
  }
}

/// What this build may do about a newer release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateMode {
  /// Downloads and installs in-app (installed builds: dmg, NSIS, deb, rpm, AppImage).
  SelfUpdate,
  /// Portable: a newer version is reported, but the user downloads it themselves.
  Manual,
  /// Microsoft Store / Snap: the store updates it; the manifest is not contacted.
  Store(UpdateChannel),
}

impl UpdateMode {
  pub fn of(paths: &PathsManager) -> Self {
    Self::from_flags(
      paths.is_microsoft_store_app(),
      paths.is_portable_app(),
      paths.is_snap_app(),
    )
  }

  fn from_flags(microsoft_store: bool, portable: bool, snap: bool) -> Self {
    if microsoft_store {
      Self::Store(UpdateChannel::Store)
    } else if snap {
      Self::Store(UpdateChannel::Snap)
    } else if portable {
      Self::Manual
    } else {
      Self::SelfUpdate
    }
  }

  pub fn can_download(self) -> bool {
    self == Self::SelfUpdate
  }
}

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
  pub available: bool,
  /// The newer version, when one exists.
  pub version: Option<String>,
  pub current_version: String,
  /// The manifest's `notes` string, unparsed.
  pub notes: Option<String>,
  /// A newer version exists but this build cannot install it in-app.
  pub manual: bool,
  /// Set when updates come from a store, whether or not a newer version is known.
  pub channel: Option<UpdateChannel>,
}

/// The newer release found in the manifest: (version, notes).
type Found = Option<(String, Option<String>)>;

fn describe(mode: UpdateMode, current: String, found: Found) -> UpdateCheck {
  let (version, notes) = match (mode, found) {
    (UpdateMode::Store(_), _) | (_, None) => (None, None),
    (_, Some((version, notes))) => (Some(version), notes),
  };
  UpdateCheck {
    available: version.is_some(),
    manual: mode == UpdateMode::Manual && version.is_some(),
    channel: match mode {
      UpdateMode::Store(channel) => Some(channel),
      _ => None,
    },
    version,
    current_version: current,
    notes,
  }
}

#[derive(Debug, PartialEq, Eq)]
enum ManifestStatus {
  Missing,
  Check,
  Failed,
}

fn manifest_status(status: u16) -> ManifestStatus {
  match status {
    404 => ManifestStatus::Missing,
    200..=299 => ManifestStatus::Check,
    _ => ManifestStatus::Failed,
  }
}

fn release_client(proxy: &OwnProxy) -> UpdaterResult<Option<reqwest::Client>> {
  let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(10));
  match proxy {
    OwnProxy::Direct => {}
    OwnProxy::Via(url) => builder = builder.proxy(reqwest::Proxy::all(url)?),
    OwnProxy::Blocked => return Ok(None),
  }
  Ok(Some(builder.build()?))
}

/// The proxy for the plugin's own check and download requests (they follow the Update it returns).
/// Passed through `configure_client`, not `UpdaterBuilder::proxy`: the plugin logs that one's raw URL
/// (user:password included).
fn updater_proxy(proxy: &OwnProxy) -> Option<reqwest::Proxy> {
  match proxy {
    OwnProxy::Via(url) => reqwest::Proxy::all(url).ok(),
    _ => None,
  }
}

async fn check_published_release(
  app: &AppHandle,
  proxy: &OwnProxy,
) -> UpdaterResult<Option<Update>> {
  let config: Config = app
    .config()
    .plugins
    .0
    .get("updater")
    .cloned()
    .map(serde_json::from_value)
    .transpose()?
    .unwrap_or_default();
  if config.endpoints.is_empty() {
    return Ok(None);
  }
  // Proxy on without an address: no request leaves the app (the tool downloads do the same).
  let Some(client) = release_client(proxy)? else {
    return Ok(None);
  };
  for endpoint in config.endpoints {
    // The plugin erases all non-2xx statuses into ReleaseNotFound. Only a confirmed 404
    // means "not published"; transport, permission and server failures must remain errors.
    let status = client.get(endpoint.clone()).send().await?.status().as_u16();
    match manifest_status(status) {
      ManifestStatus::Missing => continue,
      ManifestStatus::Failed => return Err(Error::ReleaseNotFound),
      ManifestStatus::Check => {
        // Probe and check the same endpoint. A later failure is still an error (including
        // ReleaseNotFound if the response changes between the two GETs).
        let mut updater = app.updater_builder().endpoints(vec![endpoint])?;
        match (proxy, updater_proxy(proxy)) {
          (_, Some(proxy)) => {
            updater = updater.configure_client(move |builder| builder.proxy(proxy.clone()))
          }
          (OwnProxy::Direct, None) => {}
          // An address the plugin cannot take: never fall back to a direct connection.
          _ => return Ok(None),
        }
        return updater.build()?.check().await;
      }
    }
  }
  Ok(None)
}

#[tauri::command]
pub async fn updater_check(
  app: AppHandle,
  store: State<'_, Mutex<UpdateStore>>,
  paths: State<'_, PathsManager>,
) -> UpdaterResult<UpdateCheck> {
  let current = app.package_info().version.to_string();
  let mode = UpdateMode::of(&paths);
  // Store and Snap builds are updated by their store: no manifest request.
  if matches!(mode, UpdateMode::Store(_)) {
    return Ok(describe(mode, current, None));
  }
  let proxy = crate::net::own_proxy();
  let checked = check_published_release(&app, &proxy).await?;
  let mut s = store.lock().await;
  s.bytes = None;
  s.update = None;
  s.checked_with = None;
  let Some(update) = checked else {
    return Ok(describe(mode, current, None));
  };
  tracing::info!("[updater] Update available: {}", update.version);
  let found = Some((update.version.clone(), update.body.clone()));
  // A manual (portable) build only reports the version: nothing is stored to download.
  if mode.can_download() {
    crate::usage::update_offered(&app, &update.version);
    s.update = Some(update);
    s.checked_with = Some(proxy);
  }
  Ok(describe(mode, current, found))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn found() -> Found {
    Some(("0.2.0".into(), Some("{\"en\":[\"a\"]}".into())))
  }

  #[test]
  fn modes_follow_the_build_kind() {
    assert_eq!(
      UpdateMode::from_flags(false, false, false),
      UpdateMode::SelfUpdate
    );
    assert_eq!(
      UpdateMode::from_flags(false, true, false),
      UpdateMode::Manual
    );
    assert_eq!(
      UpdateMode::from_flags(true, false, false),
      UpdateMode::Store(UpdateChannel::Store)
    );
    assert_eq!(
      UpdateMode::from_flags(false, false, true),
      UpdateMode::Store(UpdateChannel::Snap)
    );
    assert!(UpdateMode::SelfUpdate.can_download());
    assert!(!UpdateMode::Manual.can_download());
    assert!(!UpdateMode::Store(UpdateChannel::Store).can_download());
    assert!(!UpdateMode::Store(UpdateChannel::Snap).can_download());
  }

  #[test]
  fn self_updating_build_reports_version_and_notes_without_manual() {
    let r = describe(UpdateMode::SelfUpdate, "0.1.4".into(), found());
    assert!(r.available && !r.manual && r.channel.is_none());
    assert_eq!(r.version.as_deref(), Some("0.2.0"));
    assert_eq!(r.notes.as_deref(), Some("{\"en\":[\"a\"]}"));
    let none = describe(UpdateMode::SelfUpdate, "0.1.4".into(), None);
    assert!(!none.available && !none.manual && none.version.is_none() && none.notes.is_none());
  }

  #[test]
  fn portable_build_is_manual_only_when_a_newer_version_exists() {
    let r = describe(UpdateMode::Manual, "0.1.4".into(), found());
    assert!(r.available && r.manual && r.channel.is_none());
    assert_eq!(r.version.as_deref(), Some("0.2.0"));
    let none = describe(UpdateMode::Manual, "0.2.0".into(), None);
    assert!(!none.available && !none.manual);
  }

  #[test]
  fn store_and_snap_builds_carry_the_channel_and_never_a_version() {
    for (channel, word) in [
      (UpdateChannel::Store, "store"),
      (UpdateChannel::Snap, "snap"),
    ] {
      let r = describe(UpdateMode::Store(channel), "0.1.4".into(), found());
      assert!(!r.available && !r.manual && r.version.is_none() && r.notes.is_none());
      assert_eq!(r.channel, Some(channel));
      assert_eq!(serde_json::to_value(&r).unwrap()["channel"], word);
    }
  }

  #[test]
  fn wire_shape_is_camel_case() {
    let v = serde_json::to_value(describe(UpdateMode::Manual, "0.1.4".into(), found())).unwrap();
    for key in [
      "available",
      "version",
      "currentVersion",
      "notes",
      "manual",
      "channel",
    ] {
      assert!(v.get(key).is_some(), "{key}");
    }
  }

  #[tokio::test]
  async fn update_check_requests_use_the_proxy_only_when_set() {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let via = OwnProxy::Via(format!("http://{address}"));
    assert!(updater_proxy(&via).is_some());
    assert!(updater_proxy(&OwnProxy::Via("http://[bad".into())).is_none());
    assert!(updater_proxy(&OwnProxy::Direct).is_none());
    assert!(release_client(&OwnProxy::Blocked).unwrap().is_none());
    let seen = tokio::spawn(async move {
      let (mut socket, _) = listener.accept().await.unwrap();
      let mut buf = [0u8; 256];
      let n = socket.read(&mut buf).await.unwrap();
      String::from_utf8_lossy(&buf[..n]).into_owned()
    });
    let client = release_client(&via).unwrap().unwrap();
    let _ = client
      .get("http://updates.invalid/latest.json")
      .send()
      .await;
    assert!(seen
      .await
      .unwrap()
      .starts_with("GET http://updates.invalid/latest.json"));
    // No proxy: a direct lookup of a name that never resolves.
    let client = release_client(&OwnProxy::Direct).unwrap().unwrap();
    assert!(client
      .get("http://updates.invalid/latest.json")
      .send()
      .await
      .is_err());
  }

  #[test]
  fn only_missing_manifests_are_latest_and_successes_reach_the_plugin() {
    for (status, expected) in [
      (200, ManifestStatus::Check),
      (204, ManifestStatus::Check),
      (302, ManifestStatus::Failed),
      (403, ManifestStatus::Failed),
      (404, ManifestStatus::Missing),
      (500, ManifestStatus::Failed),
      (503, ManifestStatus::Failed),
    ] {
      assert_eq!(manifest_status(status), expected, "HTTP {status}");
    }
  }
}
