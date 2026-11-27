//! Network admission shared by every engine request that fetches a link the user gave us
//! (`resolve::http` page reads, `files::client` downloads): which addresses a request may reach,
//! and the pinned client that connects to exactly the admitted answer. Each consumer keeps its
//! own user agent, headers, ports, redirect loop and timeouts.
use reqwest::Url;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

/// Desktop Chrome, for requests that must look like a browser navigation (page reads, Dropbox).
pub(crate) const CHROME_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

static CONFIG: std::sync::OnceLock<crate::SharedConfig> = std::sync::OnceLock::new();

#[cfg(test)]
thread_local! {
  static TEST_PROXIED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test scope: proxy on for this thread until the guard drops (restores the previous value).
#[cfg(test)]
pub(crate) struct ProxiedGuard(bool);
#[cfg(test)]
impl ProxiedGuard {
  pub(crate) fn on() -> Self {
    Self(TEST_PROXIED.replace(true))
  }
}
#[cfg(test)]
impl Drop for ProxiedGuard {
  fn drop(&mut self) {
    TEST_PROXIED.set(self.0);
  }
}

/// True while the user has the proxy switched on in Settings. The pinned clients connect to
/// addresses they resolved and admitted themselves, which a proxy would bypass, so every
/// built-in page read refuses (and its caller falls through to the engines, which honour the
/// proxy) instead of leaving outside it. Reads the live config snapshot, the same one the
/// engines read: no second flag that could lag a config change. System and environment
/// proxies stay ignored. Unset only before startup setup and in unit tests.
pub(crate) fn proxied() -> bool {
  #[cfg(test)]
  if TEST_PROXIED.get() {
    return true;
  }
  CONFIG
    .get()
    .is_some_and(|config| config.load().network.enable_proxy == Some(true))
}

/// How the app's own clients (reports, usage counts, update check) reach the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OwnProxy {
  Direct,
  /// The user's proxy, always with a scheme (a bare `host:port` means `http://`).
  Via(String),
  /// The proxy is switched on with no address: send nothing (same as the tool downloads).
  Blocked,
}

pub(crate) fn own_proxy_for(enabled: Option<bool>, proxy: Option<&str>) -> OwnProxy {
  if enabled != Some(true) {
    return OwnProxy::Direct;
  }
  match proxy.map(str::trim).filter(|value| !value.is_empty()) {
    None => OwnProxy::Blocked,
    Some(value) if value.contains("://") => OwnProxy::Via(value.into()),
    Some(value) => OwnProxy::Via(format!("http://{value}")),
  }
}

/// The live Settings proxy for the app's own clients; read per request, so a change applies at once.
pub(crate) fn own_proxy() -> OwnProxy {
  CONFIG.get().map_or(OwnProxy::Direct, |config| {
    let config = config.load();
    own_proxy_for(config.network.enable_proxy, config.network.proxy.as_deref())
  })
}

/// Where the Nezzel browser's engines connect: an `http://` or `socks5://` proxy by host and port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProxyAddress {
  /// SOCKS5 (the engine sends the name to the proxy); HTTP CONNECT otherwise.
  pub(crate) socks: bool,
  /// An IPv4 address, an IPv6 address in brackets, or a name of letters, digits, dots and
  /// hyphens: nothing an engine's proxy argument could read as a second rule.
  pub(crate) host: String,
  /// Never 0. Always written out: a URL drops its scheme's usual port.
  pub(crate) port: u16,
}

impl ProxyAddress {
  pub(crate) fn scheme(&self) -> &'static str {
    if self.socks {
      "socks5"
    } else {
      "http"
    }
  }

  /// The host as a resolver takes it: an IPv6 address without its brackets.
  pub(crate) fn bare_host(&self) -> &str {
    self.host.trim_matches(['[', ']'])
  }
}

