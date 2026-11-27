//! Developer-only sweep (`--features catch-matrix`): the hidden catch session on a list of pages,
//! one after another, in a host with no window and none of the owner's settings or sign-ins.
//! The list stays outside the repo; the result names sites and counts, never page addresses.

use super::{
  auto::{
    admitted, site_of, AutoCatch, AutoPhase, Backend, Candidate, CatchAutoState, Fail, Page, Plan,
    Snapshot, View,
  },
  window::{hidden, CatchWindows},
};
use crate::{state::config_models::Config, SharedConfig};
use futures::future::BoxFuture;
use reqwest::Url;
use serde_json::{json, Value};
use std::{
  error::Error,
  fs,
  path::PathBuf,
  sync::{Arc, Mutex},
  time::Instant,
};
use tauri::{AppHandle, Manager};

#[derive(Default)]
struct Seen {
  started: Option<Instant>,
  /// Each phase the row would have shown, with the milliseconds since the session began.
  phases: Vec<(AutoPhase, u128)>,
  found: Option<CatchAutoState>,
  note: Option<Value>,
  opened: bool,
  /// The open page, for the timed look at its document.
  page: Option<Arc<dyn Page>>,
}

struct Recorder {
  app: AppHandle,
  seen: Arc<Mutex<Seen>>,
}

struct Watched {
  page: Arc<dyn Page>,
  seen: Arc<Mutex<Seen>>,
}

impl Page for Watched {
  fn view(&self) -> Option<View> {
    if let (Some(note), Ok(mut seen)) = (self.page.note(), self.seen.lock()) {
      seen.note = Some(note);
    }
    self.page.view()
  }

  fn final_view(&self) -> BoxFuture<'_, Option<View>> {
    self.page.final_view()
  }

  fn capture<'a>(
    &'a self,
    candidate: &'a Candidate,
  ) -> BoxFuture<'a, Result<Arc<dyn Snapshot>, Fail>> {
    self.page.capture(candidate)
  }

  fn mobile(&self) -> BoxFuture<'_, bool> {
    self.page.mobile()
  }

  fn close(&self) -> BoxFuture<'_, ()> {
    self.page.close()
  }

  fn inspect_dom(&self) -> BoxFuture<'_, Option<Value>> {
    self.page.inspect_dom()
  }
}

impl Backend for Recorder {
  fn admit<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, bool> {
    Box::pin(admitted(url))
  }

  fn open<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Option<Arc<dyn Page>>> {
    Box::pin(async move {
      let page = hidden::open(&self.app, url).await?;
      if let Ok(mut seen) = self.seen.lock() {
        seen.opened = true;
        seen.page = Some(page.clone());
      }
      Some(Arc::new(Watched {
        page,
        seen: self.seen.clone(),
      }) as Arc<dyn Page>)
    })
  }

  fn emit(&self, state: CatchAutoState) {
    let Ok(mut seen) = self.seen.lock() else {
      return;
    };
    let at = seen
      .started
      .map_or(0, |started| started.elapsed().as_millis());
    seen.phases.push((state.phase, at));
    if state.phase == AutoPhase::Found {
      seen.found = Some(state);
    }
  }

