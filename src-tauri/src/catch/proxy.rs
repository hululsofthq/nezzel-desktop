//! The Settings proxy on the Nezzel browser's pages: every tab and, through its opener, every
//! pop-up (the engines hand a pop-up its opener's store, environment or context). The hidden
//! catch page keeps its own guard route and never runs with the proxy on (`auto::Gate`).
//!
//! A window reads the setting once, when it opens (`Browser`): WebView2 takes its arguments per
//! profile folder and refuses a new page whose arguments differ from the running browser's, so
//! the tabs of one window always share one proxy. A change applies to the next window.
//!
//! On and unusable (`BrowserProxy::Blocked`): the window makes no page. A proxied page is also
//! sealed: no WebRTC and no name lookup beside the proxy (`seal`, `browser_arguments`).

use crate::net::{BrowserProxy, ProxyAddress};
use tauri::Url;

/// The command error of a window whose Settings proxy is on and unusable: the bar says why.
pub(super) const UNUSABLE: &str = "catchProxyUnusable";

// Each system builds only its own engine; the tests build all of them.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Engine {
  /// WKWebView; the store's `proxyConfigurations` exists from macOS 14.
  WebKitMac {
    major: u64,
  },
  WebView2,
  WebKitGtk,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Plan {
  /// Given to the page builder (`proxy_url`): wry applies it to the page's store (macOS) or its
  /// web context (Linux).
  pub(super) url: Option<Url>,
  /// WebView2: the browser arguments, given whole (`additional_browser_args`).
  #[cfg_attr(not(windows), allow(dead_code))]
  pub(super) arguments: Option<String>,
  /// The shared profile's store or context may still hold a proxy an earlier window of this run
  /// set: reset it to the engine's default before the page loads anything.
  pub(super) clear: bool,
  /// The page goes through the proxy: nothing of it may leave beside it (`seal`).
  pub(super) sealed: bool,
}

/// `shared`: the page uses the saved catch profile (a private tab's store is new).
/// `Err`: the proxy is on and unusable, so no page is made.
pub(super) fn plan(
  proxy: &BrowserProxy,
  engine: Engine,
  shared: bool,
) -> Result<Plan, &'static str> {
  let address = match proxy {
    BrowserProxy::Blocked => return Err(UNUSABLE),
    // Older macOS has no per-store proxy (setting the key would raise): the system proxy applies.
    _ if matches!(engine, Engine::WebKitMac { major } if major < 14) => return Ok(Plan::default()),
    BrowserProxy::Via(address) => address,
    BrowserProxy::Off | BrowserProxy::Unsupported => {
      return Ok(Plan {
        // WebView2's arguments live with the browser process, not with a store.
        clear: shared && engine != Engine::WebView2,
        ..Plan::default()
      });
    }
  };
  if engine == Engine::WebView2 {
    return Ok(Plan {
      arguments: Some(browser_arguments(address)),
      ..Plan::default()
    });
  }
  let url = format!("{}://{}:{}", address.scheme(), address.host, address.port);
  Ok(Plan {
    // An address that passed `net::browser_proxy_for` always reads as a URL.
    url: Some(Url::parse(&url).map_err(|_| UNUSABLE)?),
    sealed: true,
    ..Plan::default()
  })
}

/// WebView2's browser arguments for a proxied window: wry's defaults, the proxy with its port
/// written out, and what keeps the page behind it. Chromium skips the proxy for loopback unless
/// told not to; WebRTC may not send UDP around the proxy; names are looked up by the proxy only
/// (the engine's own resolver answers "not found" for all but the proxy's own name), so no
/// lookup, prefetch included, goes to the system's DNS. The host is an address or a plain name
/// (`ProxyAddress`), so it cannot add an argument or a second proxy rule.
fn browser_arguments(address: &ProxyAddress) -> String {
  let (host, bare) = (&address.host, address.bare_host());
  // An IPv6 proxy: either spelling, whichever this Chromium compares with.
  let own = if host == bare {
    format!("EXCLUDE {bare}")
  } else {
    format!("EXCLUDE {bare} , EXCLUDE {host}")
  };
  format!(
    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
     --autoplay-policy=no-user-gesture-required \
     --proxy-server={}://{host}:{} --proxy-bypass-list=<-loopback> \
     --host-resolver-rules=\"MAP * ~NOTFOUND , {own}\" \
     --force-webrtc-ip-handling-policy=disable_non_proxied_udp",
    address.scheme(),
    address.port
  )
}

