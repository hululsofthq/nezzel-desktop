//! A process-scoped SOCKS route so yt-dlp connects to the checked IPs, including for redirects
//! and manifest children it discovers itself.

use crate::net;
use futures::future::BoxFuture;
use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::Instant;

/// From the first byte to the answer of the CONNECT.
const HANDSHAKE: Duration = Duration::from_secs(15);
/// A connection that carried nothing either way for this long gives its slot back.
const IDLE: Duration = Duration::from_secs(60);
/// yt-dlp waits between the parts of one download on a connection it keeps.
const DOWNLOAD_IDLE: Duration = Duration::from_secs(300);
/// After an accept that failed (no descriptors left, a client gone before the accept).
const ACCEPT_PAUSE: Duration = Duration::from_millis(100);

/// Resolves and checks one destination; the answer is the only addresses the route connects to.
type Admission =
  Arc<dyn Fn(String, u16) -> BoxFuture<'static, io::Result<Vec<SocketAddr>>> + Send + Sync>;

pub(crate) struct Route {
  task: JoinHandle<()>,
  pub(crate) proxy: String,
  pub(crate) address: SocketAddr,
}

impl Drop for Route {
  fn drop(&mut self) {
    self.task.abort();
  }
}

/// Public addresses only, resolved once here: the connection goes to the checked answer, so a
/// name that turns private between check and connect (DNS rebinding) never reaches it.
async fn public(host: String, port: u16) -> io::Result<Vec<SocketAddr>> {
  net::admit(&host, port, net::public_address)
    .await
    .map_err(|_| io::Error::from(io::ErrorKind::PermissionDenied))
}

impl Route {
  pub(super) async fn start(
    host: String,
    port: u16,
    addresses: Vec<SocketAddr>,
  ) -> io::Result<Self> {
    let secret = uuid::Uuid::new_v4().simple().to_string();
    let pinned = Arc::new((host, port, addresses));
    let admission: Admission = Arc::new(move |host, port| {
      let pinned = pinned.clone();
      Box::pin(async move {
        if pinned.0.eq_ignore_ascii_case(&host) && pinned.1 == port {
          Ok(pinned.2.clone())
        } else {
          public(host, port).await
        }
      })
    });
    let (task, address) = serve(Some(secret.clone()), admission, 64, DOWNLOAD_IDLE).await?;
    let proxy = format!("socks5h://catch:{secret}@{address}");
    Ok(Self {
      task,
      proxy,
      address,
    })
  }

  /// The route the hidden page's engine uses for every connection of every frame, redirect,
  /// subresource and socket. Engines offer no SOCKS credentials, so it takes none; it is bound
  /// to loopback and admits public addresses only.
  pub(crate) async fn guard() -> io::Result<Self> {
    Self::guarded(Arc::new(|host, port| Box::pin(public(host, port)))).await
  }

  async fn guarded(admission: Admission) -> io::Result<Self> {
    // A page opens many connections at once.
    let (task, address) = serve(None, admission, 256, IDLE).await?;
    Ok(Self {
      task,
      proxy: format!("socks5://{address}"),
      address,
    })
  }

  /// False once the accept loop has ended: nothing connects through this route any more.
  pub(crate) fn alive(&self) -> bool {
    !self.task.is_finished()
  }
}

/// The guard route of the process: started with the first hidden page, and started again
/// should its accept loop ever have ended, so a dead route is never handed out.
#[derive(Default)]
pub(crate) struct Guard(tokio::sync::Mutex<Option<Arc<Route>>>);

impl Guard {
  pub(crate) async fn route(&self) -> io::Result<Arc<Route>> {
    let mut kept = self.0.lock().await;
    if let Some(route) = kept.as_ref().filter(|route| route.alive()) {
      return Ok(route.clone());
    }
    let route = Arc::new(Route::guard().await?);
    *kept = Some(route.clone());
    Ok(route)
  }
}

async fn serve(
  secret: Option<String>,
  admission: Admission,
  limit: usize,
  idle: Duration,
) -> io::Result<(JoinHandle<()>, SocketAddr)> {
  let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
  let address = listener.local_addr()?;
  let task = tokio::spawn(async move {
    accept_loop(|| listener.accept(), secret, admission, limit, idle).await;
  });
  Ok((task, address))
}

