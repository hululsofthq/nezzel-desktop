use super::*;
use crate::catch::handoff::FormatLabel;
use std::{
  collections::VecDeque,
  sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

const PAGE: &str = "https://films.example.org/watch/one";
const ON: Gate = Gate {
  setting: true,
  proxied: false,
  wayland: false,
  unguarded: false,
};

struct FakeSnapshot {
  probe: Result<Option<f64>, Fail>,
  results: Mutex<VecDeque<Result<(), Fail>>>,
  saves: Mutex<Vec<Option<String>>>,
  /// The format probe never answers.
  probe_stuck: bool,
  /// Runs inside the probe, just before it answers.
  on_probe: Mutex<Option<Box<dyn FnOnce() + Send>>>,
  save_gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
  forbidden_on_save: Mutex<Option<AutoCatch>>,
}

impl FakeSnapshot {
  fn new(results: Vec<Result<(), Fail>>) -> Arc<Self> {
    Arc::new(Self {
      probe: Ok(None),
      results: Mutex::new(results.into()),
      saves: Mutex::default(),
      probe_stuck: false,
      on_probe: Mutex::default(),
      save_gate: Mutex::default(),
      forbidden_on_save: Mutex::default(),
    })
  }
}

impl Snapshot for FakeSnapshot {
  fn probe(&self) -> BoxFuture<'_, Result<Probed, Fail>> {
    Box::pin(async move {
      if self.probe_stuck {
        std::future::pending::<()>().await;
      }
      if let Some(hook) = self.on_probe.lock().unwrap().take() {
        hook();
      }
      Ok(Probed {
        formats: vec![CatchFormat {
          id: "video-720".into(),
          label: FormatLabel::Video,
          height: Some(720),
          ext: "mp4".into(),
          size_bytes: None,
          best: true,
        }],
        duration: self.probe?,
      })
    })
  }

  fn save(
    &self,
    format_id: Option<String>,
    download_id: String,
  ) -> BoxFuture<'_, Result<(), Fail>> {
    Box::pin(async move {
      self.saves.lock().unwrap().push(format_id);
      let gate = self.save_gate.lock().unwrap().take();
      if let Some(gate) = gate {
        let _ = gate.await;
      }
      if let Some(auto) = self.forbidden_on_save.lock().unwrap().take() {
        auto.download_forbidden(&download_id);
      }
      let next = self.results.lock().unwrap().pop_front();
      next.unwrap_or(Ok(()))
    })
  }
}

struct FakePage {
  view: Mutex<View>,
  final_views: Mutex<VecDeque<View>>,
  snapshot: Arc<FakeSnapshot>,
  /// The cookie read never answers.
  stuck: bool,
  closed: AtomicBool,
  /// Mobile-version reloads asked for, and what the page shows after one.
  mobiles: AtomicUsize,
  as_mobile: Mutex<Option<View>>,
}

impl FakePage {
  fn showing(view: View, snapshot: Arc<FakeSnapshot>) -> Arc<Self> {
    Arc::new(Self {
      view: Mutex::new(view),
      final_views: Mutex::default(),
      snapshot,
      stuck: false,
      closed: AtomicBool::new(false),
      mobiles: AtomicUsize::new(0),
      as_mobile: Mutex::new(None),
    })
  }

  fn empty() -> Arc<Self> {
    Self::showing(
      view(PAGE, Phase::Looking, vec![]),
      FakeSnapshot::new(vec![]),
    )
  }

  fn film(results: Vec<Result<(), Fail>>) -> Arc<Self> {
    Self::showing(
      view(
        PAGE,
        Phase::Found,
        vec![candidate(Some(6738.0), "cdn.example.net")],
      ),
      FakeSnapshot::new(results),
    )
  }

  fn closed(&self) -> bool {
    self.closed.load(Ordering::SeqCst)
  }
}

impl Page for FakePage {
  fn view(&self) -> Option<View> {
    (!self.closed()).then(|| self.view.lock().unwrap().clone())
  }

  fn final_view(&self) -> BoxFuture<'_, Option<View>> {
    Box::pin(async {
      self
        .final_views
        .lock()
        .unwrap()
        .pop_front()
        .or_else(|| self.view())
    })
  }

  fn capture<'a>(
    &'a self,
    _candidate: &'a Candidate,
  ) -> BoxFuture<'a, Result<Arc<dyn Snapshot>, Fail>> {
    Box::pin(async move {
      if self.stuck {
        std::future::pending::<()>().await;
      }
      Ok(self.snapshot.clone() as Arc<dyn Snapshot>)
    })
  }

  fn mobile(&self) -> BoxFuture<'_, bool> {
    Box::pin(async move {
      self.mobiles.fetch_add(1, Ordering::SeqCst);
      if let Some(view) = self.as_mobile.lock().unwrap().take() {
        *self.view.lock().unwrap() = view;
      }
      true
    })
  }

  fn close(&self) -> BoxFuture<'_, ()> {
    Box::pin(async move { self.closed.store(true, Ordering::SeqCst) })
  }
}

#[derive(Default)]
struct Fake {
  events: Mutex<Vec<CatchAutoState>>,
  counts: Mutex<Vec<&'static str>>,
  pages: Mutex<VecDeque<Arc<FakePage>>>,
  opened: Mutex<Vec<Arc<FakePage>>>,
  refused: Mutex<Vec<&'static str>>,
}

impl Fake {
  fn with(pages: Vec<Arc<FakePage>>) -> (Arc<Self>, AutoCatch) {
    let fake = Arc::new(Self {
      pages: Mutex::new(pages.into()),
      ..Self::default()
    });
    (fake.clone(), AutoCatch::new(fake))
  }

  fn phases(&self) -> Vec<(String, AutoPhase)> {
    let events = self.events.lock().unwrap();
    events
      .iter()
      .map(|event| (event.group_id.clone(), event.phase))
      .collect()
  }

  fn phases_of(&self, group: &str) -> Vec<AutoPhase> {
    let phases = self.phases().into_iter();
    phases
      .filter(|(owner, _)| owner == group)
      .map(|(_, phase)| phase)
      .collect()
  }

  fn opened(&self) -> usize {
    self.opened.lock().unwrap().len()
  }
}