#[cfg(target_os = "macos")]
pub(super) fn engine() -> Engine {
  Engine::WebKitMac {
    major: objc2_foundation::NSProcessInfo::processInfo()
      .operatingSystemVersion()
      .majorVersion
      .try_into()
      .unwrap_or(0),
  }
}

#[cfg(windows)]
pub(super) fn engine() -> Engine {
  Engine::WebView2
}

#[cfg(target_os = "linux")]
pub(super) fn engine() -> Engine {
  Engine::WebKitGtk
}

/// Resets the page's store (macOS) or web context (Linux) to the engine's default proxy. Queued
/// on the main thread before the page's first navigation, which is queued after it.
pub(super) fn clear(page: &tauri::Webview) {
  #[cfg(target_os = "macos")]
  let _ = page.with_webview(|view| {
    use objc2::msg_send;
    use objc2_foundation::{NSArray, NSObject, NSString};
    use objc2_web_kit::WKWebView;
    // Tauri supplies a live WKWebView on the main thread; `plan` asks for this on macOS 14+
    // only, where the store has `proxyConfigurations`.
    unsafe {
      let view = &*view.inner().cast::<WKWebView>();
      let store = view.configuration().websiteDataStore();
      let none = NSArray::<NSObject>::new();
      let key = NSString::from_str("proxyConfigurations");
      let _: () = msg_send![&*store, setValue: &*none, forKey: &*key];
    }
  });
  #[cfg(target_os = "linux")]
  let _ = page.with_webview(|view| {
    use webkit2gtk::{NetworkProxyMode, WebContextExt, WebViewExt, WebsiteDataManagerExt};
    let view = view.inner();
    if let Some(manager) = WebViewExt::context(&view).and_then(|c| c.website_data_manager()) {
      manager.set_network_proxy_settings(NetworkProxyMode::Default, None);
    }
  });
  #[cfg(windows)]
  let _ = page;
}

/// A proxied page opens no connection beside its proxy: no WebRTC (its UDP goes direct) and, on
/// Linux, no DNS prefetch (a lookup the proxy never sees). WebView2 has both in its arguments.
/// Queued on the main thread before the page's first navigation, like `clear`.
pub(super) fn seal(page: &tauri::Webview) {
  #[cfg(target_os = "macos")]
  let _ = page.with_webview(|view| {
    use objc2::{msg_send, sel, ClassType};
    use objc2_web_kit::{WKPreferences, WKWebView};
    // WebKit's switch for `RTCPeerConnection` is a preference without a public name
    // (`WKPreferencesPrivate.h`, macOS 10.13.4+): asked of the class first, it takes one BOOL.
    // Tauri supplies a live WKWebView on the main thread.
    unsafe {
      let view = &*view.inner().cast::<WKWebView>();
      let preferences = view.configuration().preferences();
      if WKPreferences::class()
        .instance_method(sel!(_setPeerConnectionEnabled:))
        .is_some()
      {
        let _: () = msg_send![&*preferences, _setPeerConnectionEnabled: false];
      }
    }
  });
  #[cfg(target_os = "linux")]
  let _ = page.with_webview(|view| {
    use webkit2gtk::{SettingsExt, WebViewExt};
    if let Some(settings) = WebViewExt::settings(&view.inner()) {
      settings.set_enable_webrtc(false);
      settings.set_enable_dns_prefetching(false);
    }
  });
  #[cfg(windows)]
  let _ = page;
}

#[cfg(test)]
mod tests {
  use super::*;

  fn address(socks: bool, host: &str, port: u16) -> ProxyAddress {
    ProxyAddress {
      socks,
      host: host.into(),
      port,
    }
  }