/// Serves until its task is aborted. `accept` is the listener's (a test stands in for it).
async fn accept_loop<A, Incoming>(
  accept: A,
  secret: Option<String>,
  admission: Admission,
  limit: usize,
  idle: Duration,
) where
  A: Fn() -> Incoming,
  Incoming: Future<Output = io::Result<(TcpStream, SocketAddr)>>,
{
  let mut connections = JoinSet::new();
  loop {
    tokio::select! {
      _ = connections.join_next(), if !connections.is_empty() => {}
      incoming = accept(), if connections.len() < limit => {
        let mut client = match incoming {
          Ok((client, _)) => client,
          Err(error) => {
            // One failed accept says nothing about the next: the route stays up.
            tracing::warn!("catch route: accept failed ({:?})", error.kind());
            tokio::time::sleep(ACCEPT_PAUSE).await;
            continue;
          }
        };
        let admission = admission.clone();
        let secret = secret.clone();
        connections.spawn(async move {
          let connected = tokio::time::timeout(HANDSHAKE, async {
            let (host, port) = destination(&mut client, secret.as_deref()).await?;
            let addresses = admission(host, port).await?;
            TcpStream::connect(addresses.as_slice()).await
          }).await;
          let Ok(Ok(mut upstream)) = connected else {
            let _ = reply(&mut client, 2).await;
            return;
          };
          if reply(&mut client, 0).await.is_ok() {
            relay(&mut client, &mut upstream, idle).await;
          }
        });
      }
    }
  }
}

/// A client that stopped reading does not hold its slot on the answer either.
async fn reply(client: &mut TcpStream, status: u8) -> io::Result<()> {
  tokio::time::timeout(
    HANDSHAKE,
    client.write_all(&[5, status, 0, 1, 0, 0, 0, 0, 0, 0]),
  )
  .await?
}

/// Copies both ways until both sides are done, one fails, or nothing moved for `idle`.
async fn relay(client: &mut TcpStream, upstream: &mut TcpStream, idle: Duration) {
  let moved = Mutex::new(Instant::now());
  let last = || moved.lock().map_or_else(|_| Instant::now(), |moved| *moved);
  let (mut from_client, mut to_client) = client.split();
  let (mut from_upstream, mut to_upstream) = upstream.split();
  let copy = async {
    tokio::try_join!(
      pump(&mut from_client, &mut to_upstream, &moved),
      pump(&mut from_upstream, &mut to_client, &moved),
    )
  };
  let silence = async {
    while last() + idle > Instant::now() {
      tokio::time::sleep_until(last() + idle).await;
    }
  };
  tokio::select! {
    _ = copy => {}
    () = silence => {}
  }
}

async fn pump(
  from: &mut (impl AsyncReadExt + Unpin),
  to: &mut (impl AsyncWriteExt + Unpin),
  moved: &Mutex<Instant>,
) -> io::Result<()> {
  let mark = || {
    if let Ok(mut moved) = moved.lock() {
      *moved = Instant::now();
    }
  };
  let mut buffer = vec![0; 16 * 1024];
  loop {
    let size = from.read(&mut buffer).await?;
    if size == 0 {
      return to.shutdown().await;
    }
    mark();
    to.write_all(&buffer[..size]).await?;
    mark();
  }
}

