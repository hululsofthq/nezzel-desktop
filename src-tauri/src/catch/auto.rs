//! Automatic catch: a hidden catch page looks for the video of a pasted link, one page at a time.
//! This file decides when to look, queues the sessions, bounds them in time and keeps what a found
//! video needs to download after its page is gone. The page itself is `window::hidden`.

use super::{
  candidate::compare_duration_and_host,
  handoff::CatchFormat,
  state::{is_http, Phase},
};
use crate::usage::keys::{CATCH_AUTO_FOUND, CATCH_AUTO_USED, CATCH_DRM};
use futures::future::BoxFuture;
use serde::Serialize;
use std::{
  collections::{HashMap, HashSet},
  net::IpAddr,
  sync::{Arc, Mutex, MutexGuard},
  time::Duration,
};
use tauri::{AppHandle, Emitter, EventTarget, Manager, Url};
use tokio::{
  sync::{watch, Semaphore},
  time::Instant,
};

const EVENT: &str = "catch://auto";
/// How long a page gets to show its video.
const LOOK: Duration = Duration::from_secs(30);
/// The page is gone after this, whatever it is doing.
const HARD_CAP: Duration = Duration::from_secs(45);
const TICK: Duration = Duration::from_millis(500);
/// After a first video shorter than `EMBEDDED_MAX_SEC`, how long a longer one may still appear.
const SETTLE: Duration = Duration::from_secs(5);
/// How long a loaded page must keep showing its "not available in your country" page before
/// the look ends on it: a slow page is short at first, and its words can be a banner's.
const GEO_GRACE: Duration = Duration::from_secs(3);
/// How long the row waits for its picture beside the format probe.
const THUMBNAIL_WAIT: Duration = Duration::from_secs(9);
/// A caught address is signed for a while only: past this, look again before downloading.
const SNAPSHOT_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_SNAPSHOTS: usize = 32;
const MAX_DOWNLOADS: usize = 64;
/// Top-frame hosts a page may pass through before its first finished load.
const MAX_HOPS: usize = 5;
/// An embedded result this long is the video the user wanted.
const EMBEDDED_MAX_SEC: f64 = 600.0;
const CATCH_CODES: [&str; 4] = [
  "urlNotSupported",
  "urlUnsupported",
  "unknownUrlType",
  "noMediaFound",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AutoPhase {
  Waiting,
  Looking,
  Found,
  Live,
  Encrypted,
  NoVideo,
  NeedsStep,
  Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatchAutoState {
  pub(crate) group_id: String,
  pub(crate) phase: AutoPhase,
  pub(crate) beside: bool,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub(crate) title: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub(crate) duration_sec: Option<f64>,
  /// A small JPEG `data:` URL fetched through an admitted request.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub(crate) thumbnail: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub(crate) kind: Option<&'static str>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub(crate) formats: Option<Vec<CatchFormat>>,
}

/// How the read of a pasted link ended.
pub(crate) enum Read<'a> {
  /// The diagnostic code of a failed read.
  Failed(&'a str),
  /// The diagnostic code of a failed read with yt-dlp's generic extractor.
  GenericExtractorFailed(&'a str),
  /// A result that another site hosts and the pasted page only embeds.
  Embedded {
    duration: Option<f64>,
    media: Option<&'a str>,
  },
}

#[derive(Clone, Copy)]
pub(crate) struct Gate {
  /// The `autoCatch` setting.
  pub(crate) setting: bool,
  /// A Settings proxy is on: a hidden page would leave outside it.
  pub(crate) proxied: bool,
  /// No window can be kept out of sight there.
  pub(crate) wayland: bool,
  /// The engine cannot be sent through the guard route (`hidden`), so a hidden page there would
  /// connect around the public-address rule.
  pub(crate) unguarded: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Beside {
  pub(super) duration: Option<f64>,
  /// Registrable domain of the embedded media's site.
  pub(super) site: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Plan {
  pub(super) url: Url,
  /// Set when the group keeps a usable embedded result while this runs.
  pub(super) beside: Option<Beside>,
}

/// Registrable domain, or the host itself for an address or an unlisted name.
pub(super) fn site_of(url: &Url) -> Option<String> {
  let host = url.host_str()?.trim_end_matches('.');
  Some(psl::domain_str(host).unwrap_or(host).to_ascii_lowercase())
}

/// Refuses what needs no lookup to refuse: literal private addresses and local names. Every
/// other host is looked up (`Backend::admit`) before a page loads it in the top frame.
pub(crate) fn may_be_public(url: &Url) -> bool {
  let Some(host) = crate::net::host(url) else {
    return false;
  };
  if let Ok(ip) = host.parse::<IpAddr>() {
    return crate::net::public_address(ip);
  }
  let host = host.trim_end_matches('.').to_ascii_lowercase();
  host.contains('.')
    && ![".localhost", ".local", ".internal", ".lan", ".home.arpa"]
      .iter()
      .any(|local| host.ends_with(local))
}

/// Whether a hidden session starts for this read, and with what.
pub(crate) fn plan(url: &str, read: &Read<'_>, gate: Gate) -> Option<Plan> {
  if !gate.setting || gate.proxied || gate.wayland || gate.unguarded {
    return None;
  }
  let url = Url::parse(url).ok().filter(is_http)?;
  if !may_be_public(&url) {
    return None;
  }
  let beside = match read {
    Read::Failed(code) | Read::GenericExtractorFailed(code) => {
      let catchable =
        CATCH_CODES.contains(code) || matches!(read, Read::GenericExtractorFailed("unknown"));
      if !catchable || crate::files::probe::file_extension(&url) {
        return None;
      }
      None
    }
    Read::Embedded { duration, media } => {
      if duration.is_some_and(|seconds| seconds >= EMBEDDED_MAX_SEC) {
        return None;
      }
      Some(Beside {
        duration: duration.filter(|seconds| seconds.is_finite() && *seconds > 0.0),
        site: media
          .and_then(|media| Url::parse(media).ok())
          .as_ref()
          .and_then(site_of),
      })
    }
  };
  Some(Plan { url, beside })
}

/// A main candidate of the hidden page (`state::PageState::auto_view`).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Candidate {
  pub(super) id: String,
  pub(super) kind: &'static str,
  pub(super) duration: Option<f64>,
  pub(super) loaded_host: u8,
  /// Registrable domains of the media address and of the frame that played it.
  pub(super) sites: [Option<String>; 2],
}

#[derive(Clone, Debug, Default)]
pub(super) struct View {
  /// The top frame's address; `None` on a blank page.
  pub(super) top: Option<Url>,
  pub(super) loading: bool,
  pub(super) title: Option<String>,
  pub(super) phase: Phase,
  pub(super) has_player: bool,
  pub(super) mains: Vec<Candidate>,
  /// Verified at the end of the look, with its duration and top-frame player measured.
  pub(super) lone: Option<Candidate>,
  /// The poster and the page's own picture, in the order a thumbnail tries them.
  pub(super) pictures: Vec<Url>,
}

/// Beside an embedded result only a longer video from another site counts.
fn eligible(candidate: &Candidate, beside: Option<&Beside>) -> bool {
  let Some(beside) = beside else {
    return true;
  };
  if beside
    .site
    .as_ref()
    .is_some_and(|site| candidate.sites.iter().flatten().any(|own| own == site))
  {
    return false;
  }
  match (candidate.duration, beside.duration) {
    (Some(caught), Some(embedded)) => caught > embedded,
    (Some(_), None) => true,
    (None, _) => false,
  }
}

/// The longest eligible main candidate, using host evidence to break duration ties.
fn best(mains: &[Candidate], beside: Option<&Beside>) -> Option<Candidate> {
  mains
    .iter()
    .filter(|candidate| eligible(candidate, beside))
    .max_by(|a, b| {
      compare_duration_and_host((a.duration, a.loaded_host), (b.duration, b.loaded_host))
    })
    .cloned()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fail {
  Encrypted,
  Live,
  /// The media address answered 403: it expired or needs the page again.
  Forbidden,
  Other,
}

pub(super) struct Probed {
  pub(super) formats: Vec<CatchFormat>,
  pub(super) duration: Option<f64>,
}

/// What a found video needs after its page is gone: the admitted request, its cookies and formats.
pub(super) trait Snapshot: Send + Sync {
  fn probe(&self) -> BoxFuture<'_, Result<Probed, Fail>>;
  /// Queues the download of the caught media address (never the page link); returns its id.
  fn save(&self, format_id: Option<String>, download_id: String)
    -> BoxFuture<'_, Result<(), Fail>>;
  /// A small picture for the row: the first of `pictures` that works, else one frame.
  fn thumbnail(&self, _pictures: Vec<Url>) -> BoxFuture<'_, Option<String>> {
    Box::pin(async { None })
  }
}

pub(super) trait Page: Send + Sync {
  /// `None` once the page is gone.
  fn view(&self) -> Option<View>;
  /// Drains queued events and checks the current document before a timeout verdict.
  fn final_view(&self) -> BoxFuture<'_, Option<View>>;
  fn capture<'a>(
    &'a self,
    candidate: &'a Candidate,
  ) -> BoxFuture<'a, Result<Arc<dyn Snapshot>, Fail>>;
  /// Reloads the page as the Mobile version, its old observations dropped; `false` when it
  /// could not. The page is destroyed after the session, so nothing is restored.
  fn mobile(&self) -> BoxFuture<'_, bool>;
  /// Destroys the page: playback, cookies and window.
  fn close(&self) -> BoxFuture<'_, ()>;
  /// What the page shows now, for the developer's sweep table.
  #[cfg(feature = "catch-matrix")]
  fn note(&self) -> Option<serde_json::Value> {
    None
  }
  /// What the page's document holds now (players, frames, play controls, walls), for the sweep.
  #[cfg(feature = "catch-matrix")]
  fn inspect_dom(&self) -> BoxFuture<'_, Option<serde_json::Value>> {
    Box::pin(async { None })
  }
}

pub(super) trait Backend: Send + Sync + 'static {
  /// Whether every address of the link's host is public.
  fn admit<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, bool>;
  fn open<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Option<Arc<dyn Page>>>;
  fn emit(&self, state: CatchAutoState);
  fn count(&self, event: &'static str, page: &Url);
}

struct Job {
  cancel: watch::Sender<bool>,
  beside: bool,
}

struct Prepared {
  snapshot: Arc<dyn Snapshot>,
  plan: Plan,
  made: Instant,
  /// This snapshot came from the one allowed second look.
  rerun: bool,
}

struct Queued {
  plan: Plan,
  rerun: bool,
}

#[derive(Default)]
struct State {
  jobs: HashMap<String, Job>,
  saving: HashSet<String>,
  /// Oldest first.
  snapshots: Vec<(String, Prepared)>,
  /// Retry state belongs to the queued download and outlives the source row or offer.
  downloads: Vec<(String, Queued)>,
}

enum Outcome {
  Found {
    snapshot: Arc<dyn Snapshot>,
    state: CatchAutoState,
  },
  Ended(AutoPhase),
}

struct Captured {
  candidate: Candidate,
  title: Option<String>,
  pictures: Vec<Url>,
  snapshot: Arc<dyn Snapshot>,
  // A stream that never played has already passed the format probe while its page was open.
  probed: Option<Probed>,
}

#[derive(Clone)]
pub(crate) struct AutoCatch(Arc<Inner>);

struct Inner {
  backend: Arc<dyn Backend>,
  /// One hidden page at a time; the semaphore queues the rest in order.
  slot: Semaphore,
  state: Mutex<State>,
}

struct PendingDownload<'a> {
  engine: &'a Inner,
  id: &'a str,
  committed: bool,
}

impl Drop for PendingDownload<'_> {
  fn drop(&mut self) {
    if !self.committed {
      self
        .engine
        .state()
        .downloads
        .retain(|(download_id, _)| download_id != self.id);
    }
  }
}