  #[test]
  fn each_engine_gets_the_proxy_it_can_take() {
    let via = BrowserProxy::Via(address(true, "127.0.0.1", 1080));
    let on = || {
      Ok(Plan {
        url: Some(Url::parse("socks5://127.0.0.1:1080").unwrap()),
        sealed: true,
        ..Plan::default()
      })
    };
    let off = |clear| {
      Ok(Plan {
        clear,
        ..Plan::default()
      })
    };
    let (mac13, mac14) = (
      Engine::WebKitMac { major: 13 },
      Engine::WebKitMac { major: 14 },
    );
    for (proxy, engine, shared, expected) in [
      (&via, mac14, true, on()),
      (&via, mac14, false, on()),
      (&via, Engine::WebKitGtk, true, on()),
      // macOS 13: the system proxy, nothing set or cleared.
      (&via, mac13, true, off(false)),
      (&BrowserProxy::Off, mac13, true, off(false)),
      // Off after an earlier window had one: the shared store/context goes back to default.
      (&BrowserProxy::Off, mac14, true, off(true)),
      (&BrowserProxy::Off, Engine::WebKitGtk, true, off(true)),
      (&BrowserProxy::Unsupported, mac14, true, off(true)),
      (&BrowserProxy::Off, Engine::WebView2, true, off(false)),
      // A private tab's store is new.
      (&BrowserProxy::Off, mac14, false, off(false)),
      (&BrowserProxy::Off, Engine::WebKitGtk, false, off(false)),
      // On and unusable: no page on any engine, the oldest macOS included.
      (&BrowserProxy::Blocked, mac14, true, Err(UNUSABLE)),
      (&BrowserProxy::Blocked, mac13, true, Err(UNUSABLE)),
      (
        &BrowserProxy::Blocked,
        Engine::WebView2,
        false,
        Err(UNUSABLE),
      ),
      (
        &BrowserProxy::Blocked,
        Engine::WebKitGtk,
        true,
        Err(UNUSABLE),
      ),
    ] {
      assert_eq!(
        plan(proxy, engine, shared),
        expected,
        "{proxy:?} {engine:?} {shared}"
      );
    }
  }

  #[test]
  fn webview2_gets_whole_arguments_that_keep_the_page_behind_the_proxy() {
    let arguments = |address| {
      let plan = plan(&BrowserProxy::Via(address), Engine::WebView2, true).unwrap();
      assert_eq!((plan.url, plan.clear, plan.sealed), (None, false, false));
      plan.arguments.unwrap()
    };
    // Port 80 is written out: `--proxy-server=http://host:` would be no proxy at all.
    let http = arguments(address(false, "proxy.example", 80));
    for part in [
      "--proxy-server=http://proxy.example:80 ",
      "--proxy-bypass-list=<-loopback>",
      "--host-resolver-rules=\"MAP * ~NOTFOUND , EXCLUDE proxy.example\"",
      "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
      // wry's own defaults stay.
      "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection",
      "--autoplay-policy=no-user-gesture-required",
    ] {
      assert!(http.contains(part), "{part} in {http}");
    }
    let v6 = arguments(address(true, "[::1]", 1080));
    assert!(v6.contains("--proxy-server=socks5://[::1]:1080 "), "{v6}");
    assert!(v6.contains("EXCLUDE ::1 , EXCLUDE [::1]\""), "{v6}");
  }

  /// What Tauri hands wry for the plan's URL (`tauri-runtime-wry` `parse_proxy_url`: the URL's
  /// host and port as text), and what the patched wry connects to.
  // wry is a direct dependency on macOS only (Cargo.toml); the port and bare-host rule it pins is
  // the one WKWebView's proxy endpoint needs.
  #[cfg(target_os = "macos")]
  #[test]
  fn wry_connects_to_the_usual_port_and_the_bare_ipv6_address() {
    let handed = |address| {
      let url = plan(
        &BrowserProxy::Via(address),
        Engine::WebKitMac { major: 14 },
        true,
      )
      .unwrap()
      .url
      .unwrap();
      let endpoint = wry::ProxyEndpoint {
        host: url.host().map(|host| host.to_string()).unwrap_or_default(),
        port: url.port().map(|port| port.to_string()).unwrap_or_default(),
      };
      match url.scheme() {
        "http" => wry::ProxyConfig::Http(endpoint),
        _ => wry::ProxyConfig::Socks5(endpoint),
      }
    };
    let connects = |config: &wry::ProxyConfig| {
      let (wry::ProxyConfig::Http(endpoint) | wry::ProxyConfig::Socks5(endpoint)) = config;
      (endpoint.bare_host().to_owned(), config.port().to_owned())
    };
    for (address, expected) in [
      (address(false, "proxy.example", 80), ("proxy.example", "80")),
      (
        address(false, "proxy.example", 3128),
        ("proxy.example", "3128"),
      ),
      (address(true, "127.0.0.1", 1080), ("127.0.0.1", "1080")),
      (address(true, "[::1]", 1080), ("::1", "1080")),
      (address(false, "[2001:db8::1]", 80), ("2001:db8::1", "80")),
    ] {
      let (host, port) = connects(&handed(address));
      assert_eq!((host.as_str(), port.as_str()), expected);
    }
    // A SOCKS proxy without a port, should Tauri ever hand one over.
    let bare = wry::ProxyConfig::Socks5(wry::ProxyEndpoint {
      host: "h".into(),
      port: String::new(),
    });
    assert_eq!(bare.port(), "1080");
  }
}
