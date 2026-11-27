//! Which of the shell and the page is on top. The shell covers the whole window and pages sit
//! above it, so clicks on a page reach the page; while a menu, a picker or a prompt is open the
//! shell goes above the page, draws over it and takes the clicks there.

use tauri::{Webview, Window};

/// macOS: every webview is a subview of the window's content view. Only the shell moves, to the
/// front or to the back of them all: a playing page is never taken out of the window.
#[cfg(target_os = "macos")]
pub(super) fn order(
  _window: &Window,
  shell: &Webview,
  _page: Option<&Webview>,
  shell_on_top: bool,
) {
  use objc2_app_kit::{NSView, NSWindowOrderingMode};
  let _ = shell.with_webview(move |view| {
    // Tauri supplies a live WKWebView (an NSView) on the main thread.
    let view = unsafe { &*view.inner().cast::<NSView>() };
    let Some(parent) = (unsafe { view.superview() }) else {
      return;
    };
    let place = if shell_on_top {
      NSWindowOrderingMode::Above
    } else {
      NSWindowOrderingMode::Below
    };
    parent.addSubview_positioned_relativeTo(view, place, None);
  });
}

/// Windows: every webview has its own child window in the catch window. Only the shell's moves,
/// to the top or to the bottom of them (Tauri's `reparent` into the same window changes no order).
#[cfg(windows)]
pub(super) fn order(
  _window: &Window,
  shell: &Webview,
  _page: Option<&Webview>,
  shell_on_top: bool,
) {
  use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
      SetWindowPos, HWND_BOTTOM, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    },
  };
  let _ = shell.with_webview(move |view| {
    let mut container = HWND::default();
    // Tauri supplies the live controller on its COM apartment; the out pointer is valid for
    // the call.
    if unsafe { view.controller().ParentWindow(&mut container) }.is_err() {
      return;
    }
    let place = if shell_on_top { HWND_TOP } else { HWND_BOTTOM };
    let _ = unsafe {
      SetWindowPos(
        container,
        Some(place),
        0,
        0,
        0,
        0,
        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
      )
    };
  });
}

/// Linux: the webviews share one stage (see `place`), each with its own GDK window. Only the
/// shell's is raised or lowered.
#[cfg(target_os = "linux")]
pub(super) fn order(
  _window: &Window,
  shell: &Webview,
  _page: Option<&Webview>,
  shell_on_top: bool,
) {
  use gtk::prelude::WidgetExt;
  let _ = shell.with_webview(move |view| {
    if let Some(window) = view.inner().window() {
      if shell_on_top {
        window.raise();
      } else {
        window.lower();
      }
    }
  });
}

/// Puts a webview at its rectangle of the window (logical px).
#[cfg(not(target_os = "linux"))]
pub(super) fn place(view: &Webview, x: f64, y: f64, width: f64, height: f64) {
  use tauri::{LogicalPosition, LogicalSize, Rect};
  // One set_bounds: set_size and set_position each read the bounds back first.
  let _ = view.set_bounds(Rect {
    position: LogicalPosition::new(x, y).into(),
    size: LogicalSize::new(width, height).into(),
  });
}

/// Linux: Tauri packs a window's webviews one under the other in a box and ignores their
/// bounds. They move to one free-position container in that box (the stage), where each gets
/// its rectangle and they may overlap. The stage asks for no size of its own, so the window
/// still shrinks.
#[cfg(target_os = "linux")]
pub(super) fn place(view: &Webview, x: f64, y: f64, width: f64, height: f64) {
  use gtk::prelude::*;
  const STAGE: &str = "nezzel-catch-stage";
  let (x, y) = (x.round() as i32, y.round() as i32);
  let (width, height) = (width.round().max(1.) as i32, height.round().max(1.) as i32);
  let _ = view.with_webview(move |view| {
    let widget = view.inner().upcast::<gtk::Widget>();
    let Some(parent) = widget.parent() else {
      return;
    };
    let staged = parent.widget_name().as_str() == STAGE;
    let stage = if staged {
      parent.downcast::<gtk::Layout>().ok()
    } else {
      parent.downcast::<gtk::Box>().ok().map(|vbox| {
        let stage = vbox
          .children()
          .into_iter()
          .filter(|child| child.widget_name().as_str() == STAGE)
          .find_map(|child| child.downcast::<gtk::Layout>().ok())
          .unwrap_or_else(|| {
            let stage = gtk::Layout::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
            stage.set_widget_name(STAGE);
            vbox.pack_start(&stage, true, true, 0);
            stage.show();
            stage
          });
        // The webview keeps its page: wry holds the widget while it has no parent.
        vbox.remove(&widget);
        stage.put(&widget, x, y);
        stage
      })
    };
    let Some(stage) = stage else {
      return;
    };
    stage.move_(&widget, x, y);
    widget.set_size_request(width, height);
  });
}
