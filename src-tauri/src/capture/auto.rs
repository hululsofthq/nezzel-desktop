//! Automatic catch for the link in the quick bar or the copied-link card. A capture read has no
//! download group, so its hidden session runs under a group named after the offer, and its
//! phases ride on the offer (`capture_snapshot` → `auto`).

use super::preview::{FailReason, ReadFailure};
use super::state::{CaptureState, Claim, Offers};
use super::window::{wake_window, CaptureWindow};
use crate::catch::auto::{self, CatchAutoState, Read};
use tauri::{AppHandle, Manager};

const GROUP: &str = "capture-offer-";

/// Whether the group belongs to a capture offer (and not to a row of the main window).
pub(crate) fn owns(group_id: &str) -> bool {
  group_id.starts_with(GROUP)
}

fn offer_id(group_id: &str) -> Option<u64> {
  group_id.strip_prefix(GROUP)?.parse().ok()
}

pub(crate) fn held_by(capture: &CaptureState, caller: &str, group_id: &str) -> bool {
  let (Some(window), Some(id)) = (CaptureWindow::from_label(caller), offer_id(group_id)) else {
    return false;
  };
  capture.offers.lock().is_ok_and(|offers| {
    offers
      .holding(window, id)
      .is_some_and(|slot| !matches!(slot.claim, Claim::Done(_)))
  })
}

pub(super) fn group(id: u64) -> String {
  format!("{GROUP}{id}")
}

pub(crate) fn bind(app: &AppHandle, auto: auto::AutoCatch) {
  app.state::<CaptureState>().offers.lock().unwrap().auto = Some(auto);
}

/// How a capture read ended, as far as automatic catch cares.
pub(super) enum Ended {
  Other,
  Failed {
    code: &'static str,
    generic: bool,
  },
  Embedded {
    duration: Option<f64>,
    media: Option<String>,
  },
}

impl Ended {
  pub(super) fn of<D>(result: &Result<Option<(serde_json::Value, D)>, ReadFailure>) -> Self {
    match result {
      Err(ReadFailure {
        code: Some(code),
        generic,
        ..
      }) => Self::Failed {
        code,
        generic: *generic,
      },
      Ok(Some((item, _))) if item["embeddedIn"].is_string() => Self::Embedded {
        duration: item["duration"].as_f64(),
        media: item["url"].as_str().map(str::to_owned),
      },
      _ => Self::Other,
    }
  }

  fn read(&self) -> Option<Read<'_>> {
    Some(match self {
      Self::Other => return None,
      Self::Failed {
        code,
        generic: true,
      } => Read::GenericExtractorFailed(code),
      Self::Failed {
        code,
        generic: false,
      } => Read::Failed(code),
      Self::Embedded { duration, media } => Read::Embedded {
        duration: *duration,
        media: media.as_deref(),
      },
    })
  }

  /// The offer's read is recorded: start a hidden session when the rules allow.
  pub(super) fn start(self, app: &AppHandle, id: u64, url: &str) {
    if let Some(read) = self.read() {
      auto::consider(app, &group(id), url, read);
    }
  }
}

/// Puts the state on the offer it belongs to; false when no window holds that offer any more.
fn record(offers: &mut Offers, state: &CatchAutoState) -> Option<CaptureWindow> {
  let id = offer_id(&state.group_id)?;
  CaptureWindow::ALL.into_iter().find(|window| {
    let slot = offers.slot_mut(*window).as_mut();
    slot
      .filter(|slot| slot.offer.id == id && !matches!(slot.claim, Claim::Done(_)))
      .map(|slot| {
        // Show the capture offer's existing "Open the page" action when a hidden read needs help.
        if state.phase == auto::AutoPhase::NeedsStep && !state.beside && slot.offer.failed {
          slot.offer.reason = Some(FailReason::Unsupported);
        }
        slot.offer.auto = Some(state.clone());
      })
      .is_some()
  })
}

