#[derive(Debug, Clone)]
pub struct ProxyEndpoint {
  /// Proxy server host (e.g. 192.168.0.100, localhost, example.com, etc.)
  pub host: String,
  /// Proxy server port (e.g. 1080, 3128, etc.)
  pub port: String,
}

#[derive(Debug, Clone)]
pub enum ProxyConfig {
  /// Connect to proxy server via HTTP CONNECT
  Http(ProxyEndpoint),
  /// Connect to proxy server via SOCKSv5
  Socks5(ProxyEndpoint),
}

// Nezzel patch: Tauri hands a proxy over as the host and port of a URL, so the port is empty
// when it is the scheme's usual one (`http://host:80`) and an IPv6 host comes in brackets.
impl ProxyConfig {
  /// The port to connect to: the scheme's usual one when none was given.
  pub fn port(&self) -> &str {
    match self {
      Self::Http(endpoint) if endpoint.port.is_empty() => "80",
      Self::Socks5(endpoint) if endpoint.port.is_empty() => "1080",
      Self::Http(endpoint) | Self::Socks5(endpoint) => &endpoint.port,
    }
  }
}

impl ProxyEndpoint {
  /// The host as a resolver takes it: an IPv6 address without its brackets.
  pub fn bare_host(&self) -> &str {
    self
      .host
      .strip_prefix('[')
      .and_then(|host| host.strip_suffix(']'))
      .unwrap_or(&self.host)
  }
}