struct Saving<'a> {
  engine: &'a Inner,
  group: &'a str,
}

impl Drop for Saving<'_> {
  fn drop(&mut self) {
    self.engine.state().saving.remove(self.group);
  }
}

async fn cancelled(cancel: &mut watch::Receiver<bool>) {
  while !*cancel.borrow_and_update() {
    if cancel.changed().await.is_err() {
      std::future::pending::<()>().await;
    }
  }
}

/// Callers are engine tasks already on a runtime; anything else gets Tauri's.
fn spawn(session: impl std::future::Future<Output = ()> + Send + 'static) {
  match tokio::runtime::Handle::try_current() {
    Ok(runtime) => drop(runtime.spawn(session)),
    Err(_) => drop(tauri::async_runtime::spawn(session)),
  }
}

impl AutoCatch {
  pub(super) fn new(backend: Arc<dyn Backend>) -> Self {
    Self(Arc::new(Inner {
      backend,
      slot: Semaphore::new(1),
      state: Mutex::new(State::default()),
    }))
  }

  /// Starts a session for the group in the background.
  pub(super) fn start(&self, group: String, plan: Plan) {
    let Some(cancel) = self.0.begin_session(&group, &plan) else {
      return;
    };
    self
      .0
      .emit(&group, plan.beside.is_some(), AutoPhase::Waiting);
    let inner = self.0.clone();
    spawn(async move {
      inner.run_session(&group, plan, false, true, cancel).await;
    });
  }