async fn destination(
  client: &mut (impl AsyncReadExt + AsyncWriteExt + Unpin),
  secret: Option<&str>,
) -> io::Result<(String, u16)> {
  let invalid = || io::Error::from(io::ErrorKind::InvalidData);
  if client.read_u8().await? != 5 {
    return Err(invalid());
  }
  let methods = read_string(client).await?;
  match secret {
    Some(secret) => {
      if !methods.contains(&2) {
        return Err(invalid());
      }
      client.write_all(&[5, 2]).await?;
      if client.read_u8().await? != 1
        || read_string(client).await? != b"catch"
        || read_string(client).await? != secret.as_bytes()
      {
        return Err(invalid());
      }
      client.write_all(&[1, 0]).await?;
    }
    None => {
      if !methods.contains(&0) {
        return Err(invalid());
      }
      client.write_all(&[5, 0]).await?;
    }
  }
  let mut request = [0; 4];
  client.read_exact(&mut request).await?;
  let [5, 1, 0, kind] = request else {
    return Err(invalid());
  };
  let host = match kind {
    1 => {
      let mut bytes = [0; 4];
      client.read_exact(&mut bytes).await?;
      Ipv4Addr::from(bytes).to_string()
    }
    3 => {
      let host = String::from_utf8(read_string(client).await?).map_err(|_| invalid())?;
      if host.is_empty()
        || !host
          .bytes()
          .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'))
      {
        return Err(invalid());
      }
      host
    }
    4 => {
      let mut bytes = [0; 16];
      client.read_exact(&mut bytes).await?;
      Ipv6Addr::from(bytes).to_string()
    }
    _ => return Err(invalid()),
  };
  Ok((host, client.read_u16().await?))
}

