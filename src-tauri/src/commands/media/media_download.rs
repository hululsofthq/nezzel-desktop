use crate::library::{CompletionMetadata, Library};
use crate::models::DownloadItem;
use crate::scheduling::download_pipeline::DownloadSender;
use std::collections::HashMap;
use tauri::State;

#[tauri::command]
pub fn media_download(
  app: tauri::AppHandle,
  group_id: String,
  items: Vec<DownloadItem>,
  metadata: Option<HashMap<String, CompletionMetadata>>,
  origin: Option<String>,
  library: State<'_, Library>,
  pipeline: State<'_, DownloadSender>,
) -> String {
  let generation =
    crate::scheduling::download_pipeline::start_counted_batch(&group_id, items.len());
  // Downloading the embedded result ends a search for a longer video beside it.
  crate::catch::auto::acted(&app, &group_id);
  let ids: Vec<String> = items.iter().map(|item| item.id.clone()).collect();
  crate::usage::queued(
    &app,
    &group_id,
    &ids,
    crate::usage::origin(origin.as_deref(), ids.len()),
  );
  if let Some(metadata) = metadata {
    library.remember(&group_id, metadata);
  }

  pipeline.send_counted_batch(&group_id, generation, items);

  group_id
}
