//! Proxy HTTP locale su 127.0.0.1 che inoltra tutto al proxy pubblico scelto.
//! Così qualsiasi programma (browser compreso) può usare il proxy trovato.

use crate::model::{GoodProxy, Protocol};
use crate::store::{RelayInfo, Store, SwitchRequest};
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

pub type Upstream = Arc<RwLock<Option<GoodProxy>>>;

const MAX_HEAD: usize = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Ascolta su `bind` (solo loopback) finché `cancel` non scatta.
pub async fn run_relay(bind: SocketAddr, upstream: Upstream, cancel: CancellationToken) -> io::Result<()> {
    let listener = bind_relay(bind).await?;
    serve(listener, upstream, cancel).await
}

/// Apre la porta del relay: separato da `serve` così chi chiama scopre subito se è occupata.
pub async fn bind_relay(bind: SocketAddr) -> io::Result<TcpListener> {
    if !bind.ip().is_loopback() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "il relay ascolta solo su 127.0.0.1"));
    }
    TcpListener::bind(bind).await
}

pub async fn serve(listener: TcpListener, upstream: Upstream, cancel: CancellationToken) -> io::Result<()> {
    loop {
        let (sock, _) = tokio::select! {
            r = listener.accept() => r?,
            _ = cancel.cancelled() => return Ok(()),
        };
        let up = upstream.read().ok().and_then(|g| g.clone());
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = handle(sock, up) => {}
                _ = cancel.cancelled() => {}
            }
        });
    }
}

/// Relay da riga di comando: annuncia porta e proxy attivo in `relay.json` e applica le
/// richieste di cambio scritte da un'altra istanza (`request_switch`). Ogni secondo guarda solo
/// la data di modifica della richiesta: il file si apre quando c'è ed è cambiato.
/// `on_switch(vecchio, nuovo)` viene chiamato a ogni cambio. Esce quando `cancel` scatta.
pub async fn watch_switch(store: Store, port: u16, upstream: Upstream, cancel: CancellationToken, on_switch: impl Fn(Option<&GoodProxy>, &GoodProxy)) {
    let announce = |up: &GoodProxy| store.save_relay(&RelayInfo { port, pid: std::process::id(), upstream: up.clone() });
    if let Some(up) = upstream.read().ok().and_then(|g| g.clone()) {
        let _ = announce(&up);
    }
    store.clear_switch();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut seen = None;
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = cancel.cancelled() => return,
        }
        let m = store.switch_modified();
        if m.is_none() || m == seen {
            continue;
        }
        seen = m;
        let Some(req) = store.take_switch(port) else { continue };
        let old = upstream.write().ok().and_then(|mut g| g.replace(req.upstream.clone()));
        let _ = announce(&req.upstream);
        on_switch(old.as_ref(), &req.upstream);
    }
}

/// Chiede al relay da riga di comando che tiene la porta `port` di passare a `up` e aspetta la
/// conferma (al più `wait`). Restituisce il pid dell'istanza che ha cambiato proxy.
pub async fn request_switch(store: &Store, port: u16, up: &GoodProxy, wait: Duration) -> Result<u32, String> {
    let Some(info) = store.load_relay().filter(|r| r.port == port) else {
        return Err(format!("porta {port} occupata da un altro programma (la GUI?), non da un relay proxy-finder da riga di comando"));
    };
    let mut seen = store.relay_modified();
    store.save_switch(&SwitchRequest { port, upstream: up.clone() }).map_err(|e| e.to_string())?;
    let t0 = std::time::Instant::now();
    while t0.elapsed() < wait {
        tokio::time::sleep(Duration::from_millis(100)).await;
        // relay.json si riapre solo quando il relay lo riscrive (cioè dopo il cambio)
        let m = store.relay_modified();
        if m == seen {
            continue;
        }
        seen = m;
        if let Some(r) = store.load_relay().filter(|r| r.port == port && r.upstream.key() == up.key()) {
            return Ok(r.pid);
        }
    }
    store.clear_switch();
    Err(format!("il relay sulla porta {port} (pid {}) non ha risposto alla richiesta di cambio proxy", info.pid))
}

struct Head {
    method: String,
    target: String,
    version: String,
    /// Righe di intestazione senza la request-line.
    headers: Vec<String>,
    /// Byte già letti dopo la fine dell'intestazione.
    rest: Vec<u8>,
}

async fn read_head<S: AsyncRead + Unpin>(s: &mut S) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(pos + 4);
            return Ok((buf, rest));
        }
        if buf.len() > MAX_HEAD {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "intestazione troppo lunga"));
        }
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn parse_head(head: &[u8], rest: Vec<u8>) -> Option<Head> {
    let text = std::str::from_utf8(head).ok()?;
    let mut lines = text.split("\r\n").filter(|l| !l.is_empty());
    let mut first = lines.next()?.split_whitespace();
    let (method, target, version) = (first.next()?, first.next()?, first.next().unwrap_or("HTTP/1.1"));
    Some(Head {
        method: method.to_string(),
        target: target.to_string(),
        version: version.to_string(),
        headers: lines.map(String::from).collect(),
        rest,
    })
}

async fn reply<S: AsyncWrite + Unpin>(s: &mut S, status: &str, body: &str) -> io::Result<()> {
    let msg = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(msg.as_bytes()).await
}

