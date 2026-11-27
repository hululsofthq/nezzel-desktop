use crate::models::payloads::NavigatePayload;
use tauri::menu::{MenuBuilder, MenuItem, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager};

/// File > Open Nezzel Browser, item or key: sent to the main window, the one place that opens it (`AppLayout`
/// skips the key on macOS), so a press opens once and never over the welcome screen or a dialog.
const OPEN_BROWSER_EVENT: &str = "menu-open-browser";
const BROWSER_KEY: &str = "CmdOrCtrl+Shift+B";

/// The File menu's browser item, kept to give its key up while a shortcut recorder listens.
struct BrowserItem(MenuItem<tauri::Wry>);

/// A native menu key never reaches the webview: while a recorder listens the item has none, so ⇧⌘B lands in the
/// recorder (which says it is Nezzel's own key) instead of opening the browser.
fn browser_key(recording: bool) -> Option<&'static str> {
  (!recording).then_some(BROWSER_KEY)
}

/// Called with the recorder's pause (`commands::shortcuts_pause`). No menu bar on Windows and Linux: nothing to do.
pub fn pause_keys(app: &AppHandle, recording: bool) {
  if let Some(item) = app.try_state::<BrowserItem>() {
    let _ = item.0.set_accelerator(browser_key(recording));
  }
}

pub fn setup_menu(handle: &AppHandle) {
  #[cfg(target_os = "macos")]
  {
    setup_macos(handle);
  }
  #[cfg(target_os = "windows")]
  {
    setup_windows(handle);
  }
  #[cfg(target_os = "linux")]
  {
    setup_linux(handle);
  }
}

#[allow(dead_code)]
fn setup_macos(handle: &AppHandle) {
  let settings_item = MenuItemBuilder::new("Settings…")
    .id("settings")
    .accelerator("CmdOrCtrl+,")
    .build(handle)
    .expect("Could not build settings item.");

  // Opens Settings → About in the app (owner: one About, no native panel).
  let about_item = MenuItemBuilder::new("About Nezzel")
    .id("about")
    .build(handle)
    .expect("Could not build about item.");

  // Same key inside the window on every system (`BUILT_IN_KEYS.browser`); Windows and Linux have no menu bar.
  let browser_item = MenuItemBuilder::new("Open Nezzel Browser")
    .id("open-browser")
    .accelerator(BROWSER_KEY)
    .build(handle)
    .expect("Could not build browser item.");

  let file_submenu = SubmenuBuilder::new(handle, "File")
    .item(&browser_item)
    .build()
    .expect("Could not build file submenu.");

  let app_submenu = SubmenuBuilder::new(handle, "App")
    .item(&about_item)
    .separator()
    .item(&settings_item)
    .separator()
    .services()
    .separator()
    .hide()
    .hide_others()
    .separator()
    .quit()
    .build()
    .expect("Could not build app submenu.");

  let edit_submenu = SubmenuBuilder::new(handle, "Edit")
    .undo()
    .redo()
    .separator()
    .cut()
    .copy()
    .paste()
    .select_all()
    .build()
    .expect("Could not build edit submenu.");

  let menu = MenuBuilder::new(handle)
    .items(&[&app_submenu, &file_submenu, &edit_submenu])
    .build()
    .expect("Could not attach submenus to menu.");

  handle.set_menu(menu).expect("Could not attach menu.");
  handle.manage(BrowserItem(browser_item.clone()));
  handle.on_menu_event(move |app, event| {
    if event.id() == browser_item.id() {
      let _ = app.emit_to("main", OPEN_BROWSER_EVENT, ());
      return;
    }
    let route = if event.id() == settings_item.id() {
      "settings.general"
    } else if event.id() == about_item.id() {
      "settings.about"
    } else {
      return;
    };
    crate::background::show_main(app);
    let _ = app.emit(
      "navigate",
      NavigatePayload {
        route: route.into(),
      },
    );
  });
}

#[allow(dead_code)]
fn setup_windows(handle: &AppHandle) {
  let menu = MenuBuilder::new(handle)
    .items(&[])
    .build()
    .expect("Could not attach submenus to menu.");

  handle.set_menu(menu).expect("Could not attach menu.");
}

#[allow(dead_code)]
fn setup_linux(handle: &AppHandle) {
  let menu = MenuBuilder::new(handle)
    .items(&[])
    .build()
    .expect("Could not attach submenus to menu.");

  handle.set_menu(menu).expect("Could not attach menu.");
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn the_browser_key_is_given_up_only_while_a_recorder_listens() {
    assert_eq!(browser_key(false), Some("CmdOrCtrl+Shift+B"));
    assert_eq!(browser_key(true), None);
  }
}