  /// One whole session for the developer's sweep; `None` when the link was refused.
  #[cfg(feature = "catch-matrix")]
  pub(super) async fn look_once(&self, group: &str, plan: Plan) -> Option<AutoPhase> {
    self.0.session(group, plan, false).await
  }

  fn stop(&self) {
    let mut state = self.0.state();
    // Mark every receiver cancelled before closing the semaphore wakes its waiters.
    for job in state.jobs.values() {
      let _ = job.cancel.send(true);
    }
    self.0.slot.close();
    state.snapshots.clear();
    state.downloads.clear();
  }

  /// Stops the group's waiting or running session and forgets its snapshot.
  pub(crate) fn cancel(&self, group: &str) {
    let mut state = self.0.state();
    state.snapshots.retain(|(owner, _)| owner != group);
    if let Some(job) = state.jobs.get(group) {
      let _ = job.cancel.send(true);
    }
  }

  /// The user acted on the group's embedded result: a session beside it has nothing left to offer.
  pub(super) fn acted(&self, group: &str) {
    let mut state = self.0.state();
    if let Some(job) = state.jobs.get(group).filter(|job| job.beside) {
      let _ = job.cancel.send(true);
    }
    let count = state.snapshots.len();
    state
      .snapshots
      .retain(|(owner, prepared)| owner != group || prepared.plan.beside.is_none());
    let discarded = state.snapshots.len() != count;
    drop(state);
    if discarded {
      self.0.emit(group, true, AutoPhase::Cancelled);
    }
  }

