//! Server finti per i test: un'eco HTTP e un proxy HTTP minimale (forma assoluta + CONNECT).
#![allow(dead_code)]

use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn read_head(s: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut b = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = s.read(&mut b).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&b[..n]);
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Eco che risponde sempre con `body` (JSON o testo). Restituisce l'indirizzo.
pub async fn echo_server(body: &'static str) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                if read_head(&mut s).await.is_some() {
                    let msg = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = s.write_all(msg.as_bytes()).await;
                }
            });
        }
    });
    addr
}

/// Proxy HTTP finto: inoltra `GET http://host:port/...` e apre tunnel `CONNECT`.
pub async fn http_proxy() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let Some(head) = read_head(&mut s).await else { return };
                let first = head.lines().next().unwrap_or("").to_string();
                let mut parts = first.split_whitespace();
                let method = parts.next().unwrap_or("");
                let target = parts.next().unwrap_or("");
                if method == "CONNECT" {
                    let Ok(mut up) = TcpStream::connect(target).await else {
                        let _ = s.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
                        return;
                    };
                    let _ = s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await;
                    let _ = tokio::io::copy_bidirectional(&mut s, &mut up).await;
                    return;
                }
                let rest = target.strip_prefix("http://").unwrap_or("");
                let (authority, path) = match rest.find('/') {
                    Some(i) => (&rest[..i], &rest[i..]),
                    None => (rest, "/"),
                };
                let Ok(mut up) = TcpStream::connect(authority).await else {
                    let _ = s.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n").await;
                    return;
                };
                let rewritten = head.replacen(target, path, 1);
                let _ = up.write_all(rewritten.as_bytes()).await;
                let _ = tokio::io::copy_bidirectional(&mut s, &mut up).await;
            });
        }
    });
    addr
}

/// Porta locale sicuramente chiusa.
pub async fn closed_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap().port()
}

/// SOCKS5 finto senza autenticazione: rifiuta le connessioni verso `refuse_port`
/// (risposta 0x02, "non permesso") e inoltra le altre a 127.0.0.1. Registra le porte richieste.
pub async fn socks5_proxy(refuse_port: u16) -> (SocketAddr, std::sync::Arc<std::sync::Mutex<Vec<u16>>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            let log = log.clone();
            tokio::spawn(async move {
                let mut b = [0u8; 2];
                if s.read_exact(&mut b).await.is_err() {
                    return;
                }
                let mut methods = vec![0u8; b[1] as usize];
                if s.read_exact(&mut methods).await.is_err() || s.write_all(&[5, 0]).await.is_err() {
                    return;
                }
                let mut head = [0u8; 4];
                if s.read_exact(&mut head).await.is_err() {
                    return;
                }
                let skip = match head[3] {
                    1 => 4,
                    4 => 16,
                    3 => {
                        let mut n = [0u8; 1];
                        if s.read_exact(&mut n).await.is_err() {
                            return;
                        }
                        n[0] as usize
                    }
                    _ => return,
                };
                let mut rest = vec![0u8; skip + 2];
                if s.read_exact(&mut rest).await.is_err() {
                    return;
                }
                let port = u16::from_be_bytes([rest[skip], rest[skip + 1]]);
                log.lock().unwrap().push(port);
                let reply = |code: u8| [5, code, 0, 1, 0, 0, 0, 0, 0, 0];
                if port == refuse_port {
                    let _ = s.write_all(&reply(2)).await;
                    return;
                }
                let Ok(mut up) = TcpStream::connect(("127.0.0.1", port)).await else {
                    let _ = s.write_all(&reply(5)).await;
                    return;
                };
                if s.write_all(&reply(0)).await.is_ok() {
                    let _ = tokio::io::copy_bidirectional(&mut s, &mut up).await;
                }
            });
        }
    });
    (addr, seen)
}