  fn count(&self, _event: &'static str, _page: &Url) {}
}

struct Row {
  number: usize,
  category: String,
  url: Url,
  expect: String,
}

fn rows(list: &str, only: Option<&str>) -> Vec<Row> {
  list
    .lines()
    .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
    .enumerate()
    .filter_map(|(at, line)| {
      let mut fields = line.split('\t');
      let category = fields.next()?.trim().to_owned();
      let raw = fields.next()?.trim();
      let expect = fields.next().unwrap_or_default().trim().to_owned();
      Some(Row {
        number: at + 1,
        category,
        url: Url::parse(raw).ok()?,
        expect,
      })
    })
    .filter(|row| {
      only.is_none_or(|word| {
        row.url.as_str().contains(word) || row.category == word || row.number.to_string() == word
      })
    })
    .collect()
}

async fn one(app: &AppHandle, row: &Row) -> Value {
  let seen = Arc::new(Mutex::new(Seen {
    started: Some(Instant::now()),
    ..Seen::default()
  }));
  let auto = AutoCatch::new(Arc::new(Recorder {
    app: app.clone(),
    seen: seen.clone(),
  }));
  let started = Instant::now();
  let group = format!("matrix-{}", row.number);
  let look = auto.look_once(
    &group,
    Plan {
      url: row.url.clone(),
      beside: None,
    },
  );
  // The page's document at 10 s and just before the 30 s look ends.
  let dom = Arc::new(Mutex::new(Vec::new()));
  let looks = {
    let (seen, dom) = (seen.clone(), dom.clone());
    tauri::async_runtime::spawn(async move {
      for at in [10, 28] {
        tokio::time::sleep_until(
          tokio::time::Instant::from_std(started) + std::time::Duration::from_secs(at),
        )
        .await;
        let page = seen.lock().ok().and_then(|seen| seen.page.clone());
        let Some(page) = page else { break };
        let found = page.inspect_dom().await;
        if let Ok(mut dom) = dom.lock() {
          dom.push(json!({ "at": at, "dom": found }));
        }
      }
    })
  };
  // The session ends itself well before this; a row that does not is named, not waited for.
  let Ok(phase) = tokio::time::timeout(std::time::Duration::from_secs(120), look).await else {
    return json!({
      "n": row.number,
      "category": row.category,
      "site": site_of(&row.url),
      "expect": row.expect,
      "state": "hung",
      "ms": started.elapsed().as_millis(),
    });
  };
  let total = started.elapsed().as_millis();
  looks.abort();
  let dom = dom
    .lock()
    .map(|mut dom| std::mem::take(&mut *dom))
    .unwrap_or_default();
  let seen = seen.lock().map(|mut seen| std::mem::take(&mut *seen));
  let seen = seen.unwrap_or_default();
  let found = seen.found.as_ref();
  json!({
    "n": row.number,
    "category": row.category,
    "site": site_of(&row.url),
    "expect": row.expect,
    "state": phase.map_or(Value::from("refused"), |phase| json!(phase)),
    "opened": seen.opened,
    "ms": total,
    "phases": seen.phases.iter().map(|(phase, at)| json!([phase, at])).collect::<Vec<_>>(),
    "kind": found.and_then(|found| found.kind),
    "durationSec": found.and_then(|found| found.duration_sec),
    "formats": found.and_then(|found| found.formats.as_ref()).map(Vec::len),
    "titled": found.is_some_and(|found| found.title.is_some()),
    "pictured": found.is_some_and(|found| found.thumbnail.is_some()),
    "drm": phase == Some(AutoPhase::Encrypted),
    "page": seen.note,
    "dom": dom,
  })
}

async fn sweep(app: AppHandle, rows: Vec<Row>, out: PathBuf) -> Result<(), Box<dyn Error>> {
  let mut results = Vec::new();
  for row in &rows {
    let result = one(&app, row).await;
    eprintln!(
      "catch matrix: {:>2} {:<28} {:<10} {:>6} ms",
      row.number,
      result["site"].as_str().unwrap_or("-"),
      result["state"].as_str().unwrap_or("-"),
      result["ms"]
    );
    results.push(result);
    // Written after every page: a run that is stopped keeps what it has.
    fs::write(&out, serde_json::to_vec_pretty(&results)?)?;
  }
  Ok(())
}

fn option(args: &[String], name: &str) -> Option<String> {
  let at = args.iter().position(|arg| arg == name)?;
  args.get(at + 1).cloned()
}

fn start() -> Result<(), Box<dyn Error>> {
  // A panic on a worker thread would leave the sweep waiting for ever.
  let default_hook = std::panic::take_hook();
  std::panic::set_hook(Box::new(move |panic| {
    default_hook(panic);
    std::process::exit(3);
  }));
  let args: Vec<_> = std::env::args().skip(1).collect();
  let links = option(&args, "--links").ok_or("--links <list.tsv> is needed")?;
  let out = PathBuf::from(option(&args, "--out").ok_or("--out <results.json> is needed")?);
  let only = option(&args, "--only");
  let rows = rows(&fs::read_to_string(links)?, only.as_deref());
  if rows.is_empty() {
    return Err("no matching rows".into());
  }
  let home = std::env::temp_dir().join(format!("nezzel-catch-matrix-{}", uuid::Uuid::new_v4()));
  fs::create_dir_all(&home)?;
  let bin_dir = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is unset")?)
    .join("Library/Application Support/dev.ahmed.nezzel/bin");

  let mut context = tauri::generate_context!(test = true);
  context.config_mut().app.windows.clear();
  #[allow(unused_mut)]
  let mut app = super::window::with_crash_hook(tauri::Builder::default())
    .plugin(tauri_plugin_store::Builder::new().build())
    .build(context)?;
  // No Dock icon: it must not look like a second Nezzel, nor take the keyboard.
  #[cfg(target_os = "macos")]
  app.set_activation_policy(tauri::ActivationPolicy::Accessory);
  let handle = app.handle();
  handle.manage(crate::paths::PathsManager::isolated(home.clone(), bin_dir));
  let mut config = Config::default();
  // No browser sign-in is read or copied into the pages of a sweep.
  config.auth.cookie_browser = "none".into();
  let config: SharedConfig = Arc::new(crate::state::config::ConfigHandle::isolated(
    handle, config,
  )?);
  crate::net::setup(config.clone());
  handle.manage::<SharedConfig>(config);
  handle.manage::<crate::SharedPreferences>(Arc::new(
    crate::state::preferences::PreferencesHandle::isolated(
      handle,
      crate::state::preferences_models::Preferences::default(),
    )?,
  ));
  handle.manage(CatchWindows::default());
  handle.manage(hidden::HiddenWindows::default());

  let handle = handle.clone();
  tauri::async_runtime::spawn(async move {
    let outcome = sweep(handle.clone(), rows, out).await;
    if let Err(error) = &outcome {
      eprintln!("catch matrix: {error}");
    }
    hidden::stop_all(&handle);
    super::window::stop_all(&handle);
    let _ = fs::remove_dir_all(&home);
    std::process::exit(if outcome.is_ok() { 0 } else { 2 });
  });
  app.run(|_, event| {
    // A hidden page closing is never the end of the sweep.
    if let tauri::RunEvent::ExitRequested { api, .. } = event {
      api.prevent_exit();
    }
  });
  Ok(())
}

pub fn main() {
  if let Err(error) = start() {
    eprintln!("catch matrix: {error}");
    std::process::exit(2);
  }
}