/// "host:porta" (porta di default se manca), gestendo gli IPv6 tra parentesi.
fn split_host_port(s: &str, default_port: u16) -> Option<(String, u16)> {
    if let Some(rest) = s.strip_prefix('[') {
        let (h, after) = rest.split_once(']')?;
        let port = after.strip_prefix(':').map(|p| p.parse().ok()).unwrap_or(Some(default_port))?;
        return Some((h.to_string(), port));
    }
    match s.rsplit_once(':') {
        Some((h, p)) => Some((h.to_string(), p.parse().ok()?)),
        None => Some((s.to_string(), default_port)),
    }
}

/// Da "http://host:porta/percorso" a ((host, porta), "/percorso").
fn split_absolute(url: &str) -> Option<((String, u16), String)> {
    let rest = url.strip_prefix("http://")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None => (rest, "/".to_string()),
    };
    let authority = authority.rsplit('@').next()?;
    Some((split_host_port(authority, 80)?, path))
}

async fn tcp(addr: SocketAddr) -> io::Result<TcpStream> {
    tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "timeout verso il proxy"))?
}

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// Tunnel verso host:porta attraverso l'upstream (CONNECT per http, SOCKS altrimenti).
async fn open_tunnel(up: &GoodProxy, host: &str, port: u16) -> io::Result<Box<dyn Stream>> {
    let proxy = SocketAddr::from((up.ip, up.port));
    let other = |e: tokio_socks::Error| io::Error::other(e.to_string());
    match up.protocol {
        Protocol::Http => {
            let mut s = tcp(proxy).await?;
            let req = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n\r\n");
            s.write_all(req.as_bytes()).await?;
            let (head, rest) = read_head(&mut s).await?;
            let status = String::from_utf8_lossy(&head);
            let ok = status.split_whitespace().nth(1).is_some_and(|c| c.starts_with('2'));
            if !ok {
                let line = status.lines().next().unwrap_or("").to_string();
                return Err(io::Error::other(format!("CONNECT rifiutato: {line}")));
            }
            if rest.is_empty() {
                Ok(Box::new(s))
            } else {
                Ok(Box::new(Prefixed { prefix: rest, inner: s }))
            }
        }
        Protocol::Socks5 => {
            let s = tcp(proxy).await?;
            let st = tokio_socks::tcp::Socks5Stream::connect_with_socket(s, (host, port)).await.map_err(other)?;
            Ok(Box::new(st))
        }
        Protocol::Socks4 => {
            // SOCKS4 vuole un IPv4: si risolve in locale
            let target = tokio::net::lookup_host((host, port))
                .await?
                .find(|a| a.is_ipv4())
                .ok_or_else(|| io::Error::other(format!("{host}: nessun indirizzo IPv4")))?;
            let s = tcp(proxy).await?;
            let st = tokio_socks::tcp::Socks4Stream::connect_with_socket(s, target).await.map_err(other)?;
            Ok(Box::new(st))
        }
    }
}

/// Flusso con alcuni byte già letti da restituire per primi.
struct Prefixed<S> {
    prefix: Vec<u8>,
    inner: S,
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        if !self.prefix.is_empty() {
            let n = self.prefix.len().min(buf.remaining());
            buf.put_slice(&self.prefix[..n]);
            self.prefix.drain(..n);
            return std::task::Poll::Ready(Ok(()));
        }
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

async fn handle(mut client: TcpStream, up: Option<GoodProxy>) -> io::Result<()> {
    let (head, rest) = read_head(&mut client).await?;
    let Some(req) = parse_head(&head, rest) else {
        return reply(&mut client, "400 Bad Request", "Richiesta non valida").await;
    };
    let Some(up) = up else {
        return reply(&mut client, "503 Service Unavailable", "Nessun proxy connesso").await;
    };

    if req.method.eq_ignore_ascii_case("CONNECT") {
        let Some((host, port)) = split_host_port(&req.target, 443) else {
            return reply(&mut client, "400 Bad Request", "Destinazione non valida").await;
        };
        let mut remote = match open_tunnel(&up, &host, port).await {
            Ok(r) => r,
            Err(e) => return reply(&mut client, "502 Bad Gateway", &e.to_string()).await,
        };
        client.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await?;
        if !req.rest.is_empty() {
            remote.write_all(&req.rest).await?;
        }
        tokio::io::copy_bidirectional(&mut client, &mut remote).await?;
        return Ok(());
    }

    let Some(((host, port), path)) = split_absolute(&req.target) else {
        return reply(&mut client, "400 Bad Request", "Serve un URL assoluto http://").await;
    };

    let mut remote: Box<dyn Stream>;
    let mut out = Vec::with_capacity(head.len() + 64);
    if up.protocol == Protocol::Http {
        // un proxy http accetta la richiesta così com'è
        remote = match tcp(SocketAddr::from((up.ip, up.port))).await {
            Ok(s) => Box::new(s),
            Err(e) => return reply(&mut client, "502 Bad Gateway", &e.to_string()).await,
        };
        out.extend_from_slice(&head);
    } else {
        remote = match open_tunnel(&up, &host, port).await {
            Ok(r) => r,
            Err(e) => return reply(&mut client, "502 Bad Gateway", &e.to_string()).await,
        };
        // verso il server la request-line va in forma origine; una richiesta per connessione
        out.extend_from_slice(format!("{} {} {}\r\n", req.method, path, req.version).as_bytes());
        for h in req.headers.iter().filter(|h| {
            let n = h.split(':').next().unwrap_or("").trim().to_ascii_lowercase();
            n != "proxy-connection" && n != "connection" && n != "keep-alive"
        }) {
            out.extend_from_slice(h.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"Connection: close\r\n\r\n");
    }
    out.extend_from_slice(&req.rest);
    remote.write_all(&out).await?;
    tokio::io::copy_bidirectional(&mut client, &mut remote).await?;
    Ok(())
}