/// The Settings proxy as the Nezzel browser's engines take it (wry's `ProxyConfig`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BrowserProxy {
  /// Proxy off: each engine's own default (the system proxy).
  Off,
  Via(ProxyAddress),
  /// On, with a well-formed address of a kind no engine takes (https, socks4): the engine's
  /// default, and Settings says which kinds the browser takes.
  Unsupported,
  /// On, with no address, one that does not read as `scheme://host:port`, or one with a user
  /// name or password (no engine takes them): the browser loads no page rather than connect
  /// around the proxy.
  Blocked,
}

impl BrowserProxy {
  /// Whether a window opened with this setting may fetch beside its pages (manifest probes,
  /// thumbnails). Never while the proxy was on: those requests connect directly, and the
  /// window keeps its proxy even after the setting changes.
  pub(crate) fn fetches_beside(&self) -> bool {
    matches!(self, Self::Off)
  }
}

/// Dot-separated labels of ASCII letters, digits and inner hyphens.
fn host_name(name: &str) -> bool {
  name.len() <= 253
    && name.split('.').all(|label| {
      !label.is_empty()
        && label.len() <= 63
        && label
          .bytes()
          .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !label.starts_with('-')
        && !label.ends_with('-')
    })
}

pub(crate) fn browser_proxy_for(enabled: Option<bool>, proxy: Option<&str>) -> BrowserProxy {
  if enabled != Some(true) {
    return BrowserProxy::Off;
  }
  let OwnProxy::Via(value) = own_proxy_for(enabled, proxy) else {
    return BrowserProxy::Blocked;
  };
  let Ok(url) = Url::parse(&value) else {
    return BrowserProxy::Blocked;
  };
  let socks = match url.scheme() {
    "http" => Some(false),
    // The engines send the name to a SOCKS5 proxy themselves.
    "socks5" | "socks5h" => Some(true),
    "https" | "socks4" | "socks4a" => None,
    _ => return BrowserProxy::Blocked,
  };
  let named = host(&url).filter(|bare| bare.parse::<IpAddr>().is_ok() || host_name(bare));
  let (Some(_), Some(host)) = (named, url.host_str()) else {
    return BrowserProxy::Blocked;
  };
  if url.port() == Some(0)
    || !matches!(url.path(), "" | "/")
    || url.query().is_some()
    || url.fragment().is_some()
  {
    return BrowserProxy::Blocked;
  }
  let Some(socks) = socks else {
    return BrowserProxy::Unsupported;
  };
  if !url.username().is_empty() || url.password().is_some() {
    return BrowserProxy::Blocked;
  }
  BrowserProxy::Via(ProxyAddress {
    socks,
    host: host.to_owned(),
    // A bare SOCKS proxy listens on its usual port.
    port: url.port().unwrap_or(if socks { 1080 } else { 80 }),
  })
}

/// The live Settings proxy for the Nezzel browser's pages.
pub(crate) fn browser_proxy() -> BrowserProxy {
  CONFIG.get().map_or(BrowserProxy::Off, |config| {
    let config = config.load();
    browser_proxy_for(config.network.enable_proxy, config.network.proxy.as_deref())
  })
}

pub(crate) fn setup(config: crate::SharedConfig) {
  let _ = CONFIG.set(config);
}

/// Bound on one DNS lookup during admission.
pub(crate) const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
  /// An empty answer, an answer on another port, or an address the policy refuses.
  Address,
  /// The lookup did not answer within [`LOOKUP_TIMEOUT`].
  Timeout,
  /// The lookup failed.
  Lookup(std::io::ErrorKind),
}

/// The URL host without IPv6 brackets, ready for an IP-literal parse or a lookup.
pub(crate) fn host(url: &Url) -> Option<&str> {
  url.host_str().map(|host| host.trim_matches(['[', ']']))
}

