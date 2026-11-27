//! Tab shortcuts while the keyboard is in a page. A page webview has no IPC and its scripts are
//! not trusted, so the keys are read natively, before the page gets them. While the keyboard is
//! in the shell, the shell's own key handler does the same work.

use tauri::{AppHandle, Webview};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shortcut {
  NewTab,
  CloseTab,
  NextTab,
  PreviousTab,
  /// Put the keyboard in the address field.
  Address,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Key {
  T,
  W,
  L,
  Tab,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Mods {
  /// ⌘ on macOS, Ctrl elsewhere.
  pub(super) primary: bool,
  pub(super) ctrl: bool,
  pub(super) shift: bool,
  pub(super) alt: bool,
}

/// ⌘/Ctrl+T new tab, ⌘/Ctrl+W close tab, ⌘/Ctrl+L address, Ctrl+Tab next tab (Shift: previous).
pub(super) fn shortcut(key: Key, mods: Mods) -> Option<Shortcut> {
  if mods.alt {
    return None;
  }
  match key {
    Key::Tab if mods.ctrl && mods.shift => Some(Shortcut::PreviousTab),
    Key::Tab if mods.ctrl => Some(Shortcut::NextTab),
    Key::Tab => None,
    _ if !mods.primary || mods.shift => None,
    Key::T => Some(Shortcut::NewTab),
    Key::W => Some(Shortcut::CloseTab),
    Key::L => Some(Shortcut::Address),
  }
}

/// macOS: a window's shell is known, so keys pressed anywhere else in that window are read.
#[cfg(target_os = "macos")]
pub(super) fn watch_shell(app: &AppHandle, id: &str, shell: &Webview) {
  platform::watch_shell(app, id, shell);
}

#[cfg(not(target_os = "macos"))]
pub(super) fn watch_shell(_app: &AppHandle, _id: &str, _shell: &Webview) {}

/// The window is gone.
#[cfg(target_os = "macos")]
pub(super) fn forget(app: &AppHandle, id: &str) {
  platform::forget(app, id);
}

#[cfg(not(target_os = "macos"))]
pub(super) fn forget(_app: &AppHandle, _id: &str) {}

/// Windows and Linux: each page reports the shortcut keys pressed in it. macOS reads them for
/// the whole window (`watch_shell`).
#[cfg(not(target_os = "macos"))]
pub(super) fn watch_page(app: &AppHandle, id: &str, page: &Webview) {
  let (app, id) = (app.clone(), id.to_owned());
  let _ = page.with_webview(move |view| {
    platform::watch_page(view, move |shortcut| {
      super::window::shortcut(&app, &id, shortcut)
    });
  });
}

#[cfg(target_os = "macos")]
pub(super) fn watch_page(_app: &AppHandle, _id: &str, _page: &Webview) {}

#[cfg(target_os = "macos")]
mod platform {
  use super::*;
  use block2::RcBlock;
  use objc2::{rc::Retained, MainThreadMarker};
  use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSView};
  use std::{cell::RefCell, collections::HashMap, ptr::NonNull, sync::Once};

  thread_local! {
    /// Main thread only: the shell view of every open window, by window id.
    static SHELLS: RefCell<HashMap<String, Retained<NSView>>> = RefCell::default();
  }

  pub(super) fn watch_shell(app: &AppHandle, id: &str, shell: &Webview) {
    let (app, id) = (app.clone(), id.to_owned());
    let _ = shell.with_webview(move |view| {
      // Tauri supplies a live WKWebView (an NSView) on the main thread.
      let Some(view) = (unsafe { Retained::retain(view.inner().cast::<NSView>()) }) else {
        return;
      };
      SHELLS.with_borrow_mut(|shells| shells.insert(id, view));
      install(app);
    });
  }

  pub(super) fn forget(app: &AppHandle, id: &str) {
    let id = id.to_owned();
    let _ = app.run_on_main_thread(move || {
      SHELLS.with_borrow_mut(|shells| shells.remove(&id));
    });
  }

  /// One key monitor for the app, added with the first window.
  fn install(app: AppHandle) {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
      let handler = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        // AppKit passes a live key event, on the main thread.
        match pressed(unsafe { event.as_ref() }) {
          Some((id, shortcut)) => {
            super::super::window::shortcut(&app, &id, shortcut);
            // Handled: neither the page nor a menu gets the key.
            std::ptr::null_mut()
          }
          None => event.as_ptr(),
        }
      });
      // The block returns the event it was given, or null.
      let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &handler)
      };
      // The monitor stays for the life of the app.
      std::mem::forget(monitor);
    });
  }

  fn key(event: &NSEvent) -> Option<Key> {
    // With ⌘ held, `characters` is the Latin letter in other keyboard layouts too (Arabic).
    let typed = event.characters().map(|typed| typed.to_string());
    match (event.keyCode(), typed.as_deref()) {
      (48, _) => Some(Key::Tab),
      (_, Some("t" | "T")) => Some(Key::T),
      (_, Some("w" | "W")) => Some(Key::W),
      (_, Some("l" | "L")) => Some(Key::L),
      _ => None,
    }
  }

  /// The window and shortcut of a key pressed outside the shell of a Nezzel browser window.
  fn pressed(event: &NSEvent) -> Option<(String, Shortcut)> {
    let mtm = MainThreadMarker::new()?;
    let flags = event.modifierFlags();
    let mods = Mods {
      primary: flags.contains(NSEventModifierFlags::Command),
      ctrl: flags.contains(NSEventModifierFlags::Control),
      shift: flags.contains(NSEventModifierFlags::Shift),
      alt: flags.contains(NSEventModifierFlags::Option),
    };
    // Cheap test first: this runs for every key pressed in the app.
    if !(mods.primary || mods.ctrl) {
      return None;
    }
    let shortcut = shortcut(key(event)?, mods)?;
    let window = event.window(mtm)?;
    SHELLS.with_borrow(|shells| {
      let (id, shell) = shells.iter().find(|(_, shell)| {
        shell
          .window()
          .is_some_and(|owner| Retained::as_ptr(&owner) == Retained::as_ptr(&window))
      })?;
      // The keyboard is in the shell: its own key handler acts.
      let in_shell = window
        .firstResponder()
        .and_then(|responder| responder.downcast::<NSView>().ok())
        .is_some_and(|view| view.isDescendantOf(shell));
      (!in_shell).then(|| (id.clone(), shortcut))
    })
  }
}