  pub(super) async fn download(
    &self,
    group: &str,
    format_id: Option<String>,
  ) -> Result<String, &'static str> {
    self.0.download(group, format_id).await
  }

  /// A queued download of a snapshot answered 403: look once more, then the page needs a step.
  pub(super) fn download_forbidden(&self, download_id: &str) {
    let inner = self.0.clone();
    let queued = {
      let mut state = inner.state();
      let Some(at) = state.downloads.iter().position(|(id, _)| id == download_id) else {
        return;
      };
      state.downloads.remove(at).1
    };
    if queued.rerun {
      inner.emit(download_id, false, AutoPhase::NeedsStep);
    } else {
      let download_id = download_id.to_owned();
      spawn(async move {
        inner.session(&download_id, queued.plan, true).await;
      });
    }
  }
}

fn needs_step_phase(beside: bool) -> AutoPhase {
  if beside {
    AutoPhase::Cancelled
  } else {
    AutoPhase::NeedsStep
  }
}

impl Inner {
  fn state(&self) -> MutexGuard<'_, State> {
    self.state.lock().unwrap_or_else(|error| error.into_inner())
  }

  fn emit(&self, group: &str, beside: bool, phase: AutoPhase) {
    self.backend.emit(CatchAutoState {
      group_id: group.to_owned(),
      phase,
      beside,
      title: None,
      duration_sec: None,
      thumbnail: None,
      kind: None,
      formats: None,
    });
  }

  fn begin_session(&self, group: &str, plan: &Plan) -> Option<watch::Receiver<bool>> {
    let mut state = self.state();
    if self.slot.is_closed() || state.jobs.contains_key(group) {
      return None;
    }
    let (send, cancel) = watch::channel(false);
    state.snapshots.retain(|(owner, _)| owner != group);
    state.jobs.insert(
      group.to_owned(),
      Job {
        cancel: send,
        beside: plan.beside.is_some(),
      },
    );
    Some(cancel)
  }

  async fn session(&self, group: &str, plan: Plan, rerun: bool) -> Option<AutoPhase> {
    let cancel = self.begin_session(group, &plan)?;
    self.run_session(group, plan, rerun, false, cancel).await
  }

  async fn run_session(
    &self,
    group: &str,
    plan: Plan,
    rerun: bool,
    waiting_shown: bool,
    mut cancel: watch::Receiver<bool>,
  ) -> Option<AutoPhase> {
    let beside = plan.beside.is_some();
    let mut outcome = tokio::select! {
      biased;
      _ = cancelled(&mut cancel) => Some(Outcome::Ended(AutoPhase::Cancelled)),
      admitted = self.backend.admit(&plan.url) => {
        if admitted {
          Some(self.run(group, &plan, waiting_shown, &mut cancel).await)
        } else if rerun {
          Some(Outcome::Ended(needs_step_phase(beside)))
        } else if waiting_shown {
          Some(Outcome::Ended(AutoPhase::Cancelled))
        } else {
          None
        }
      },
    };
    let mut state = self.state();
    state.jobs.remove(group);
    // A cancel that landed as the probe answered wins: nothing is kept or shown as found.
    if *cancel.borrow() {
      outcome = Some(Outcome::Ended(AutoPhase::Cancelled));
    }
    let phase = match outcome? {
      Outcome::Found {
        snapshot,
        state: found,
      } => {
        if state.snapshots.len() >= MAX_SNAPSHOTS {
          state.snapshots.remove(0);
        }
        state.snapshots.push((
          group.to_owned(),
          Prepared {
            snapshot,
            plan: plan.clone(),
            made: Instant::now(),
            rerun,
          },
        ));
        drop(state);
        self.backend.count(CATCH_AUTO_FOUND, &plan.url);
        self.backend.emit(found);
        return Some(AutoPhase::Found);
      }
      Outcome::Ended(AutoPhase::Cancelled) => AutoPhase::Cancelled,
      // A second look that finds nothing leaves the row with "Open the page".
      Outcome::Ended(_) if rerun => needs_step_phase(beside),
      // Beside an embedded result, nothing better found means that result just stays.
      Outcome::Ended(_) if beside => AutoPhase::Cancelled,
      Outcome::Ended(phase) => phase,
    };
    drop(state);
    self.emit(group, beside, phase);
    Some(phase)
  }

  async fn run(
    &self,
    group: &str,
    plan: &Plan,
    waiting_shown: bool,
    cancel: &mut watch::Receiver<bool>,
  ) -> Outcome {
    let beside = plan.beside.is_some();
    let permit = match self.slot.try_acquire() {
      Ok(permit) => permit,
      Err(_) => {
        if !waiting_shown {
          self.emit(group, beside, AutoPhase::Waiting);
        }
        tokio::select! {
          biased;
          _ = cancelled(cancel) => return Outcome::Ended(AutoPhase::Cancelled),
          permit = self.slot.acquire() => match permit {
            Ok(permit) => permit,
            Err(_) => return Outcome::Ended(AutoPhase::Cancelled),
          },
        }
      }
    };
    if *cancel.borrow() || self.slot.is_closed() {
      return Outcome::Ended(AutoPhase::Cancelled);
    }
    self.emit(group, beside, AutoPhase::Looking);
    self.backend.count(CATCH_AUTO_USED, &plan.url);
    let Some(page) = self.backend.open(&plan.url).await else {
      return Outcome::Ended(AutoPhase::NeedsStep);
    };
    // One cap for the page and the format probe after it.
    let deadline = Instant::now() + HARD_CAP;
    let caught = tokio::select! {
      biased;
      _ = cancelled(cancel) => Err(AutoPhase::Cancelled),
      _ = tokio::time::sleep_until(deadline) => Err(AutoPhase::NeedsStep),
      caught = self.look(page.as_ref(), plan, deadline) => caught,
    };
    // The page is gone before the slower format probe, and the next session may start.
    page.close().await;
    drop(permit);
    let Captured {
      candidate,
      title,
      pictures,
      snapshot,
      probed,
    } = match caught {
      Ok(caught) => caught,
      Err(phase) => {
        if phase == AutoPhase::Encrypted {
          // The final verdict, after the Mobile version was tried too.
          self.backend.count(CATCH_DRM, &plan.url);
        }
        return Outcome::Ended(phase);
      }
    };
    // The picture is fetched beside the format probe; it never holds the row back for long.
    let thumbnail = async {
      tokio::time::timeout(THUMBNAIL_WAIT, snapshot.thumbnail(pictures))
        .await
        .ok()
        .flatten()
    };
    let probe = async {
      match probed {
        Some(probed) => Ok(probed),
        None => snapshot.probe().await,
      }
    };
    let (probed, thumbnail) = tokio::select! {
      biased;
      _ = cancelled(cancel) => return Outcome::Ended(AutoPhase::Cancelled),
      _ = tokio::time::sleep_until(deadline) => return Outcome::Ended(AutoPhase::NeedsStep),
      both = futures::future::join(probe, thumbnail) => both,
    };
    let probed = match probed {
      Ok(probed) => probed,
      Err(Fail::Encrypted) => {
        self.backend.count(CATCH_DRM, &plan.url);
        return Outcome::Ended(AutoPhase::Encrypted);
      }
      Err(Fail::Live) => return Outcome::Ended(AutoPhase::Live),
      Err(_) => return Outcome::Ended(AutoPhase::NeedsStep),
    };
    Outcome::Found {
      snapshot,
      state: CatchAutoState {
        group_id: group.to_owned(),
        phase: AutoPhase::Found,
        beside,
        title,
        duration_sec: candidate.duration.or(probed.duration),
        thumbnail,
        kind: Some(candidate.kind),
        formats: Some(probed.formats),
      },
    }
  }

  /// Watches the page until it shows a main video, and takes what its download needs while
  /// the page is still there.
  async fn look(
    &self,
    page: &dyn Page,
    plan: &Plan,
    deadline: Instant,
  ) -> Result<Captured, AutoPhase> {
    let mut started = Instant::now();
    let mut mobile_tried = false;
    let mut host = plan.url.host_str().map(str::to_owned);
    let mut pinned: Option<String> = None;
    let mut first_found: Option<(String, Instant)> = None;
    let mut geo_since: Option<Instant> = None;
    let mut hops = 0;
    loop {
      tokio::time::sleep(TICK).await;
      let mut view = page.view().ok_or(AutoPhase::NeedsStep)?;
      let timed_out = started.elapsed() >= LOOK;
      if timed_out
        && !view.loading
        && !matches!(view.phase, Phase::Drm | Phase::Live | Phase::Crashed)
      {
        let fresh = page.final_view().await.ok_or(AutoPhase::NeedsStep)?;
        if fresh.top != view.top || fresh.loading {
          return Err(AutoPhase::NeedsStep);
        }
        view = fresh;
      }
      if let Some(top) = &view.top {
        if top.host_str() != host.as_deref() {
          // A redirect: the new host is checked like the pasted one, a few times at most.
          hops += 1;
          if hops > MAX_HOPS || !may_be_public(top) || !self.backend.admit(top).await {
            return Err(AutoPhase::NeedsStep);
          }
          host = top.host_str().map(str::to_owned);
        }
        let site = site_of(top);
        match &pinned {
          // Leaving the site after its page loaded is an ad redirect, not the video.
          Some(pinned) if site.as_ref() != Some(pinned) => return Err(AutoPhase::NeedsStep),
          None if !view.loading => pinned = site,
          _ => {}
        }
      }
      match view.phase {
        // Try harder first: once, the Mobile version, with the look's time started again
        // (the session's hard cap still ends it).
        Phase::Drm if !mobile_tried => {
          mobile_tried = true;
          if page.mobile().await {
            started = Instant::now();
            first_found = None;
            continue;
          }
          return Err(AutoPhase::Encrypted);
        }
        Phase::Drm => return Err(AutoPhase::Encrypted),
        Phase::Live => return Err(AutoPhase::Live),
        Phase::Crashed | Phase::NotFound => return Err(AutoPhase::NeedsStep),
        _ => {}
      }
      if view.phase == Phase::GeoBlocked && !view.loading {
        if geo_since.get_or_insert_with(Instant::now).elapsed() >= GEO_GRACE {
          return Err(AutoPhase::NeedsStep);
        }
      } else {
        geo_since = None;
      }
      let chosen = best(&view.mains, plan.beside.as_ref());
      if chosen.is_none() {
        // What was found is gone: the next find waits its own moment.
        first_found = None;
      }
      let short = chosen
        .as_ref()
        .is_some_and(|c| c.duration.is_some_and(|s| s < 60.0));
      // A short clip first promoted at the cutoff can still be a pre-roll. Give it its own
      // settling window, including when it replaces an earlier offer.
      let settled = chosen.as_ref().is_some_and(|candidate| {
        let (id, since) = first_found.get_or_insert_with(|| (candidate.id.clone(), Instant::now()));
        if short && *id != candidate.id {
          *since = Instant::now();
        }
        id.clone_from(&candidate.id);
        since.elapsed() >= SETTLE
          || (!short
            && (candidate
              .duration
              .is_some_and(|seconds| seconds >= EMBEDDED_MAX_SEC)
              || *since + SETTLE >= deadline
              || timed_out))
      });
      if let Some(candidate) = chosen.filter(|_| settled) {
        let captured = page.capture(&candidate).await;
        if short {
          let fresh = page.final_view().await.ok_or(AutoPhase::NeedsStep)?;
          if fresh.top != view.top || fresh.loading {
            return Err(AutoPhase::NeedsStep);
          }
          if best(&fresh.mains, plan.beside.as_ref()).is_none_or(|c| c.id != candidate.id)
            || matches!(
              fresh.phase,
              Phase::Drm | Phase::Live | Phase::Crashed | Phase::GeoBlocked
            )
          {
            first_found = None;
            continue;
          }
          view = fresh;
        }
        return match captured {
          Ok(snapshot) => Ok(Captured {
            candidate,
            title: view.title,
            pictures: view.pictures,
            snapshot,
            probed: None,
          }),
          Err(Fail::Encrypted) => Err(AutoPhase::Encrypted),
          Err(Fail::Live) => Err(AutoPhase::Live),
          Err(_) => Err(AutoPhase::NeedsStep),
        };
      }
      if timed_out {
        if short && Instant::now() + TICK < deadline {
          continue;
        }
        if let Some(candidate) = view
          .lone
          .as_ref()
          .filter(|c| !view.loading && !mobile_tried && eligible(c, plan.beside.as_ref()))
        {
          match Self::capture_idle(page, &view, candidate, plan.beside.as_ref()).await {
            Ok(caught) => return Ok(caught),
            Err(Fail::Encrypted) => {
              mobile_tried = true;
              if page.mobile().await {
                started = Instant::now();
                first_found = None;
                continue;
              }
              return Err(AutoPhase::Encrypted);
            }
            Err(Fail::Live) => return Err(AutoPhase::Live),
            Err(_) => return Err(AutoPhase::NeedsStep),
          }
        }
        return Err(
          if !mobile_tried
            && view.phase == Phase::Looking
            && !view.loading
            && view.top.is_some()
            && !view.has_player
          {
            AutoPhase::NoVideo
          } else {
            AutoPhase::NeedsStep
          },
        );
      }
    }
  }

  async fn capture_idle(
    page: &dyn Page,
    view: &View,
    candidate: &Candidate,
    beside: Option<&Beside>,
  ) -> Result<Captured, Fail> {
    let snapshot = page.capture(candidate).await?;
    let probed = snapshot.probe().await;
    // The probe can take several seconds. Read any new playback or encryption before we close.
    let fresh = page.final_view().await.ok_or(Fail::Other)?;
    if fresh.top != view.top || fresh.loading {
      return Err(Fail::Other);
    }
    match fresh.phase {
      Phase::Drm => return Err(Fail::Encrypted),
      Phase::Live => return Err(Fail::Live),
      _ => {}
    }
    if fresh.lone.as_ref() != Some(candidate) {
      return Err(Fail::Other);
    }
    let probed = probed?;
    let mut candidate = candidate.clone();
    if let Some(duration) = probed.duration {
      candidate.duration = Some(duration.min(candidate.duration.unwrap_or(duration)));
      if !(60.0..=1.0e9).contains(&duration) || !eligible(&candidate, beside) {
        return Err(Fail::Other);
      }
    }
    Ok(Captured {
      candidate,
      title: fresh.title,
      pictures: fresh.pictures,
      snapshot,
      probed: Some(probed),
    })
  }

  fn prepared(&self, group: &str) -> Option<(Arc<dyn Snapshot>, Plan, bool, bool)> {
    let state = self.state();
    let (_, prepared) = state.snapshots.iter().find(|(owner, _)| owner == group)?;
    Some((
      prepared.snapshot.clone(),
      prepared.plan.clone(),
      prepared.made.elapsed() < SNAPSHOT_TTL,
      prepared.rerun,
    ))
  }

  fn queued(&self, download_id: &str, plan: &Plan, rerun: bool) {
    let mut state = self.state();
    if state.downloads.len() >= MAX_DOWNLOADS {
      state.downloads.remove(0);
    }
    let plan = Plan {
      url: plan.url.clone(),
      // On the download's own row the capture is its primary result.
      beside: None,
    };
    state
      .downloads
      .push((download_id.to_owned(), Queued { plan, rerun }));
  }

  async fn save_snapshot(
    &self,
    snapshot: &dyn Snapshot,
    format_id: Option<String>,
    plan: &Plan,
    rerun: bool,
  ) -> Result<String, Fail> {
    let id = uuid::Uuid::new_v4().to_string();
    // The dispatcher can report failure as soon as the request is sent.
    self.queued(&id, plan, rerun);
    let mut pending = PendingDownload {
      engine: self,
      id: &id,
      committed: false,
    };
    snapshot.save(format_id, id.clone()).await?;
    pending.committed = true;
    drop(pending);
    Ok(id)
  }

  fn needs_step(&self, group: &str, beside: bool) -> &'static str {
    self.state().snapshots.retain(|(owner, _)| owner != group);
    self.emit(group, beside, needs_step_phase(beside));
    "catchAutoNeedsStep"
  }

  async fn download(&self, group: &str, format_id: Option<String>) -> Result<String, &'static str> {
    let _saving = {
      let mut state = self.state();
      if state.jobs.contains_key(group) || !state.saving.insert(group.to_owned()) {
        return Err("catchAutoBusy");
      }
      Saving {
        engine: self,
        group,
      }
    };
    let (snapshot, plan, fresh, rerun) = self.prepared(group).ok_or("catchAutoGone")?;
    if fresh {
      match self
        .save_snapshot(snapshot.as_ref(), format_id.clone(), &plan, rerun)
        .await
      {
        Ok(id) => return Ok(id),
        Err(Fail::Forbidden) => {}
        Err(Fail::Encrypted) => return Err("drmProtected"),
        Err(Fail::Live) => return Err("liveStreamUnsupported"),
        Err(Fail::Other) => return Err("catchAutoFailed"),
      }
    }
    let beside = plan.beside.is_some();
    // Expired or refused: one more look, then the page needs a step from the user.
    if rerun {
      return Err(self.needs_step(group, beside));
    }
    if self.session(group, plan.clone(), true).await != Some(AutoPhase::Found) {
      return Err("catchAutoNeedsStep");
    }
    let (snapshot, ..) = self.prepared(group).ok_or("catchAutoNeedsStep")?;
    match self
      .save_snapshot(snapshot.as_ref(), format_id, &plan, true)
      .await
    {
      Ok(id) => Ok(id),
      Err(_) => Err(self.needs_step(group, beside)),
    }
  }
}