/// An IP-literal host is its own answer; any other host is looked up once. Every address in the
/// answer must pass `allows`, so one private address refuses the whole host.
pub(crate) async fn admit(
  host: &str,
  port: u16,
  allows: impl Fn(IpAddr) -> bool,
) -> Result<Vec<SocketAddr>, Refusal> {
  let answer = if let Ok(ip) = host.parse::<IpAddr>() {
    vec![SocketAddr::new(ip, port)]
  } else {
    tokio::time::timeout(LOOKUP_TIMEOUT, tokio::net::lookup_host((host, port)))
      .await
      .map_err(|_| Refusal::Timeout)?
      .map_err(|error| Refusal::Lookup(error.kind()))?
      .collect()
  };
  admit_answer(answer, port, allows)
}

pub(crate) fn admit_answer(
  answer: Vec<SocketAddr>,
  port: u16,
  allows: impl Fn(IpAddr) -> bool,
) -> Result<Vec<SocketAddr>, Refusal> {
  if answer.is_empty() || answer.iter().any(|a| a.port() != port || !allows(a.ip())) {
    return Err(Refusal::Address);
  }
  Ok(answer)
}

/// The hardened builder every admitted request starts from. It connects to `addresses` only:
/// reqwest never resolves `host` again (no DNS rebinding between check and connect), no proxy
/// resolves it instead, and reqwest never follows a redirect itself, so each hop returns to the
/// caller's admission. No retries, no referer, no decompression (callers bound raw bytes),
/// HTTP/1.1, TLS 1.2+. Timeouts, `https_only` and headers stay with the caller.
pub(crate) fn pinned_builder(host: &str, addresses: &[SocketAddr]) -> reqwest::ClientBuilder {
  reqwest::Client::builder()
    .no_proxy()
    .redirect(reqwest::redirect::Policy::none())
    .referer(false)
    .retry(reqwest::retry::never())
    .no_gzip()
    .no_brotli()
    .no_deflate()
    .no_zstd()
    .http1_only()
    .min_tls_version(reqwest::tls::Version::TLS_1_2)
    .resolve_to_addrs(host, addresses)
}