#[cfg(windows)]
mod platform {
  use super::*;
  use tauri::webview::PlatformWebview;
  use webview2_com::{
    AcceleratorKeyPressedEventHandler,
    Microsoft::Web::WebView2::Win32::{
      COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
    },
  };
  use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT,
  };

  pub(super) fn watch_page(view: PlatformWebview, pressed: impl Fn(Shortcut) + 'static) {
    let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
      let Some(args) = args else {
        return Ok(());
      };
      let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
      let mut code = 0u32;
      // The out pointers are valid for the calls.
      unsafe {
        args.KeyEventKind(&mut kind)?;
        args.VirtualKey(&mut code)?;
      }
      if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN {
        return Ok(());
      }
      let key = match code {
        0x09 => Key::Tab,
        0x54 => Key::T,
        0x57 => Key::W,
        0x4C => Key::L,
        _ => return Ok(()),
      };
      // The key state at the time of this message.
      let down = |key: u16| unsafe { GetKeyState(i32::from(key)) } < 0;
      let ctrl = down(VK_CONTROL);
      let mods = Mods {
        primary: ctrl,
        ctrl,
        shift: down(VK_SHIFT),
        alt: down(VK_MENU),
      };
      if let Some(shortcut) = shortcut(key, mods) {
        unsafe { args.SetHandled(true) }?;
        pressed(shortcut);
      }
      Ok(())
    }));
    let mut token = 0i64;
    // WebView2 retains the handler for the webview's lifetime.
    let _ = unsafe {
      view
        .controller()
        .add_AcceleratorKeyPressed(&handler, &mut token)
    };
  }
}

#[cfg(target_os = "linux")]
mod platform {
  use super::*;
  use gtk::{gdk::ModifierType, glib::Propagation, prelude::*};
  use tauri::webview::PlatformWebview;

  pub(super) fn watch_page(view: PlatformWebview, pressed: impl Fn(Shortcut) + 'static) {
    view.inner().connect_key_press_event(move |_, event| {
      // The letter typed, or the same physical key in another keyboard layout (Arabic).
      let typed = event.keyval().to_unicode().map(|c| c.to_ascii_lowercase());
      let key = match (typed, event.hardware_keycode()) {
        (_, 23) => Key::Tab,
        (Some('t'), _) | (_, 28) => Key::T,
        (Some('w'), _) | (_, 25) => Key::W,
        (Some('l'), _) | (_, 46) => Key::L,
        _ => return Propagation::Proceed,
      };
      let state = event.state();
      let ctrl = state.contains(ModifierType::CONTROL_MASK);
      let mods = Mods {
        primary: ctrl,
        ctrl,
        shift: state.contains(ModifierType::SHIFT_MASK),
        alt: state.contains(ModifierType::MOD1_MASK),
      };
      match shortcut(key, mods) {
        Some(shortcut) => {
          pressed(shortcut);
          Propagation::Stop
        }
        None => Propagation::Proceed,
      }
    });
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn tab_shortcuts_need_their_modifier_and_nothing_extra() {
    let mods = |primary, ctrl, shift, alt| Mods {
      primary,
      ctrl,
      shift,
      alt,
    };
    let cases = [
      (
        Key::T,
        mods(true, false, false, false),
        Some(Shortcut::NewTab),
      ),
      (
        Key::W,
        mods(true, false, false, false),
        Some(Shortcut::CloseTab),
      ),
      (
        Key::L,
        mods(true, false, false, false),
        Some(Shortcut::Address),
      ),
      (
        Key::Tab,
        mods(false, true, false, false),
        Some(Shortcut::NextTab),
      ),
      (
        Key::Tab,
        mods(false, true, true, false),
        Some(Shortcut::PreviousTab),
      ),
      // Ctrl is the primary key on Windows and Linux.
      (
        Key::Tab,
        mods(true, true, false, false),
        Some(Shortcut::NextTab),
      ),
      (Key::T, mods(false, false, false, false), None),
      // macOS: Ctrl+T is the page's own key.
      (Key::T, mods(false, true, false, false), None),
      // ⌘⇧T, ⌘⌥W: not ours.
      (Key::T, mods(true, false, true, false), None),
      (Key::W, mods(true, false, false, true), None),
      // ⌘Tab belongs to the system, plain Tab to the page.
      (Key::Tab, mods(true, false, false, false), None),
      (Key::Tab, mods(false, false, false, false), None),
    ];
    for (key, mods, expected) in cases {
      assert_eq!(shortcut(key, mods), expected, "{key:?} {mods:?}");
    }
  }
}