struct AppBackend(AppHandle);

impl Backend for AppBackend {
  fn admit<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, bool> {
    Box::pin(admitted(url))
  }

  fn open<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Option<Arc<dyn Page>>> {
    Box::pin(super::window::hidden::open(&self.0, url))
  }

  fn emit(&self, state: CatchAutoState) {
    // The quick bar and the card read the state from their offer; rows get the event.
    if crate::capture::auto::owns(&state.group_id) {
      crate::capture::auto::changed(&self.0, &state);
    } else {
      let _ = self.0.emit_to(EventTarget::webview("main"), EVENT, state);
    }
  }

  fn count(&self, event: &'static str, page: &Url) {
    crate::usage::count_catch(&self.0, event, page);
  }
}

/// The public-address rule every engine request passes, for the link a hidden page will load.
pub(super) async fn admitted(url: &Url) -> bool {
  let (Some(host), Some(port)) = (crate::net::host(url), url.port_or_known_default()) else {
    return false;
  };
  is_http(url)
    && may_be_public(url)
    && crate::net::admit(host, port, crate::net::public_address)
      .await
      .is_ok()
}

fn wayland() -> bool {
  cfg!(target_os = "linux")
    && (std::env::var_os("WAYLAND_DISPLAY").is_some()
      || std::env::var("XDG_SESSION_TYPE").is_ok_and(|kind| kind == "wayland"))
}