/// A session of a capture offer changed: the window that shows the offer reads it again.
pub(crate) fn changed(app: &AppHandle, state: &CatchAutoState) {
  let Some(capture) = app.try_state::<CaptureState>() else {
    return;
  };
  let window = record(
    &mut capture.offers.lock().unwrap_or_else(|e| e.into_inner()),
    state,
  );
  match window {
    Some(window) => {
      capture.changed.notify_waiters();
      wake_window(app, window);
    }
    // The link was replaced, dismissed or acted on.
    None => auto::forget(app, &state.group_id),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::capture::state::Offer;

  #[test]
  fn a_session_state_lands_on_the_offer_it_was_started_for() {
    let state = |group: &str| CatchAutoState {
      group_id: group.into(),
      phase: auto::AutoPhase::Looking,
      beside: false,
      title: None,
      duration_sec: None,
      thumbnail: None,
      kind: None,
      formats: None,
    };
    let mut offers = Offers::default();
    let offer = |id| Offer {
      id,
      failed: true,
      reason: Some(FailReason::Unknown),
      ..Offer::default()
    };
    offers.replace(CaptureWindow::Bar, offer(7));
    offers.replace(CaptureWindow::Card, offer(8));
    assert_eq!(
      record(&mut offers, &state("capture-offer-8")),
      Some(CaptureWindow::Card)
    );
    assert!(offers.current(CaptureWindow::Bar).unwrap().auto.is_none());
    let shown = serde_json::to_value(offers.current(CaptureWindow::Card).unwrap()).unwrap();
    assert_eq!(shown["auto"]["groupId"], "capture-offer-8");
    assert_eq!(shown["auto"]["phase"], "looking");
    assert_eq!(shown["reason"], "unknown");
    let mut needs_step = state("capture-offer-8");
    needs_step.phase = auto::AutoPhase::NeedsStep;
    assert_eq!(record(&mut offers, &needs_step), Some(CaptureWindow::Card));
    let shown = serde_json::to_value(offers.current(CaptureWindow::Card).unwrap()).unwrap();
    assert_eq!(shown["reason"], "unsupported");
    // A replaced link, a row of the main window, or a made-up group reaches no offer.
    offers.replace(CaptureWindow::Card, offer(9));
    for group in ["capture-offer-8", "group-1", "capture-offer-x", "capture-9"] {
      assert_eq!(record(&mut offers, &state(group)), None, "{group}");
    }
    assert!(owns("capture-offer-8") && !owns("group-1"));
  }

  #[test]
  fn only_the_current_holding_window_can_use_an_offer() {
    let capture = CaptureState::default();
    capture.offers.lock().unwrap().replace(
      CaptureWindow::Card,
      Offer {
        id: 8,
        url: "https://example.org/video".into(),
        ..Offer::default()
      },
    );
    assert!(held_by(
      &capture,
      CaptureWindow::Card.label(),
      "capture-offer-8"
    ));
    assert!(!held_by(
      &capture,
      CaptureWindow::Bar.label(),
      "capture-offer-8"
    ));
    capture
      .offers
      .lock()
      .unwrap()
      .claim(
        CaptureWindow::Card,
        8,
        super::super::state::Action::Download,
      )
      .unwrap();
    assert!(!held_by(
      &capture,
      CaptureWindow::Card.label(),
      "capture-offer-8"
    ));
    capture
      .offers
      .lock()
      .unwrap()
      .dismiss(CaptureWindow::Card, 8);
    assert!(!held_by(
      &capture,
      CaptureWindow::Card.label(),
      "capture-offer-8"
    ));
  }

  #[test]
  fn failed_capture_reads_plan_the_same_catch_as_rows() {
    let url = "https://example.com/news/report";
    let gate = auto::Gate {
      setting: true,
      proxied: false,
      wayland: false,
      unguarded: false,
    };
    for (stderr, code, generic, catches) in [
      (
        "ERROR: [generic] Unsupported URL: https://example.com/news/report",
        "urlNotSupported",
        false,
        true,
      ),
      (
        "ERROR: [generic] page: Unable to extract",
        "unknown",
        true,
        true,
      ),
      (
        "ERROR: [generic] page: Unable to download webpage: HTTP Error 406: Not Acceptable",
        "unknown",
        true,
        true,
      ),
      (
        "ERROR: [generic] page: Unable to download webpage: HTTP Error 403: Forbidden",
        "accessForbidden403",
        false,
        false,
      ),
      (
        "ERROR: [youtube] abc: Private video. Sign in if you've been granted access",
        "signInRequired",
        false,
        false,
      ),
      (
        "ERROR: [youtube] page: Unable to extract",
        "unknown",
        false,
        false,
      ),
    ] {
      let row = if generic {
        Read::GenericExtractorFailed(code)
      } else {
        Read::Failed(code)
      };
      let expected = auto::plan(url, &row, gate);
      assert_eq!(expected.is_some(), catches, "{stderr}");
      let failure =
        ReadFailure::from_ytdlp(url, super::super::preview::fail_reason(stderr), stderr);
      assert_eq!(
        (failure.code, failure.generic),
        (Some(code), generic),
        "{stderr}"
      );
      let bar = Ended::of::<()>(&Err(failure));
      assert_eq!(
        bar.read().and_then(|read| auto::plan(url, &read, gate)),
        expected,
        "{stderr}"
      );
    }
  }

  #[test]
  fn embedded_reads_keep_their_duration_and_media() {
    let item = |value: serde_json::Value| Ok(Some((value, ())));
    for other in [
      Err(FailReason::Network.into()),
      Ok(None),
      item(serde_json::json!({ "title": "Own video", "duration": 30.0 })),
    ] {
      assert!(matches!(Ended::of::<()>(&other), Ended::Other));
    }
    let embedded = item(serde_json::json!({
      "embeddedIn": "https://films.example.org/one", "duration": 150.0,
      "url": "https://www.youtube.com/watch?v=trailer",
    }));
    let Ended::Embedded { duration, media } = Ended::of(&embedded) else {
      panic!("expected an embedded read");
    };
    assert_eq!(duration, Some(150.0));
    assert_eq!(
      media.as_deref(),
      Some("https://www.youtube.com/watch?v=trailer")
    );
  }
}
