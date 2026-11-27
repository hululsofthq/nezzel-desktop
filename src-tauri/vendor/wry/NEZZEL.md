# Local wry patch

Source: wry 0.57.1, https://github.com/tauri-apps/wry (MIT OR Apache-2.0).
Copied from the published crate; source and licences retained.

Release builds use `panic = "abort"`, so an `unwrap()` on webview state that a page or a close
race can make empty kills the whole app. Only change: those unwraps become early returns or
errors. Every changed spot carries a `Nezzel patch:` comment (`grep -rn "Nezzel patch" src`):

- `wkwebview/mod.rs` `url_from_webview`: nil `URL` (fresh or crashed page) → `Err`. Tauri adds a
  page-load handler to every webview, so this ran on every load, even without `on_page_load`.
- `wkwebview/mod.rs` `evaluate_script` callback: a value `NSJSONSerialization` refuses → `""`,
  also when it refuses with an Objective-C exception (a date, NaN).
- `wkwebview/mod.rs` `focus`: no window (focus racing a close) → nothing.
- `wkwebview/class/wry_web_view_delegate.rs` IPC handler: message from a frame without URL → dropped.
- `webview2/mod.rs` new-window handler: event without its sender → skipped.
- `wkwebview/mod.rs` `navigate_to_url`: address `NSURL` refuses → `Err`.
- `wkwebview/mod.rs` `bounds` / `set_bounds`: no superview/window (resize racing a close).
- `wkwebview/navigation.rs` `navigation_policy`: request without URL → cancel.
- `lib.rs` `navigation_main_frame` + `wkwebview/navigation.rs`: additive; macOS tells the navigation
  handler (via a thread-local read during the call) whether the main frame navigates.
- `navigation_modifiers` exposes the link activation type, modifier flags and mouse button during
  the macOS navigation policy callback. The button is WebKit's mask (`toNSButtonNumber`: middle is 4).
- `set_navigation_failure_hook` observes macOS main-frame provisional and committed failures with
  their webview, ID, native error code and failing address when available.
- `wkwebview/class/wry_web_view_ui_delegate.rs`: popup without URL, window or screen → denied.
- `lib.rs` `navigation_new_window` / `popups_dropped` (additive, logging only): macOS tells the
  navigation handler whether the decision opens a new window, and counts pop-ups the UI delegate
  denied before the handler (no URL, window or screen).
- `wkwebview/download.rs`: download without request URL → cancelled / ignored.
- `webkitgtk/mod.rs` page-load handler: no `uri()` → event skipped.
- `wkwebview/class/wry_web_view_ui_delegate.rs` `run_file_upload_panel`: a webview whose window is
  hidden (the automatic-catch page) or gone answers with no files instead of a modal panel.
- `wkwebview/proxy.rs` + `wkwebview/mod.rs` (feature `mac-proxy`): `nw_proxy_config_create_socksv5`
  / `_http_connect` exist from macOS 14 only; they are found with `dlsym` at run time instead of
  linked (a strong import stops the app launching on macOS 10.13–13), and a missing symbol or a
  bad endpoint is an `Err` from webview creation instead of an `unwrap()`.
  `nw_endpoint_create_host` (macOS 10.14+) is looked up the same way and the Network framework is
  no longer linked. The nw objects are released (`Retained`), and a data store that has no
  `setProxyConfigurations:` or refuses the value with an exception is an `Err` too, which also
  removes the webview's `WEBVIEW_STATE` entry.
- `wkwebview/navigation.rs` `navigation_policy`: a navigation that turns into a download
  (`<a download>`, `shouldPerformDownload`) passes the navigation handler first; upstream skipped it.
- `webkitgtk/mod.rs`: a proxy asked for on a context without a website data manager → `Err`
  from webview creation; upstream made the page without the proxy.
- `proxy.rs` `ProxyConfig::port` / `ProxyEndpoint::bare_host` (additive), used by `wkwebview` and
  `webkitgtk`: Tauri passes a proxy URL's port as text, empty for the scheme's usual port
  (`http://host:80`), and an IPv6 host in brackets. An empty port made `nw_endpoint_create_host`
  return nothing (no page) and gave WebKitGTK `http://host:`; a bracketed host made macOS look
  `[::1]` up as a name. The usual port is filled in and macOS gets the address without brackets.
  WebView2 is not patched: the app passes its own browser arguments there (`catch::proxy`).

Remove this patch when upstream wry handles these cases.