/// WKWebView takes a proxy from macOS 14 (`nw_proxy_config`); WebView2 and WebKitGTK always do.
/// An engine that cannot be told to leave WebRTC alone would connect beside the route.
fn unguarded() -> bool {
  #[cfg(target_os = "macos")]
  {
    !super::window::hidden::closes_webrtc() || !super::native::use_persistent_profile()
  }
  #[cfg(not(target_os = "macos"))]
  {
    !super::window::hidden::closes_webrtc()
  }
}

pub(crate) fn setup(app: &AppHandle) {
  #[cfg(windows)]
  super::window::hidden::sweep_profiles(app);
  app.manage(super::window::hidden::HiddenWindows::default());
  let auto = AutoCatch::new(Arc::new(AppBackend(app.clone())));
  crate::capture::auto::bind(app, auto.clone());
  app.manage(auto);
}

pub(crate) fn stop_all(app: &AppHandle) {
  if let Some(auto) = app.try_state::<AutoCatch>() {
    auto.stop();
  }
  super::window::hidden::stop_all(app);
}

/// Called where the read of a pasted link ends. Starts a hidden session when the rules allow.
pub(crate) fn consider(app: &AppHandle, group_id: &str, url: &str, read: Read<'_>) {
  let Some(auto) = app.try_state::<AutoCatch>() else {
    return;
  };
  let gate = Gate {
    setting: app
      .try_state::<crate::SharedConfig>()
      .is_some_and(|config| config.load().input.auto_catch),
    proxied: crate::net::proxied(),
    wayland: wayland(),
    unguarded: unguarded(),
  };
  if let Some(plan) = plan(url, &read, gate) {
    auto.start(group_id.to_owned(), plan);
  }
}

