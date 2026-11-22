use crate::commands::UpdateStore;
use crate::net::OwnProxy;
use crate::paths::PathsManager;
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Config, Error, Result as UpdaterResult, Update, UpdaterExt};
use tokio::sync::Mutex;

#[derive(serde::Serialize)]
pub struct UpdateCheck {
  pub available: bool,
  pub current_version: String,
  pub available_version: Option<String>,
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
  // Portable app does not auto-update, nor does the microsoft/snap store version.
  if paths.is_microsoft_store_app() || paths.is_portable_app() || paths.is_snap_app() {
    return Ok(UpdateCheck {
      available: false,
      current_version: current,
      available_version: None,
    });
  }
  let proxy = crate::net::own_proxy();
  let checked = check_published_release(&app, &proxy).await?;
  if let Some(update) = checked {
    tracing::info!("[updater] Update available: {}", update.version);
    crate::usage::update_offered(&app, &update.version);
    let mut s = store.lock().await;
    s.bytes = None;
    s.update = Some(update.clone());
    s.checked_with = Some(proxy);
    Ok(UpdateCheck {
      available: true,
      current_version: current,
      available_version: Some(update.version),
    })
  } else {
    let mut s = store.lock().await;
    s.bytes = None;
    s.update = None;
    s.checked_with = None;
    Ok(UpdateCheck {
      available: false,
      current_version: current,
      available_version: None,
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;

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
