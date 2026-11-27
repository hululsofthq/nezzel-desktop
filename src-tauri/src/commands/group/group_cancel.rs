use crate::download_records::DownloadRecords;
use crate::logging::LogStoreState;
use crate::scheduling::dispatcher::DispatchRequest;
use crate::scheduling::download_pipeline::DownloadSender;
use crate::scheduling::fetch_pipeline::FetchSender;
use crate::scheduling::group_state::{cancel_group, finish_group_cancel};
use tauri::State;

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri injects each State as its own argument.
pub fn group_cancel(
  app: tauri::AppHandle,
  group_id: String,
  discard_files: Option<bool>,
  partial_roots: Option<Vec<String>>,
  log_state: State<'_, LogStoreState>,
  fetch_sender: State<'_, FetchSender>,
  download_sender: State<'_, DownloadSender>,
  records: State<'_, DownloadRecords>,
  library: State<'_, crate::library::Library>,
) {
  // Keep cancellation bookkeeping ordered with the synchronous media_download command.
  let generation = cancel_group(&group_id);
  crate::catch::auto::forget(&app, &group_id);
  library.forget_group(&group_id);
  crate::files::runner::forget(&group_id);
  let discard_roots = if discard_files == Some(true) {
    // Removed from the list (not a pause): its job records go, so a reused ID counts its own end.
    crate::usage::forget_group(&app, &group_id);
    crate::files::runner::discard_group(&group_id);
    Some(partial_roots.unwrap_or_default())
  } else {
    None
  };
  records.forget(&group_id);

  // Send cleanup requests for items that were already dispatched.
  let _ = fetch_sender.0.send(DispatchRequest::Cleanup {
    group_id: group_id.clone(),
    generation: Some(generation),
  });
  let _ = download_sender.0.send(DispatchRequest::Cleanup {
    group_id: group_id.clone(),
    generation: Some(generation),
  });

  log_state.write().remove_group(&group_id);

  let cleanup_group = group_id.clone();
  let disk = tauri::async_runtime::spawn_blocking(move || {
    crate::runners::ytdlp_download::cancel_partials(
      &app,
      &cleanup_group,
      generation,
      discard_roots.as_deref(),
    );
  });
  tauri::async_runtime::spawn(async move {
    let result = disk.await;
    finish_group_cancel(&group_id, generation);
    if let Err(error) = result {
      tracing::warn!(%error, "Download partial cleanup task failed");
    }
  });
}