/// The group was cancelled or removed.
pub(crate) fn forget(app: &AppHandle, group_id: &str) {
  if let Some(auto) = app.try_state::<AutoCatch>() {
    auto.cancel(group_id);
  }
}

/// The user started a download from the group's own (embedded) result.
pub(crate) fn acted(app: &AppHandle, group_id: &str) {
  if let Some(auto) = app.try_state::<AutoCatch>() {
    auto.acted(group_id);
  }
}

/// A caught download failed with this code.
pub(crate) fn download_failed(app: &AppHandle, download_id: &str, code: &str) {
  if code == "accessForbidden403" {
    if let Some(auto) = app.try_state::<AutoCatch>() {
      auto.download_forbidden(download_id);
    }
  }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutoSaved {
  download_id: String,
}

/// The main window acts on its rows; the quick bar and the card only on their own offers.
fn caller_owns(
  caller: &str,
  group_id: &str,
  capture: Option<&crate::capture::CaptureState>,
) -> Result<(), &'static str> {
  let allowed = if caller == "main" {
    !crate::capture::auto::owns(group_id)
  } else {
    capture.is_some_and(|capture| crate::capture::auto::held_by(capture, caller, group_id))
  };
  allowed.then_some(()).ok_or("catchWrongWindow")
}

#[tauri::command]
pub(crate) async fn catch_auto_cancel(
  app: AppHandle,
  caller: tauri::Webview,
  group_id: String,
) -> Result<(), &'static str> {
  let capture = app.try_state::<crate::capture::CaptureState>();
  caller_owns(caller.label(), &group_id, capture.as_deref())?;
  forget(&app, &group_id);
  Ok(())
}

#[tauri::command]
pub(crate) async fn catch_auto_download(
  app: AppHandle,
  caller: tauri::Webview,
  group_id: String,
  format_id: Option<String>,
) -> Result<AutoSaved, &'static str> {
  let capture = app.try_state::<crate::capture::CaptureState>();
  caller_owns(caller.label(), &group_id, capture.as_deref())?;
  let auto = app.try_state::<AutoCatch>().ok_or("catchAutoGone")?;
  auto
    .download(&group_id, format_id)
    .await
    .map(|download_id| AutoSaved { download_id })
}

#[cfg(test)]
#[path = "auto_tests.rs"]
mod tests;