/// Conservative port of Android MediaHttpPolicy, including mapped IPv4 and well-known NAT64.
pub(crate) fn public_address(ip: IpAddr) -> bool {
  match ip {
    IpAddr::V4(v4) => {
      let value = u32::from(v4);
      ![
        (0x00000000, 8),
        (0x0a000000, 8),
        (0x64400000, 10),
        (0x7f000000, 8),
        (0xa9fe0000, 16),
        (0xac100000, 12),
        (0xc0000000, 24),
        (0xc0000200, 24),
        (0xc0586300, 24),
        (0xc0a80000, 16),
        (0xc6120000, 15),
        (0xc6336400, 24),
        (0xcb007100, 24),
        (0xe0000000, 4),
        (0xf0000000, 4),
      ]
      .iter()
      .any(|(prefix, bits)| value & (u32::MAX << (32 - bits)) == *prefix)
    }
    IpAddr::V6(v6) => {
      if let Some(v4) = v6.to_ipv4_mapped() {
        return public_address(v4.into());
      }
      let b = v6.octets();
      if b[..12] == [0, 0x64, 0xff, 0x9b, 0, 0, 0, 0, 0, 0, 0, 0] {
        return public_address(Ipv4Addr::new(b[12], b[13], b[14], b[15]).into());
      }
      b[0] & 0xe0 == 0x20
        && !(b[0] == 0x20 && b[1] == 1 && b[2] & 0xfe == 0)
        && b[..4] != [0x20, 1, 0x0d, 0xb8]
        && b[..2] != [0x20, 2]
        && !(b[0] == 0x3f && b[1] == 0xff && b[2] & 0xf0 == 0)
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn browser_proxy_takes_only_what_the_engines_take() {
    let via = |socks, host: &str, port| {
      BrowserProxy::Via(ProxyAddress {
        socks,
        host: host.into(),
        port,
      })
    };
    for (enabled, proxy, expected) in [
      (None, Some("socks5://h:1"), BrowserProxy::Off),
      (Some(false), Some("socks5://h:1"), BrowserProxy::Off),
      (Some(false), Some("not a proxy"), BrowserProxy::Off),
      (
        Some(true),
        Some("127.0.0.1:8888"),
        via(false, "127.0.0.1", 8888),
      ),
      (
        Some(true),
        Some("http://proxy.example:3128/"),
        via(false, "proxy.example", 3128),
      ),
      // The usual port is written out: a URL would drop it, and wry would get an empty port.
      (
        Some(true),
        Some("http://proxy.example"),
        via(false, "proxy.example", 80),
      ),
      (
        Some(true),
        Some("http://proxy.example:80"),
        via(false, "proxy.example", 80),
      ),
      (
        Some(true),
        Some("socks5://127.0.0.1:1080"),
        via(true, "127.0.0.1", 1080),
      ),
      (
        Some(true),
        Some("socks5h://my-proxy.example:9050"),
        via(true, "my-proxy.example", 9050),
      ),
      (Some(true), Some("socks5://h"), via(true, "h", 1080)),
      (
        Some(true),
        Some("socks5://[::1]:1080"),
        via(true, "[::1]", 1080),
      ),
      (
        Some(true),
        Some("http://[2001:db8::1]:8080"),
        via(false, "[2001:db8::1]", 8080),
      ),
      // Well-formed, of a kind no engine takes: the engine's default.
      (Some(true), Some("https://h:443"), BrowserProxy::Unsupported),
      (
        Some(true),
        Some("https://user:pass@h"),
        BrowserProxy::Unsupported,
      ),
      (
        Some(true),
        Some("socks4://h:1080"),
        BrowserProxy::Unsupported,
      ),
      (
        Some(true),
        Some("socks4a://h:1080"),
        BrowserProxy::Unsupported,
      ),
      // On and unusable: no page loads.
      (Some(true), None, BrowserProxy::Blocked),
      (Some(true), Some("  "), BrowserProxy::Blocked),
      (Some(true), Some("http://"), BrowserProxy::Blocked),
      (Some(true), Some("ftp://h:21"), BrowserProxy::Blocked),
      (Some(true), Some("http://h:99999"), BrowserProxy::Blocked),
      (Some(true), Some("http://h h:8080"), BrowserProxy::Blocked),
      (
        Some(true),
        Some("http://h:8080/path"),
        BrowserProxy::Blocked,
      ),
      (
        Some(true),
        Some("http://h:8080/?x=1"),
        BrowserProxy::Blocked,
      ),
      // No engine takes a user name or password: never the proxy without them.
      (
        Some(true),
        Some("http://user:pass@h:8080"),
        BrowserProxy::Blocked,
      ),
      (
        Some(true),
        Some("http://user:p@ss@h:8080"),
        BrowserProxy::Blocked,
      ),
      (
        Some(true),
        Some("socks5://user@h:1080"),
        BrowserProxy::Blocked,
      ),
      // Port 0, and hosts that are not an address or a plain name.
      (Some(true), Some("http://h:0"), BrowserProxy::Blocked),
      (Some(true), Some("socks5://h:0"), BrowserProxy::Blocked),
      (Some(true), Some("https://h:0"), BrowserProxy::Blocked),
      (
        Some(true),
        Some("socks5://a;https=direct:1080"),
        BrowserProxy::Blocked,
      ),
      (
        Some(true),
        Some("socks5://a,direct:1080"),
        BrowserProxy::Blocked,
      ),
      (
        Some(true),
        Some("socks5://a\"b:1080"),
        BrowserProxy::Blocked,
      ),
      (Some(true), Some("socks5://a'b:1080"), BrowserProxy::Blocked),
      (Some(true), Some("socks5://a=b:1080"), BrowserProxy::Blocked),
      (
        Some(true),
        Some("socks5://a%20b:1080"),
        BrowserProxy::Blocked,
      ),
      (Some(true), Some("socks5://a_b:1080"), BrowserProxy::Blocked),
      (Some(true), Some("http://a;b:8080"), BrowserProxy::Blocked),
      (
        Some(true),
        Some("http://-a.example:8080"),
        BrowserProxy::Blocked,
      ),
      (Some(true), Some("https://a;b:443"), BrowserProxy::Blocked),
    ] {
      assert_eq!(
        browser_proxy_for(enabled, proxy),
        expected,
        "case {proxy:?}"
      );
    }
  }

  #[test]
  fn a_window_opened_with_the_proxy_on_fetches_nothing_beside_its_pages() {
    assert!(BrowserProxy::Off.fetches_beside());
    for proxy in [
      browser_proxy_for(Some(true), Some("socks5://127.0.0.1:1080")),
      BrowserProxy::Unsupported,
      BrowserProxy::Blocked,
    ] {
      assert!(!proxy.fetches_beside(), "{proxy:?}");
    }
  }

  #[test]
  fn own_proxy_follows_the_setting() {
    assert_eq!(own_proxy_for(None, Some("1.2.3.4:80")), OwnProxy::Direct);
    assert_eq!(
      own_proxy_for(Some(false), Some("1.2.3.4:80")),
      OwnProxy::Direct
    );
    assert_eq!(own_proxy_for(Some(true), None), OwnProxy::Blocked);
    assert_eq!(own_proxy_for(Some(true), Some(" ")), OwnProxy::Blocked);
    assert_eq!(
      own_proxy_for(Some(true), Some("127.0.0.1:8888")),
      OwnProxy::Via("http://127.0.0.1:8888".into())
    );
    assert_eq!(
      own_proxy_for(Some(true), Some("socks5://h:1")),
      OwnProxy::Via("socks5://h:1".into())
    );
  }

  #[test]
  fn public_address_rejects_private_android_ranges() {
    for ip in [
      "0.0.0.0",
      "10.1.2.3",
      "100.64.1.1",
      "127.0.0.1",
      "169.254.169.254",
      "172.16.0.1",
      "192.0.0.8",
      "192.168.1.1",
      "198.18.0.1",
      "192.0.2.1",
      "198.51.100.2",
      "203.0.113.1",
      "224.0.0.1",
      "255.255.255.255",
      "::",
      "::1",
      "fc00::1",
      "fe80::1",
      "ff02::1",
      "2001:db8::1",
      "2002:808:808::1",
      "2001::1",
      "3fff::1",
      "::ffff:127.0.0.1",
      "64:ff9b::a00:1",
    ] {
      assert!(!public_address(ip.parse().unwrap()), "{ip}");
    }
    for ip in [
      "8.8.8.8",
      "1.1.1.1",
      "2606:4700:4700::1111",
      "::ffff:8.8.8.8",
      "64:ff9b::808:808",
    ] {
      assert!(public_address(ip.parse().unwrap()), "{ip}");
    }
  }

  #[test]
  fn one_refused_or_off_port_address_refuses_the_whole_answer() {
    let public = |a: &str| a.parse::<SocketAddr>().unwrap();
    assert_eq!(
      admit_answer(vec![public("8.8.8.8:443")], 443, public_address),
      Ok(vec![public("8.8.8.8:443")])
    );
    for answer in [
      vec![],
      vec![public("8.8.8.8:443"), public("10.0.0.1:443")],
      vec![
        public("[2606:4700:4700::1111]:443"),
        public("[::ffff:127.0.0.1]:443"),
      ],
      vec![public("8.8.8.8:80")],
    ] {
      assert_eq!(
        admit_answer(answer.clone(), 443, public_address),
        Err(Refusal::Address),
        "{answer:?}"
      );
    }
  }

  #[tokio::test]
  async fn ip_literals_are_their_own_answer_and_hosts_lose_brackets() {
    let url = Url::parse("https://[::1]:8443/x").unwrap();
    assert_eq!(host(&url), Some("::1"));
    assert_eq!(
      admit("::1", 8443, |ip| ip.is_loopback()).await,
      Ok(vec!["[::1]:8443".parse().unwrap()])
    );
    assert_eq!(
      admit("::1", 8443, public_address).await,
      Err(Refusal::Address)
    );
  }
}