impl Backend for Fake {
  fn admit<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, bool> {
    Box::pin(async move {
      !self
        .refused
        .lock()
        .unwrap()
        .contains(&url.host_str().unwrap())
    })
  }

  fn open<'a>(&'a self, _url: &'a Url) -> BoxFuture<'a, Option<Arc<dyn Page>>> {
    Box::pin(async move {
      let page = self.pages.lock().unwrap().pop_front();
      let page = page.unwrap_or_else(FakePage::empty);
      self.opened.lock().unwrap().push(page.clone());
      Some(page as Arc<dyn Page>)
    })
  }

  fn emit(&self, state: CatchAutoState) {
    self.events.lock().unwrap().push(state);
  }

  fn count(&self, event: &'static str, _page: &Url) {
    self.counts.lock().unwrap().push(event);
  }
}

fn view(top: &str, phase: Phase, mains: Vec<Candidate>) -> View {
  View {
    top: Some(Url::parse(top).unwrap()),
    loading: false,
    title: Some("One".into()),
    phase,
    has_player: !mains.is_empty(),
    mains,
    lone: None,
    pictures: Vec::new(),
  }
}

fn candidate(duration: Option<f64>, site: &str) -> Candidate {
  Candidate {
    id: format!("{site}-{duration:?}"),
    kind: "hls",
    duration,
    loaded_host: 0,
    sites: [Some(site.into()), Some("films.example.org".into())],
  }
}

#[test]
fn a_loaded_host_breaks_duration_ties_without_outweighing_a_longer_film() {
  let film = candidate(Some(5400.0), "native.example.com");
  let mut mse = candidate(Some(120.0), "mse.example.org");
  mse.loaded_host = 2;
  assert_eq!(best(&[film.clone(), mse.clone()], None), Some(film.clone()));
  mse.duration = Some(5399.9);
  assert_eq!(best(&[film, mse.clone()], None), Some(mse));
}

fn failed() -> Plan {
  plan(PAGE, &Read::Failed("urlNotSupported"), ON).unwrap()
}

fn beside(duration: Option<f64>) -> Plan {
  let read = Read::Embedded {
    duration,
    media: Some("https://www.youtube.com/watch?v=trailer"),
  };
  plan(PAGE, &read, ON).unwrap()
}

#[test]
fn unclassified_web_failure_starts_a_session_only_with_the_generic_extractor() {
  for (url, code, generic, starts) in [
    (PAGE, "unknown", true, true),
    (PAGE, "unknown", false, false),
    (PAGE, "noMediaFound", true, true),
    (PAGE, "noMediaFound", false, true),
    (PAGE, "drmProtected", true, false),
    (PAGE, "networkTimeout", true, false),
    (PAGE, "signInRequired", false, false),
    ("https://films.example.org/file.pdf", "unknown", true, false),
    (
      "https://films.example.org/file.DOCX?download=1#top",
      "urlNotSupported",
      false,
      false,
    ),
    (
      "https://films.example.org/file%2Exlsx",
      "noMediaFound",
      false,
      false,
    ),
    (
      "https://films.example.org/watch?name=file.pdf",
      "unknown",
      true,
      true,
    ),
    ("file:///tmp/page.html", "unknown", true, false),
    ("https://127.0.0.1/report", "unknown", true, false),
  ] {
    let read = if generic {
      Read::GenericExtractorFailed(code)
    } else {
      Read::Failed(code)
    };
    assert_eq!(
      plan(url, &read, ON).is_some(),
      starts,
      "{url}: {code}, generic={generic}"
    );
  }
}

#[test]
fn a_session_starts_only_for_catch_codes_and_short_embedded_results() {
  let embedded = |duration| Read::Embedded {
    duration,
    media: Some("https://www.youtube.com/watch?v=trailer"),
  };
  let off = |change: fn(&mut Gate)| {
    let mut gate = ON;
    change(&mut gate);
    gate
  };
  for (label, url, read, gate, starts) in [
    (
      "unsupported site",
      PAGE,
      Read::Failed("urlNotSupported"),
      ON,
      true,
    ),
    (
      "unsupported link",
      PAGE,
      Read::Failed("urlUnsupported"),
      ON,
      true,
    ),
    (
      "unknown link type",
      PAGE,
      Read::Failed("unknownUrlType"),
      ON,
      true,
    ),
    ("no media", PAGE, Read::Failed("noMediaFound"), ON, true),
    (
      "sign-in needed",
      PAGE,
      Read::Failed("signInRequired"),
      ON,
      false,
    ),
    ("region", PAGE, Read::Failed("geoBlocked"), ON, false),
    ("network", PAGE, Read::Failed("networkTimeout"), ON, false),
    ("encrypted", PAGE, Read::Failed("drmProtected"), ON, false),
    ("short embed", PAGE, embedded(Some(150.0)), ON, true),
    ("embed of unknown length", PAGE, embedded(None), ON, true),
    (
      "embed just under ten minutes",
      PAGE,
      embedded(Some(599.0)),
      ON,
      true,
    ),
    ("ten-minute embed", PAGE, embedded(Some(600.0)), ON, false),
    ("long embed", PAGE, embedded(Some(6738.0)), ON, false),
    (
      "setting off",
      PAGE,
      Read::Failed("urlNotSupported"),
      off(|g| g.setting = false),
      false,
    ),
    (
      "setting off, embed",
      PAGE,
      embedded(None),
      off(|g| g.setting = false),
      false,
    ),
    (
      "proxy set",
      PAGE,
      Read::Failed("urlNotSupported"),
      off(|g| g.proxied = true),
      false,
    ),
    (
      "Wayland",
      PAGE,
      Read::Failed("urlNotSupported"),
      off(|g| g.wayland = true),
      false,
    ),
    (
      "an engine that cannot use the guard route",
      PAGE,
      Read::Failed("urlNotSupported"),
      off(|g| g.unguarded = true),
      false,
    ),
    (
      "loopback",
      "http://127.0.0.1:8080/watch",
      Read::Failed("urlNotSupported"),
      ON,
      false,
    ),
    (
      "private address",
      "http://192.168.1.10/watch",
      Read::Failed("urlNotSupported"),
      ON,
      false,
    ),
    (
      "private IPv6",
      "http://[fd00::1]/watch",
      Read::Failed("urlNotSupported"),
      ON,
      false,
    ),
    (
      "localhost",
      "http://localhost/watch",
      Read::Failed("urlNotSupported"),
      ON,
      false,
    ),
    (
      "local name",
      "http://nas.local/watch",
      Read::Failed("urlNotSupported"),
      ON,
      false,
    ),
    (
      "bare name",
      "http://router/watch",
      Read::Failed("urlNotSupported"),
      ON,
      false,
    ),
    (
      "not a web link",
      "ftp://films.example.org/one",
      Read::Failed("urlNotSupported"),
      ON,
      false,
    ),
    (
      "credentials in link",
      "https://a:b@films.example.org/",
      Read::Failed("urlNotSupported"),
      ON,
      false,
    ),
  ] {
    assert_eq!(plan(url, &read, gate).is_some(), starts, "{label}");
  }
  assert_eq!(failed().beside, None);
  assert_eq!(
    beside(Some(150.0)).beside,
    Some(Beside {
      duration: Some(150.0),
      site: Some("youtube.com".into()),
    })
  );
}