async fn read_string(client: &mut (impl AsyncReadExt + Unpin)) -> io::Result<Vec<u8>> {
  let size = client.read_u8().await?;
  let mut bytes = vec![0; size.into()];
  client.read_exact(&mut bytes).await?;
  Ok(bytes)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn authenticates_and_reads_the_unresolved_destination() {
    let (mut client, mut server) = tokio::io::duplex(128);
    let (destination, ()) = tokio::join!(destination(&mut server, Some("secret")), async {
      client.write_all(&[5, 2, 0, 2]).await.unwrap();
      assert_eq!(client.read_u16().await.unwrap(), 0x0502);
      client.write_all(b"\x01\x05catch\x06secret").await.unwrap();
      assert_eq!(client.read_u16().await.unwrap(), 0x0100);
      client
        .write_all(b"\x05\x01\x00\x03\x09media.com\x01\xbb")
        .await
        .unwrap();
    });
    assert_eq!(destination.unwrap(), ("media.com".into(), 443));
  }

  /// The engine's side: a SOCKS5 CONNECT without credentials. The reply status, and the
  /// stream when the route connected.
  async fn connect(route: &Route, host: &str, port: u16) -> (u8, TcpStream) {
    let mut client = TcpStream::connect(route.address).await.unwrap();
    client.write_all(&[5, 1, 0]).await.unwrap();
    assert_eq!(client.read_u16().await.unwrap(), 0x0500);
    let mut request = vec![5, 1, 0, 3, host.len() as u8];
    request.extend_from_slice(host.as_bytes());
    request.extend_from_slice(&port.to_be_bytes());
    client.write_all(&request).await.unwrap();
    let mut answer = [0; 10];
    client.read_exact(&mut answer).await.unwrap();
    (answer[1], client)
  }

  /// A connection the route made would already wait in this listener's queue.
  fn never_reached(listener: &std::net::TcpListener) -> bool {
    listener.set_nonblocking(true).unwrap();
    matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
  }

  #[tokio::test]
  async fn the_page_route_never_connects_to_a_name_that_resolves_private_even_after_a_redirect() {
    let private = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let private_address = private.local_addr().unwrap();
    // A public page that redirects to the private name.
    let public = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let public_address = public.local_addr().unwrap();
    let server = tokio::spawn(async move {
      let (mut page, _) = public.accept().await.unwrap();
      let mut request = [0; 256];
      let _ = page.read(&mut request).await.unwrap();
      page
        .write_all(b"HTTP/1.1 302 Found\r\nLocation: http://private.example/admin\r\nContent-Length: 0\r\n\r\n")
        .await
        .unwrap();
    });
    // In-process resolver: what each name answers, and where the test really listens.
    let resolver: Admission = Arc::new(move |host, port| {
      Box::pin(async move {
        let (answer, listening): (std::net::IpAddr, SocketAddr) = match host.as_str() {
          "public.example" => ([93, 184, 216, 34].into(), public_address),
          "private.example" => ([192, 168, 1, 10].into(), private_address),
          _ => return Err(io::ErrorKind::NotFound.into()),
        };
        net::admit_answer(
          vec![SocketAddr::new(answer, port)],
          port,
          net::public_address,
        )
        .map(|_| vec![listening])
        .map_err(|_| io::Error::from(io::ErrorKind::PermissionDenied))
      })
    });
    let route = Route::guarded(resolver).await.unwrap();

    let (status, _) = connect(&route, "private.example", 80).await;
    assert_ne!(status, 0);
    assert!(never_reached(&private));

    let (status, mut page) = connect(&route, "public.example", 80).await;
    assert_eq!(status, 0);
    page
      .write_all(b"GET / HTTP/1.1\r\nHost: public.example\r\n\r\n")
      .await
      .unwrap();
    let mut response = String::new();
    page.read_to_string(&mut response).await.unwrap();
    server.await.unwrap();
    assert!(response.contains("Location: http://private.example/admin"));
    let (status, _) = connect(&route, "private.example", 80).await;
    assert_ne!(status, 0);
    assert!(never_reached(&private));
  }

  /// A "public" name that really answers on a local test listener.
  fn local(listening: SocketAddr) -> Admission {
    Arc::new(move |_, _| Box::pin(async move { Ok(vec![listening]) }))
  }

  #[tokio::test]
  async fn a_failed_accept_does_not_end_the_route() {
    let site = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let listener = Arc::new(TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap());
    let address = listener.local_addr().unwrap();
    let failed = std::sync::atomic::AtomicBool::new(false);
    let accept = move || {
      let listener = listener.clone();
      let fail = !failed.swap(true, std::sync::atomic::Ordering::Relaxed);
      async move {
        if fail {
          return Err(io::ErrorKind::ConnectionAborted.into());
        }
        listener.accept().await
      }
    };
    tokio::time::pause();
    let task = tokio::spawn(accept_loop(
      accept,
      None,
      local(site.local_addr().unwrap()),
      4,
      IDLE,
    ));
    // The route meets the error and waits out its pause; nothing else is in flight meanwhile.
    tokio::time::sleep(Duration::from_secs(1)).await;
    tokio::time::resume();
    let route = Route {
      task,
      proxy: String::new(),
      address,
    };
    assert!(route.alive());
    let (status, _) = connect(&route, "site.example", 80).await;
    assert_eq!(status, 0);
  }

  #[tokio::test]
  async fn a_silent_connection_gives_its_slot_back() {
    let site = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    // One slot: a second connection is only served once the first is gone.
    let (task, address) = serve(None, local(site.local_addr().unwrap()), 1, IDLE)
      .await
      .unwrap();
    let route = Route {
      task,
      proxy: String::new(),
      address,
    };
    let (status, mut stalled) = connect(&route, "site.example", 80).await;
    assert_eq!(status, 0);
    let (_held, _) = site.accept().await.unwrap();

    // Both ends are connected and silent from here on: only the clock moves.
    tokio::time::pause();
    tokio::time::sleep(IDLE * 2).await;
    tokio::time::resume();
    // Ends at once when the route let go; the limit only stops a route that did not.
    let mut rest = Vec::new();
    let closed = tokio::time::timeout(Duration::from_secs(30), stalled.read_to_end(&mut rest));
    assert!(closed.await.is_ok(), "the route kept a silent connection");
    let (status, _) = connect(&route, "site.example", 80).await;
    assert_eq!(status, 0);
  }

  #[tokio::test]
  async fn a_route_that_ended_is_started_again() {
    let guard = Guard::default();
    let first = guard.route().await.unwrap();
    assert_eq!(guard.route().await.unwrap().address, first.address);
    first.task.abort();
    while first.alive() {
      tokio::task::yield_now().await;
    }
    let second = guard.route().await.unwrap();
    assert_ne!(second.address, first.address);
    assert!(second.alive());
  }

  #[tokio::test]
  async fn the_page_route_refuses_private_address_literals() {
    let route = Route::guard().await.unwrap();
    let private = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = private.local_addr().unwrap().port();
    for host in ["127.0.0.1", "0.0.0.0", "10.0.0.1"] {
      let (status, _) = connect(&route, host, port).await;
      assert_ne!(status, 0, "{host}");
    }
    assert!(never_reached(&private));
  }
}