#[tokio::test]
async fn a_link_to_a_private_address_is_never_admitted() {
  for url in [
    "http://127.0.0.1/watch",
    "http://10.0.0.5/watch",
    "http://[::1]/watch",
    "http://localhost/watch",
  ] {
    assert!(!admitted(&Url::parse(url).unwrap()).await, "{url}");
  }
  assert!(admitted(&Url::parse("https://93.184.216.34/watch").unwrap()).await);
}

#[tokio::test(start_paused = true)]
async fn a_host_that_is_not_public_starts_nothing() {
  let (fake, auto) = Fake::with(vec![]);
  fake.refused.lock().unwrap().push("films.example.org");
  assert_eq!(auto.0.session("g", failed(), false).await, None);
  assert!(fake.phases().is_empty());
  assert_eq!(fake.opened(), 0);
  assert!(fake.counts.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_pending_admission_shows_waiting_and_refusal_restores_the_read_error() {
  let (fake, auto) = Fake::with(vec![]);
  fake.refused.lock().unwrap().push("films.example.org");
  auto.start("g".into(), failed());
  assert_eq!(fake.phases_of("g"), [AutoPhase::Waiting]);
  tokio::task::yield_now().await;
  assert_eq!(
    fake.phases_of("g"),
    [AutoPhase::Waiting, AutoPhase::Cancelled]
  );
  assert_eq!(fake.opened(), 0);
}

#[tokio::test(start_paused = true)]
async fn the_second_session_waits_and_runs_after_the_first() {
  let (fake, auto) = Fake::with(vec![FakePage::empty(), FakePage::empty()]);
  auto.start("first".into(), failed());
  auto.start("second".into(), failed());
  tokio::time::sleep(Duration::from_secs(1)).await;
  assert_eq!(
    fake.phases(),
    [
      ("first".into(), AutoPhase::Waiting),
      ("second".into(), AutoPhase::Waiting),
      ("first".into(), AutoPhase::Looking),
    ]
  );
  assert_eq!(fake.opened(), 1);

  tokio::time::sleep(LOOK).await;
  assert_eq!(
    fake.phases()[3..],
    [
      ("first".into(), AutoPhase::NoVideo),
      ("second".into(), AutoPhase::Looking),
    ]
  );
  // The second page opened only once the first was gone.
  let opened = fake.opened.lock().unwrap().clone();
  assert_eq!(opened.len(), 2);
  assert!(opened[0].closed() && !opened[1].closed());

  tokio::time::sleep(LOOK).await;
  assert_eq!(fake.phases_of("second").last(), Some(&AutoPhase::NoVideo));
  assert!(opened[1].closed());
}

#[tokio::test(start_paused = true)]
async fn a_page_that_shows_nothing_ends_with_no_video_after_the_look_time() {
  let (fake, auto) = Fake::with(vec![FakePage::empty()]);
  let started = Instant::now();
  let phase = auto.0.session("g", failed(), false).await;
  assert_eq!(phase, Some(AutoPhase::NoVideo));
  assert!((LOOK..LOOK + Duration::from_secs(1)).contains(&started.elapsed()));
  assert_eq!(
    fake.phases_of("g"),
    [AutoPhase::Looking, AutoPhase::NoVideo]
  );
  assert!(fake.opened.lock().unwrap()[0].closed());
  assert_eq!(*fake.counts.lock().unwrap(), [CATCH_AUTO_USED]);
  assert_eq!(auto.download("g", None).await, Err("catchAutoGone"));
}

#[tokio::test(start_paused = true)]
async fn the_final_document_distinguishes_a_text_page_from_an_idle_player() {
  for (has_player, expected) in [(false, AutoPhase::NoVideo), (true, AutoPhase::NeedsStep)] {
    let page = FakePage::empty();
    page.final_views.lock().unwrap().push_back(View {
      has_player,
      ..view(PAGE, Phase::Looking, vec![])
    });
    let (fake, auto) = Fake::with(vec![page]);
    assert_eq!(auto.0.session("g", failed(), false).await, Some(expected));
    assert_eq!(fake.phases_of("g").last(), Some(&expected));
  }
}

#[tokio::test(start_paused = true)]
async fn a_lone_offer_respects_beside_and_the_latest_encryption_or_live_evidence() {
  let mut idle = view(PAGE, Phase::Looking, vec![]);
  idle.has_player = true;
  idle.lone = Some(candidate(Some(120.0), "cdn.example.net"));
  let mut same_site = beside(Some(90.0));
  same_site.beside.as_mut().unwrap().site = Some("cdn.example.net".into());
  for (label, plan, latest, probe, expected, mobiles) in [
    (
      "clear after the look",
      failed(),
      Phase::Looking,
      Ok(None),
      AutoPhase::Found,
      0,
    ),
    (
      "longer embedded result",
      beside(Some(150.0)),
      Phase::Looking,
      Ok(None),
      AutoPhase::Cancelled,
      0,
    ),
    (
      "shorter embedded result",
      beside(Some(90.0)),
      Phase::Looking,
      Ok(None),
      AutoPhase::Found,
      0,
    ),
    (
      "same embedded site",
      same_site,
      Phase::Looking,
      Ok(None),
      AutoPhase::Cancelled,
      0,
    ),
    (
      "probe measures a short clip",
      failed(),
      Phase::Looking,
      Ok(Some(8.0)),
      AutoPhase::NeedsStep,
      0,
    ),
    (
      "late EME",
      failed(),
      Phase::Drm,
      Ok(None),
      AutoPhase::NeedsStep,
      1,
    ),
    (
      "probe discovers encryption",
      failed(),
      Phase::Looking,
      Err(Fail::Encrypted),
      AutoPhase::NeedsStep,
      1,
    ),
    (
      "probe discovers live",
      failed(),
      Phase::Looking,
      Err(Fail::Live),
      AutoPhase::Live,
      0,
    ),
  ] {
    let mut snapshot = FakeSnapshot::new(vec![]);
    Arc::get_mut(&mut snapshot).unwrap().probe = probe;
    let page = FakePage::showing(view(PAGE, Phase::Looking, vec![]), snapshot);
    *page.final_views.lock().unwrap() = [
      idle.clone(),
      View {
        phase: latest,
        ..idle.clone()
      },
    ]
    .into();
    let (fake, auto) = Fake::with(vec![page.clone()]);
    let started = Instant::now();
    assert_eq!(
      auto.0.session("g", plan, false).await,
      Some(expected),
      "{label}"
    );
    assert!(started.elapsed() >= LOOK, "{label}");
    assert_eq!(page.mobiles.load(Ordering::SeqCst), mobiles, "{label}");
    assert_eq!(fake.phases_of("g").last(), Some(&expected), "{label}");
  }
}

#[tokio::test(start_paused = true)]
async fn a_page_that_hangs_is_gone_at_the_hard_cap() {
  let stuck = Arc::new(FakePage {
    view: Mutex::new(view(
      PAGE,
      Phase::Found,
      vec![candidate(Some(6738.0), "cdn.example.net")],
    )),
    final_views: Mutex::default(),
    snapshot: FakeSnapshot::new(vec![]),
    stuck: true,
    closed: AtomicBool::new(false),
    mobiles: AtomicUsize::new(0),
    as_mobile: Mutex::new(None),
  });
  let (fake, auto) = Fake::with(vec![stuck.clone()]);
  let started = Instant::now();
  let phase = auto.0.session("g", failed(), false).await;
  assert_eq!(phase, Some(AutoPhase::NeedsStep));
  assert_eq!(started.elapsed(), HARD_CAP);
  assert!(stuck.closed());
  assert_eq!(fake.phases_of("g").last(), Some(&AutoPhase::NeedsStep));
}

#[tokio::test(start_paused = true)]
async fn a_format_probe_that_hangs_ends_at_the_same_hard_cap() {
  let mut snapshot = FakeSnapshot::new(vec![]);
  Arc::get_mut(&mut snapshot).unwrap().probe_stuck = true;
  let page = FakePage::showing(
    view(
      PAGE,
      Phase::Found,
      vec![candidate(Some(6738.0), "cdn.example.net")],
    ),
    snapshot,
  );
  let (fake, auto) = Fake::with(vec![page]);
  let started = Instant::now();
  // Bounded: without the shared cap this session would wait forever.
  let phase = tokio::time::timeout(HARD_CAP * 2, auto.0.session("g", failed(), false))
    .await
    .expect("the probe is not bounded by the hard cap");
  assert_eq!(phase, Some(AutoPhase::NeedsStep));
  assert_eq!(started.elapsed(), HARD_CAP);
  assert_eq!(fake.phases_of("g").last(), Some(&AutoPhase::NeedsStep));
}

#[tokio::test(start_paused = true)]
async fn a_cancel_that_lands_as_the_probe_answers_is_not_found() {
  let page = FakePage::film(vec![]);
  let (fake, auto) = Fake::with(vec![page.clone()]);
  let inner = auto.0.clone();
  *page.snapshot.on_probe.lock().unwrap() = Some(Box::new(move || AutoCatch(inner).cancel("g")));
  assert_eq!(
    auto.0.session("g", failed(), false).await,
    Some(AutoPhase::Cancelled)
  );
  assert_eq!(
    fake.phases_of("g"),
    [AutoPhase::Looking, AutoPhase::Cancelled]
  );
  assert_eq!(auto.download("g", None).await, Err("catchAutoGone"));
}

#[tokio::test(start_paused = true)]
async fn cancel_ends_a_running_and_a_waiting_session_and_destroys_the_page() {
  let (fake, auto) = Fake::with(vec![FakePage::empty()]);
  auto.start("running".into(), failed());
  auto.start("waiting".into(), failed());
  tokio::time::sleep(Duration::from_secs(2)).await;
  auto.cancel("waiting");
  auto.cancel("running");
  tokio::time::sleep(Duration::from_millis(10)).await;
  assert_eq!(
    fake.phases_of("running"),
    [AutoPhase::Waiting, AutoPhase::Looking, AutoPhase::Cancelled]
  );
  assert_eq!(
    fake.phases_of("waiting"),
    [AutoPhase::Waiting, AutoPhase::Cancelled]
  );
  let opened = fake.opened.lock().unwrap().clone();
  assert_eq!(opened.len(), 1, "the waiting session never opened a page");
  assert!(opened[0].closed());
  // The slot is free again.
  drop(opened);
  assert_eq!(
    auto.0.session("next", failed(), false).await,
    Some(AutoPhase::NoVideo)
  );
}

#[tokio::test(start_paused = true)]
async fn shutdown_closes_the_active_page_and_never_starts_another() {
  let (fake, auto) = Fake::with(vec![FakePage::film(vec![]), FakePage::empty()]);
  assert_eq!(
    auto.0.session("found", failed(), false).await,
    Some(AutoPhase::Found)
  );
  auto.start("running".into(), failed());
  auto.start("waiting".into(), failed());
  tokio::time::sleep(Duration::from_secs(2)).await;
  assert_eq!(fake.phases_of("waiting"), [AutoPhase::Waiting]);
  auto.stop();
  assert_eq!(auto.0.session("new", failed(), false).await, None);
  tokio::time::sleep(Duration::from_millis(10)).await;
  assert_eq!(fake.opened(), 2);
  assert!(fake.opened.lock().unwrap()[1].closed());
  assert_eq!(auto.download("found", None).await, Err("catchAutoGone"));
}

#[tokio::test(start_paused = true)]
async fn a_found_video_downloads_from_its_snapshot_not_from_the_page_link() {
  let page = FakePage::film(vec![]);
  let (fake, auto) = Fake::with(vec![page.clone()]);
  assert_eq!(
    auto.0.session("g", failed(), false).await,
    Some(AutoPhase::Found)
  );
  assert!(page.closed(), "the page is gone before the row is ready");
  let found = fake.events.lock().unwrap().last().cloned().unwrap();
  assert_eq!(
    serde_json::to_value(&found).unwrap(),
    serde_json::json!({
      "groupId": "g",
      "phase": "found",
      "beside": false,
      "title": "One",
      "durationSec": 6738.0,
      "kind": "hls",
      "formats": [{ "id": "video-720", "label": "video", "height": 720, "ext": "mp4", "best": true }],
    })
  );
  assert_eq!(
    *fake.counts.lock().unwrap(),
    [CATCH_AUTO_USED, CATCH_AUTO_FOUND]
  );

  assert!(auto.download("g", Some("video-720".into())).await.is_ok());
  assert_eq!(
    *page.snapshot.saves.lock().unwrap(),
    [Some("video-720".into())]
  );
  assert_eq!(fake.opened(), 1, "no page is opened to download");
  // A group without a snapshot has nothing to fall back on.
  assert_eq!(auto.download("other", None).await, Err("catchAutoGone"));
  auto.cancel("g");
  assert_eq!(auto.download("g", None).await, Err("catchAutoGone"));
}

#[tokio::test(start_paused = true)]
async fn only_one_save_of_a_snapshot_can_be_in_flight() {
  let page = FakePage::film(vec![]);
  let (_, auto) = Fake::with(vec![page.clone()]);
  auto.0.session("g", failed(), false).await;

  let (_release, gate) = tokio::sync::oneshot::channel();
  *page.snapshot.save_gate.lock().unwrap() = Some(gate);
  let mut first = Box::pin(auto.download("g", None));
  assert!(futures::poll!(&mut first).is_pending());
  assert_eq!(auto.download("g", None).await, Err("catchAutoBusy"));
  drop(first);

  assert!(auto.download("g", None).await.is_ok());
  assert_eq!(page.snapshot.saves.lock().unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_refused_snapshot_looks_once_more_then_the_page_needs_a_step() {
  // The second look finds a working address.
  let first = FakePage::film(vec![Err(Fail::Forbidden)]);
  let second = FakePage::film(vec![]);
  let (fake, auto) = Fake::with(vec![first.clone(), second.clone()]);
  auto.0.session("g", failed(), false).await;
  assert!(auto.download("g", None).await.is_ok());
  assert_eq!(fake.opened(), 2);
  assert_eq!(first.snapshot.saves.lock().unwrap().len(), 1);
  assert_eq!(
    fake.phases_of("g"),
    [
      AutoPhase::Looking,
      AutoPhase::Found,
      AutoPhase::Looking,
      AutoPhase::Found,
    ]
  );

  // The second look is refused too: no third page.
  let pages = vec![
    FakePage::film(vec![Err(Fail::Forbidden)]),
    FakePage::film(vec![Err(Fail::Forbidden)]),
  ];
  let (fake, auto) = Fake::with(pages);
  auto.0.session("g", failed(), false).await;
  assert_eq!(auto.download("g", None).await, Err("catchAutoNeedsStep"));
  assert_eq!(fake.phases_of("g").last(), Some(&AutoPhase::NeedsStep));
  assert_eq!(auto.download("g", None).await, Err("catchAutoGone"));
  assert_eq!(fake.opened(), 2);

  // The second look finds nothing.
  let (fake, auto) = Fake::with(vec![FakePage::film(vec![Err(Fail::Forbidden)])]);
  auto.0.session("g", failed(), false).await;
  assert_eq!(auto.download("g", None).await, Err("catchAutoNeedsStep"));
  assert_eq!(fake.phases_of("g").last(), Some(&AutoPhase::NeedsStep));
}

#[tokio::test(start_paused = true)]
async fn an_expired_snapshot_looks_again_before_downloading() {
  let first = FakePage::film(vec![]);
  let (fake, auto) = Fake::with(vec![first.clone(), FakePage::film(vec![])]);
  auto.0.session("g", failed(), false).await;
  tokio::time::sleep(SNAPSHOT_TTL).await;
  assert!(auto.download("g", None).await.is_ok());
  assert!(first.snapshot.saves.lock().unwrap().is_empty());
  assert_eq!(fake.opened(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_download_that_ends_in_403_looks_once_more() {
  let retry = FakePage::film(vec![]);
  let (fake, auto) = Fake::with(vec![FakePage::film(vec![]), retry.clone()]);
  auto.0.session("g", failed(), false).await;
  let first = auto.download("g", None).await.unwrap();
  auto.cancel("g");
  auto.download_forbidden("unrelated-download");
  auto.download_forbidden(&first);
  tokio::time::sleep(Duration::from_secs(2)).await;
  assert_eq!(fake.opened(), 2);
  assert_eq!(fake.phases_of(&first).last(), Some(&AutoPhase::Found));
  *retry.snapshot.forbidden_on_save.lock().unwrap() = Some(auto.clone());
  let second = auto.download(&first, None).await.unwrap();
  assert_ne!(first, second);
  auto.cancel(&first);
  // A 403 reported before the queue call returns still belongs to the new row.
  assert_eq!(fake.opened(), 2, "one second look per group");
  assert_eq!(fake.phases_of(&second).last(), Some(&AutoPhase::NeedsStep));
}

#[tokio::test(start_paused = true)]
async fn beside_an_embedded_result_only_a_longer_video_from_another_site_is_found() {
  let trailer = 150.0;
  for (label, mains, found) in [
    (
      "the embed itself",
      vec![candidate(Some(6738.0), "youtube.com")],
      false,
    ),
    (
      "a shorter video",
      vec![candidate(Some(90.0), "cdn.example.net")],
      false,
    ),
    (
      "a video of the same length",
      vec![candidate(Some(trailer), "cdn.example.net")],
      false,
    ),
    (
      "a video of unknown length",
      vec![candidate(None, "cdn.example.net")],
      false,
    ),
    (
      "the full video",
      vec![candidate(Some(6738.0), "cdn.example.net")],
      true,
    ),
    (
      "the full video next to the embed",
      vec![
        candidate(Some(9000.0), "youtube.com"),
        candidate(Some(6738.0), "cdn.example.net"),
      ],
      true,
    ),
  ] {
    let page = FakePage::showing(view(PAGE, Phase::Found, mains), FakeSnapshot::new(vec![]));
    let (fake, auto) = Fake::with(vec![page]);
    let phase = auto.0.session("g", beside(Some(trailer)), false).await;
    let last = fake.events.lock().unwrap().last().cloned().unwrap();
    assert!(last.beside, "{label}");
    if found {
      assert_eq!(phase, Some(AutoPhase::Found), "{label}");
      assert_eq!(last.duration_sec, Some(6738.0), "{label}");
    } else {
      // Nothing better: the embedded row just stays.
      assert_eq!(phase, Some(AutoPhase::Cancelled), "{label}");
      assert_eq!(auto.download("g", None).await, Err("catchAutoGone"));
    }
  }

  // A frame of the embedded site is the embed too, whatever host serves its media.
  let mut framed = candidate(Some(6738.0), "cdn.example.net");
  framed.sites[1] = Some("youtube.com".into());
  let page = FakePage::showing(
    view(PAGE, Phase::Found, vec![framed]),
    FakeSnapshot::new(vec![]),
  );
  let (_, auto) = Fake::with(vec![page]);
  assert_eq!(
    auto.0.session("g", beside(None), false).await,
    Some(AutoPhase::Cancelled)
  );
}

#[tokio::test(start_paused = true)]
async fn a_short_first_video_gives_way_to_a_longer_one_found_soon_after() {
  // The trailer plays first; the film shows its length a few seconds later.
  for (film_after, expected) in [(Some(3), 6738.0), (Some(8), 136.0), (None, 136.0)] {
    let page = FakePage::showing(
      view(
        PAGE,
        Phase::Found,
        vec![candidate(Some(136.0), "trailers.example.net")],
      ),
      FakeSnapshot::new(vec![]),
    );
    if let Some(seconds) = film_after {
      let later = page.clone();
      tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(seconds)).await;
        let film = candidate(Some(6738.0), "cdn.example.net");
        later.view.lock().unwrap().mains.push(film);
      });
    }
    let (fake, auto) = Fake::with(vec![page]);
    assert_eq!(
      auto.0.session("g", failed(), false).await,
      Some(AutoPhase::Found),
      "{film_after:?}"
    );
    let found = fake.events.lock().unwrap().last().cloned().unwrap();
    assert_eq!(found.duration_sec, Some(expected), "{film_after:?}");
  }
}

#[tokio::test(start_paused = true)]
async fn a_preroll_at_the_cutoff_does_not_take_the_film_slot() {
  let page = FakePage::showing(
    view(PAGE, Phase::AdPlaying, vec![]),
    FakeSnapshot::new(vec![]),
  );
  let later = page.clone();
  tokio::spawn(async move {
    let start = Instant::now();
    tokio::time::sleep_until(start + LOOK - Duration::from_secs(1)).await;
    *later.view.lock().unwrap() = view(
      PAGE,
      Phase::Found,
      vec![candidate(Some(10.0), "ads.example.net")],
    );
    tokio::time::sleep_until(start + LOOK + Duration::from_secs(1)).await;
    *later.view.lock().unwrap() = view(
      PAGE,
      Phase::Found,
      vec![candidate(Some(6738.0), "cdn.example.net")],
    );
  });
  let (fake, auto) = Fake::with(vec![page]);
  assert_eq!(
    auto.0.session("g", failed(), false).await,
    Some(AutoPhase::Found)
  );
  assert_eq!(
    fake.events.lock().unwrap().last().unwrap().duration_sec,
    Some(6738.0)
  );
}

#[tokio::test(start_paused = true)]
async fn short_offer_is_rechecked_after_capture() {
  let page = FakePage::showing(
    view(
      PAGE,
      Phase::Found,
      vec![candidate(Some(10.0), "ads.example.net")],
    ),
    FakeSnapshot::new(vec![]),
  );
  let film = view(
    PAGE,
    Phase::Found,
    vec![candidate(Some(6738.0), "cdn.example.net")],
  );
  page.final_views.lock().unwrap().push_back(film);
  let later = page.clone();
  tokio::spawn(async move {
    tokio::time::sleep(SETTLE + TICK + TICK).await;
    *later.view.lock().unwrap() = view(
      PAGE,
      Phase::Found,
      vec![candidate(Some(6738.0), "cdn.example.net")],
    );
  });
  let (fake, auto) = Fake::with(vec![page]);
  assert_eq!(
    auto.0.session("g", failed(), false).await,
    Some(AutoPhase::Found)
  );
  assert_eq!(
    fake.events.lock().unwrap().last().unwrap().duration_sec,
    Some(6738.0)
  );
}

#[tokio::test(start_paused = true)]
async fn a_video_found_after_the_first_one_left_waits_its_own_moment() {
  // A preview shows and goes; much later a trailer plays, and the film a few seconds after it.
  let trailer = || candidate(Some(136.0), "trailers.example.net");
  let page = FakePage::showing(
    view(PAGE, Phase::Found, vec![trailer()]),
    FakeSnapshot::new(vec![]),
  );
  let later = page.clone();
  tokio::spawn(async move {
    let at = |millis| tokio::time::sleep_until(Instant::now() + Duration::from_millis(millis));
    let (gone, again, film) = (at(2250), at(10250), at(13250));
    gone.await;
    later.view.lock().unwrap().mains.clear();
    again.await;
    later.view.lock().unwrap().mains.push(trailer());
    film.await;
    let film = candidate(Some(6738.0), "cdn.example.net");
    later.view.lock().unwrap().mains.push(film);
  });
  let (fake, auto) = Fake::with(vec![page]);
  assert_eq!(
    auto.0.session("g", failed(), false).await,
    Some(AutoPhase::Found)
  );
  let found = fake.events.lock().unwrap().last().cloned().unwrap();
  assert_eq!(found.duration_sec, Some(6738.0));
}

#[tokio::test(start_paused = true)]
async fn a_short_video_found_too_late_to_wait_is_taken_before_the_hard_cap() {
  // Encrypted late, so the Mobile version's look runs past the hard cap; it shows a short
  // video 3 s before the cap, with no time left to wait for a longer one.
  let page = FakePage::showing(
    view(PAGE, Phase::Looking, vec![]),
    FakeSnapshot::new(vec![]),
  );
  *page.as_mobile.lock().unwrap() = Some(view(PAGE, Phase::Looking, vec![]));
  let later = page.clone();
  tokio::spawn(async move {
    let at = |millis| tokio::time::sleep_until(Instant::now() + Duration::from_millis(millis));
    let (encrypted, short) = (at(27750), at(41750));
    encrypted.await;
    later.view.lock().unwrap().phase = Phase::Drm;
    short.await;
    let short = candidate(Some(136.0), "cdn.example.net");
    later.view.lock().unwrap().mains.push(short);
  });
  let (fake, auto) = Fake::with(vec![page.clone()]);
  let started = Instant::now();
  assert_eq!(
    auto.0.session("g", failed(), false).await,
    Some(AutoPhase::Found)
  );
  assert_eq!(page.mobiles.load(Ordering::SeqCst), 1);
  assert!(started.elapsed() < HARD_CAP);
  let found = fake.events.lock().unwrap().last().cloned().unwrap();
  assert_eq!(found.duration_sec, Some(136.0));
}

#[tokio::test(start_paused = true)]
async fn acting_on_the_embedded_row_ends_only_a_session_beside_it() {
  let (fake, auto) = Fake::with(vec![FakePage::empty(), FakePage::empty()]);
  auto.start("embedded".into(), beside(None));
  tokio::time::sleep(Duration::from_secs(2)).await;
  auto.acted("embedded");
  tokio::time::sleep(Duration::from_millis(10)).await;
  assert_eq!(
    fake.phases_of("embedded"),
    [AutoPhase::Waiting, AutoPhase::Looking, AutoPhase::Cancelled]
  );
  assert!(fake.opened.lock().unwrap()[0].closed());

  auto.start("failed".into(), failed());
  tokio::time::sleep(Duration::from_secs(2)).await;
  auto.acted("failed");
  tokio::time::sleep(Duration::from_millis(10)).await;
  assert_eq!(
    fake.phases_of("failed"),
    [AutoPhase::Waiting, AutoPhase::Looking]
  );
}

#[tokio::test(start_paused = true)]
async fn acting_on_the_embedded_row_also_discards_a_found_alternative() {
  let (fake, auto) = Fake::with(vec![FakePage::film(vec![])]);
  assert_eq!(
    auto.0.session("embedded", beside(None), false).await,
    Some(AutoPhase::Found)
  );
  auto.acted("embedded");
  assert_eq!(
    fake.phases_of("embedded").last(),
    Some(&AutoPhase::Cancelled)
  );
  assert_eq!(auto.download("embedded", None).await, Err("catchAutoGone"));
}

#[tokio::test(start_paused = true)]
async fn a_refused_alternative_leaves_the_embedded_result() {
  let pages = vec![
    FakePage::film(vec![Err(Fail::Forbidden)]),
    FakePage::film(vec![Err(Fail::Forbidden)]),
  ];
  let (fake, auto) = Fake::with(pages);
  assert_eq!(
    auto.0.session("embedded", beside(None), false).await,
    Some(AutoPhase::Found)
  );
  assert_eq!(
    auto.download("embedded", None).await,
    Err("catchAutoNeedsStep")
  );
  let event = fake.events.lock().unwrap().last().cloned().unwrap();
  assert_eq!(event.phase, AutoPhase::Cancelled);
  assert!(event.beside);
}

#[tokio::test(start_paused = true)]
async fn a_page_outcome_maps_to_its_row_phase() {
  let off_site = "https://ads.example.com/landing";
  let probe_fails = |fail| {
    let mut snapshot = FakeSnapshot::new(vec![]);
    Arc::get_mut(&mut snapshot).unwrap().probe = Err(fail);
    FakePage::showing(
      view(
        PAGE,
        Phase::Found,
        vec![candidate(Some(6738.0), "cdn.example.net")],
      ),
      snapshot,
    )
  };
  for (label, page, phase) in [
    (
      "encrypted media",
      FakePage::showing(view(PAGE, Phase::Drm, vec![]), FakeSnapshot::new(vec![])),
      AutoPhase::Encrypted,
    ),
    (
      "live stream",
      FakePage::showing(view(PAGE, Phase::Live, vec![]), FakeSnapshot::new(vec![])),
      AutoPhase::Live,
    ),
    (
      "played with no video",
      FakePage::showing(
        view(PAGE, Phase::NotFound, vec![]),
        FakeSnapshot::new(vec![]),
      ),
      AutoPhase::NeedsStep,
    ),
    (
      "crashed",
      FakePage::showing(
        view(PAGE, Phase::Crashed, vec![]),
        FakeSnapshot::new(vec![]),
      ),
      AutoPhase::NeedsStep,
    ),
    (
      "only an ad",
      FakePage::showing(
        view(PAGE, Phase::AdPlaying, vec![]),
        FakeSnapshot::new(vec![]),
      ),
      AutoPhase::NeedsStep,
    ),
    (
      "probe says encrypted",
      probe_fails(Fail::Encrypted),
      AutoPhase::Encrypted,
    ),
    ("probe says live", probe_fails(Fail::Live), AutoPhase::Live),
    (
      "probe refused",
      probe_fails(Fail::Forbidden),
      AutoPhase::NeedsStep,
    ),
  ] {
    let (fake, auto) = Fake::with(vec![page.clone()]);
    assert_eq!(
      auto.0.session("g", failed(), false).await,
      Some(phase),
      "{label}"
    );
    assert_eq!(fake.phases_of("g").last(), Some(&phase), "{label}");
    assert!(page.closed(), "{label}");
  }

  // The page loaded, then the top frame left for another site: an ad redirect.
  let page = FakePage::empty();
  let (fake, auto) = Fake::with(vec![page.clone()]);
  auto.start("g".into(), failed());
  tokio::time::sleep(Duration::from_secs(2)).await;
  *page.view.lock().unwrap() = view(
    off_site,
    Phase::Found,
    vec![candidate(Some(6738.0), "cdn.example.net")],
  );
  tokio::time::sleep(Duration::from_secs(1)).await;
  assert_eq!(
    fake.phases_of("g"),
    [AutoPhase::Waiting, AutoPhase::Looking, AutoPhase::NeedsStep]
  );
  assert!(page.closed());

  // A redirect to a host that is not public, before the first load finished.
  let mut loading = view("http://intranet.example.org/", Phase::Looking, vec![]);
  loading.loading = true;
  let page = FakePage::showing(loading, FakeSnapshot::new(vec![]));
  let (fake, auto) = Fake::with(vec![page.clone()]);
  fake.refused.lock().unwrap().push("intranet.example.org");
  let started = Instant::now();
  assert_eq!(
    auto.0.session("g", failed(), false).await,
    Some(AutoPhase::NeedsStep)
  );
  assert!(started.elapsed() < Duration::from_secs(2));
  assert!(page.closed());
}

#[test]
fn each_window_acts_only_on_its_own_sessions() {
  use crate::capture::{BAR_LABEL, CARD_LABEL};
  for (caller, group, allowed) in [
    ("main", "group-1", true),
    ("main", "capture-offer-3", false),
    (BAR_LABEL, "capture-offer-3", false),
    (CARD_LABEL, "capture-offer-3", false),
    (BAR_LABEL, "group-1", false),
    (CARD_LABEL, "group-1", false),
    (
      "catch-bar-0123456789abcdef0123456789abcdef",
      "group-1",
      false,
    ),
    (
      "catch-0123456789abcdef0123456789abcdef",
      "capture-offer-3",
      false,
    ),
  ] {
    assert_eq!(
      caller_owns(caller, group, None).is_ok(),
      allowed,
      "{caller}: {group}"
    );
  }
}

#[tokio::test(start_paused = true)]
async fn an_encrypted_page_is_tried_once_as_the_mobile_version_first() {
  // Encrypted as a desktop page and as the Mobile version: one retry, then the verdict, counted.
  let page = FakePage::showing(view(PAGE, Phase::Drm, vec![]), FakeSnapshot::new(vec![]));
  let (fake, auto) = Fake::with(vec![page.clone()]);
  assert_eq!(
    auto.0.session("g", failed(), false).await,
    Some(AutoPhase::Encrypted)
  );
  assert_eq!(page.mobiles.load(Ordering::SeqCst), 1);
  assert_eq!(
    fake
      .counts
      .lock()
      .unwrap()
      .iter()
      .filter(|c| **c == CATCH_DRM)
      .count(),
    1
  );

  // The Mobile version plays a clear film: found, nothing counted as encrypted.
  let page = FakePage::showing(view(PAGE, Phase::Drm, vec![]), FakeSnapshot::new(vec![]));
  *page.as_mobile.lock().unwrap() = Some(view(
    PAGE,
    Phase::Found,
    vec![candidate(Some(6738.0), "cdn.example.net")],
  ));
  let (fake, auto) = Fake::with(vec![page.clone()]);
  assert_eq!(
    auto.0.session("g", failed(), false).await,
    Some(AutoPhase::Found)
  );
  assert_eq!(page.mobiles.load(Ordering::SeqCst), 1);
  assert!(!fake.counts.lock().unwrap().contains(&CATCH_DRM));
}

#[tokio::test(start_paused = true)]
async fn an_inconclusive_mobile_retry_needs_a_step() {
  let mut endings = Vec::new();
  for (mobile_phase, desktop_drm_at) in [
    (Phase::NotFound, Duration::ZERO),
    (Phase::Looking, Duration::ZERO),
    (Phase::Looking, LOOK - Duration::from_secs(2)),
  ] {
    let page = FakePage::showing(
      view(
        PAGE,
        if desktop_drm_at.is_zero() {
          Phase::Drm
        } else {
          Phase::Looking
        },
        vec![],
      ),
      FakeSnapshot::new(vec![]),
    );
    *page.as_mobile.lock().unwrap() = Some(view(PAGE, mobile_phase, vec![]));
    if !desktop_drm_at.is_zero() {
      let later = page.clone();
      tokio::spawn(async move {
        tokio::time::sleep(desktop_drm_at).await;
        later.view.lock().unwrap().phase = Phase::Drm;
      });
    }
    let (fake, auto) = Fake::with(vec![page.clone()]);
    endings.push((
      auto.0.session("g", failed(), false).await,
      fake.counts.lock().unwrap().contains(&CATCH_DRM),
    ));
    assert_eq!(page.mobiles.load(Ordering::SeqCst), 1);
  }
  assert_eq!(endings, [(Some(AutoPhase::NeedsStep), false); 3]);
}

#[tokio::test(start_paused = true)]
async fn a_block_page_ends_the_look_only_once_loaded_and_still_there() {
  let block = |loading| View {
    loading,
    ..view(PAGE, Phase::GeoBlocked, vec![])
  };
  // A slow page reads as a block page while it loads, and a moment after: its film is caught.
  let page = FakePage::showing(block(true), FakeSnapshot::new(vec![Ok(())]));
  let later = page.clone();
  tokio::spawn(async move {
    let at = |millis| tokio::time::sleep_until(Instant::now() + Duration::from_millis(millis));
    let (loaded, film) = (at(6250), at(8250));
    loaded.await;
    *later.view.lock().unwrap() = block(false);
    film.await;
    let film = vec![candidate(Some(6738.0), "cdn.example.net")];
    *later.view.lock().unwrap() = view(PAGE, Phase::Found, film);
  });
  let (_fake, auto) = Fake::with(vec![page]);
  assert_eq!(
    auto.0.session("g", failed(), false).await,
    Some(AutoPhase::Found)
  );
  // A real one: loaded and unchanged for the grace time, long before the look's own end.
  let page = FakePage::showing(block(false), FakeSnapshot::new(vec![]));
  let (_fake, auto) = Fake::with(vec![page]);
  let started = Instant::now();
  assert_eq!(
    auto.0.session("g", failed(), false).await,
    Some(AutoPhase::NeedsStep)
  );
  assert!(
    started.elapsed() < Duration::from_secs(5),
    "{:?}",
    started.elapsed()
  );
}
