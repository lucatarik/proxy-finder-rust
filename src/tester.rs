//! Test dei proxy: passaggio A in http (o in https se il proxy rifiuta l'http in chiaro),
//! passaggio B in https (CONNECT), pool a N corsie.

use crate::model::{now_secs, Candidate, GoodProxy};
use crate::rating::proxy_score;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const UA: &str = concat!("proxy-finder/", env!("CARGO_PKG_VERSION"));
const WORKERS: usize = 25;

/// Durate serializzate in millisecondi.
mod ms {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;
    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_millis() as u64)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        Ok(Duration::from_millis(u64::deserialize(d)?))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TestConfig {
    /// Corsie in parallelo, 8..=512.
    pub concurrency: usize,
    #[serde(with = "ms", rename = "http_timeout_ms")]
    pub http_timeout: Duration,
    #[serde(with = "ms", rename = "https_timeout_ms")]
    pub https_timeout: Duration,
    pub retry_after_timeout: bool,
    pub drop_http_only: bool,
    /// "host/percorso", senza schema.
    pub http_echo: Vec<String>,
    pub https_echo: Vec<String>,
}

impl Default for TestConfig {
    fn default() -> Self {
        TestConfig {
            concurrency: 64,
            http_timeout: Duration::from_secs(5),
            https_timeout: Duration::from_secs(8),
            retry_after_timeout: false,
            drop_http_only: false,
            http_echo: default_http_echo(),
            https_echo: default_https_echo(),
        }
    }
}

fn workers() -> impl Iterator<Item = String> {
    (1..=WORKERS).map(|i| format!("st-echo-{i:02}.lucatarik.workers.dev/"))
}

pub fn default_http_echo() -> Vec<String> {
    workers()
        .chain(["api.ipify.org/?format=text", "icanhazip.com/", "checkip.amazonaws.com/"].map(String::from))
        .collect()
}

pub fn default_https_echo() -> Vec<String> {
    workers()
        .chain(["api.ipify.org/?format=text", "icanhazip.com/", "ifconfig.me/ip", "ipecho.net/plain"].map(String::from))
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Good(GoodProxy),
    Dead { reason: String },
    Cancelled,
}

fn is_ipv4(s: &str) -> bool {
    s.parse::<std::net::Ipv4Addr>().is_ok()
}

fn find_ip(t: &str) -> Option<String> {
    t.split(|c: char| !(c.is_ascii_hexdigit() || c == '.' || c == ':'))
        .map(|w| w.trim_matches(':'))
        .find(|w| is_ipv4(w) || (w.contains(':') && w.parse::<std::net::Ipv6Addr>().is_ok()))
        .map(String::from)
}

/// Estrae (ip, nazione) dalla risposta di un sito di eco: JSON dei worker `{ip, nazione}`,
/// JSON con `ip|origin|query` e `country|country_code`, `ip=` del trace Cloudflare, testo semplice.
pub fn parse_echo(text: &str) -> Option<(String, String)> {
    let t = text.trim();
    if t.starts_with('{') {
        if let Ok(j) = serde_json::from_str::<serde_json::Value>(t) {
            let pick = |keys: &[&str]| {
                keys.iter()
                    .find_map(|k| j.get(*k).and_then(|v| v.as_str()).filter(|s| !s.is_empty()))
                    .unwrap_or("")
                    .to_string()
            };
            let ip = pick(&["ip", "origin", "query"]);
            let ip = ip.split(',').next().unwrap_or("").trim();
            let cc = pick(&["nazione", "country", "country_code"]);
            let cc = if cc.len() == 2 && cc.chars().all(|c| c.is_ascii_alphabetic()) { cc.to_ascii_uppercase() } else { String::new() };
            if let Some(ip) = find_ip(ip) {
                return Some((ip, cc));
            }
        }
    }
    for line in t.lines() {
        if let Some(v) = line.trim().strip_prefix("ip=") {
            if let Some(ip) = find_ip(v) {
                return Some((ip, String::new()));
            }
        }
    }
    find_ip(t).map(|ip| (ip, String::new()))
}

/// IP reale, chiesto in diretta. Senza, i test non partono.
pub async fn real_ip(client: &reqwest::Client) -> Result<String, String> {
    let mut last = String::from("nessuna risposta");
    for url in ["https://api.ipify.org/?format=text", "https://1.1.1.1/cdn-cgi/trace", "https://icanhazip.com/"] {
        match client.get(url).timeout(Duration::from_secs(10)).send().await {
            Ok(r) if r.status().is_success() => {
                if let Some((ip, _)) = parse_echo(&r.text().await.unwrap_or_default()) {
                    return Ok(ip);
                }
                last = format!("{url}: risposta senza IP");
            }
            Ok(r) => last = format!("{url}: HTTP {}", r.status().as_u16()),
            Err(e) => last = format!("{url}: {e}"),
        }
    }
    Err(format!("IP reale non disponibile ({last})"))
}

enum ProbeErr {
    Timeout,
    /// Il proxy stesso non si raggiunge (connessione rifiutata, rete o host irraggiungibile).
    Unreachable(String),
    /// Il proxy ha accettato la connessione ma la richiesta è fallita (tunnel o SOCKS rifiutato…).
    Network(String),
    /// Il proxy ha risposto, ma non con un IP valido (stato HTTP, risposta senza IP).
    Bad(String),
    /// L'eco vede il nostro IP reale.
    Transparent,
}

impl ProbeErr {
    fn text(&self) -> String {
        match self {
            ProbeErr::Timeout => "timeout".into(),
            ProbeErr::Transparent => "trasparente: espone il tuo IP".into(),
            ProbeErr::Unreachable(s) | ProbeErr::Network(s) | ProbeErr::Bad(s) => s.clone(),
        }
    }

    /// Il proxy ha risposto ma rifiuta l'http in chiaro: va riprovato in https, perché
    /// molti proxy accettano solo CONNECT o solo la porta 443 (come l'estensione).
    fn proxy_answered(&self) -> bool {
        matches!(self, ProbeErr::Network(_) | ProbeErr::Bad(_))
    }
}

struct Probe {
    ip: String,
    country: String,
    ms: u32,
}

fn client_for(key: &str) -> Result<reqwest::Client, String> {
    let proxy = reqwest::Proxy::all(key).map_err(|e| e.to_string())?;
    reqwest::Client::builder()
        .proxy(proxy)
        .redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(0)
        .user_agent(UA)
        .build()
        .map_err(|e| e.to_string())
}

/// true se l'errore dice che il proxy stesso non si raggiunge. Per i socks hyper-util non
/// espone l'`io::Error` nella catena: resta solo il suo messaggio per la connessione al proxy.
fn proxy_unreachable(e: &reqwest::Error) -> bool {
    use std::io::ErrorKind::*;
    let mut src: Option<&dyn std::error::Error> = Some(e);
    while let Some(x) = src {
        if let Some(io) = x.downcast_ref::<std::io::Error>() {
            if matches!(io.kind(), ConnectionRefused | HostUnreachable | NetworkUnreachable | AddrNotAvailable) {
                return true;
            }
        }
        if x.to_string().contains("failed to create underlying connection") {
            return true;
        }
        src = x.source();
    }
    false
}

fn err_text(e: &reqwest::Error) -> String {
    use std::error::Error;
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(inner) = src {
        s = inner.to_string();
        src = inner.source();
    }
    s
}

async fn probe(client: &reqwest::Client, url: &str, timeout: Duration, real_ip: &str) -> Result<Probe, ProbeErr> {
    let t0 = Instant::now();
    let res = client.get(url).timeout(timeout).send().await.map_err(|e| {
        if e.is_timeout() {
            ProbeErr::Timeout
        } else if proxy_unreachable(&e) {
            ProbeErr::Unreachable(err_text(&e))
        } else {
            ProbeErr::Network(err_text(&e))
        }
    })?;
    if res.status().as_u16() != 200 {
        return Err(ProbeErr::Bad(format!("HTTP {}", res.status().as_u16())));
    }
    let body = tokio::time::timeout(timeout.saturating_sub(t0.elapsed()).max(Duration::from_millis(500)), res.text())
        .await
        .map_err(|_| ProbeErr::Timeout)?
        .map_err(|e| ProbeErr::Network(err_text(&e)))?;
    let ms = t0.elapsed().as_millis().min(u32::MAX as u128) as u32;
    let (ip, country) = parse_echo(&body).ok_or_else(|| ProbeErr::Bad("risposta senza IP".into()))?;
    if ip == real_ip {
        return Err(ProbeErr::Transparent);
    }
    Ok(Probe { ip, country, ms })
}

fn pick(list: &[String], i: usize) -> Option<&String> {
    if list.is_empty() {
        None
    } else {
        list.get(i % list.len())
    }
}

/// Passaggio http con un secondo tentativo su un'altra eco dopo un errore di rete
/// (o anche dopo un timeout se `retry_after_timeout`).
async fn http_stage(client: &reqwest::Client, cfg: &TestConfig, idx: usize, real_ip: &str) -> Result<Probe, ProbeErr> {
    let first = pick(&cfg.http_echo, idx).ok_or_else(|| ProbeErr::Bad("nessun sito di eco http".into()))?;
    match probe(client, &format!("http://{first}"), cfg.http_timeout, real_ip).await {
        Ok(p) => Ok(p),
        Err(e) => {
            let retry = match e {
                ProbeErr::Network(_) => true,
                ProbeErr::Timeout => cfg.retry_after_timeout,
                ProbeErr::Unreachable(_) | ProbeErr::Bad(_) | ProbeErr::Transparent => false,
            };
            match pick(&cfg.http_echo, idx + 1).filter(|_| retry) {
                Some(second) => probe(client, &format!("http://{second}"), cfg.http_timeout, real_ip).await,
                None => Err(e),
            }
        }
    }
}

/// Una prova https (tunnel CONNECT o SOCKS verso la 443, TLS vera) sull'eco `idx`.
async fn https_probe(client: &reqwest::Client, cfg: &TestConfig, idx: usize, real_ip: &str) -> Result<Probe, ProbeErr> {
    let h = pick(&cfg.https_echo, idx).ok_or_else(|| ProbeErr::Bad("nessun sito di eco https".into()))?;
    probe(client, &format!("https://{h}"), cfg.https_timeout, real_ip).await
}

async fn run_test(c: &Candidate, real_ip: &str, cfg: &TestConfig, idx: usize) -> Outcome {
    let dead = |reason: String| Outcome::Dead { reason };
    let client = match client_for(&c.key()) {
        Ok(cl) => cl,
        Err(e) => return dead(e),
    };
    // passaggio A in http; se il proxy ha risposto ma rifiuta l'http in chiaro, di nuovo in https
    let (a, http_ok) = match http_stage(&client, cfg, idx, real_ip).await {
        Ok(p) => (p, true),
        Err(e) if e.proxy_answered() => match https_probe(&client, cfg, idx, real_ip).await {
            Ok(p) => (p, false),
            Err(e2) => return dead(format!("http: {}; https: {}", e.text(), e2.text())),
        },
        Err(e) => return dead(format!("http: {}", e.text())),
    };
    // passaggio B in https su un'altra eco
    let b = https_probe(&client, cfg, idx + 1, real_ip).await;
    let (https, latency, exit_ip, echo_cc) = match b {
        Ok(b) => {
            let cc = if b.country.is_empty() { a.country.clone() } else { b.country };
            (true, (a.ms + b.ms) / 2, b.ip, cc)
        }
        // regge solo l'https ma la seconda prova fallisce: instabile, non "solo http"
        Err(e) if !http_ok => return dead(format!("https instabile: {}", e.text())),
        Err(e) => {
            if cfg.drop_http_only {
                return dead(format!("solo http (https: {})", e.text()));
            }
            // conferma: una seconda prova http su un'altra eco
            match http_stage(&client, cfg, idx + 2, real_ip).await {
                Ok(_) => (false, a.ms, a.ip, a.country),
                Err(e2) => return dead(format!("http instabile: {}", e2.text())),
            }
        }
    };
    let now = now_secs();
    Outcome::Good(GoodProxy {
        protocol: c.protocol,
        ip: c.ip,
        port: c.port,
        latency_ms: latency,
        exit_ip,
        country: if echo_cc.is_empty() { c.country.clone() } else { echo_cc },
        https,
        score: proxy_score(latency, https),
        sources: c.sources.clone(),
        added_at: now,
        checked_at: now,
        alive: true,
        speed_bps: None,
    })
}

/// File scaricato da `speed_test` (Cloudflare, 2 MB).
pub const SPEED_URL: &str = "https://speed.cloudflare.com/__down?bytes=2000000";
/// Tetto iniziale di uno speedtest (poi scende al tempo del più veloce).
pub const SPEED_LIMIT: Duration = Duration::from_secs(15);
/// Speedtest contemporanei.
pub const SPEED_PARALLEL: usize = 3;

/// Esito di `speed_test`: byte arrivati, secondi impiegati e se il file è arrivato tutto.
#[derive(Debug, Clone, Copy)]
pub struct Speed {
    pub bytes: u64,
    pub secs: f64,
    pub complete: bool,
}

impl Speed {
    pub fn bytes_per_sec(&self) -> f64 {
        self.bytes as f64 / self.secs.max(1e-3)
    }
}

/// Scarica `SPEED_URL` attraverso `p`. Il tempo parte dalla richiesta (connessione e TLS
/// comprese). `limit_ms` è il tetto condiviso fra le prove in parallelo: chi finisce lo abbassa al
/// proprio tempo, e chi lo supera si ferma subito (sarebbe comunque più lento). Una prova fermata
/// dal tetto restituisce quanto è arrivato (`complete: false`); errore se non arriva niente.
pub async fn speed_test(p: &GoodProxy, limit_ms: &AtomicU64) -> Result<Speed, String> {
    let client = client_for(&p.key())?;
    let t0 = Instant::now();
    let deadline = || t0 + Duration::from_millis(limit_ms.load(Ordering::Relaxed));
    // il tetto può scendere mentre si aspetta: si ricontrolla almeno ogni 200 ms
    let tick = || tokio::time::sleep_until(deadline().min(Instant::now() + Duration::from_millis(200)).into());
    let speed = |bytes, complete| Speed { bytes, secs: t0.elapsed().as_secs_f64(), complete };
    let slower = || Err("più lento del più veloce".to_string());

    let send = client.get(SPEED_URL).send();
    tokio::pin!(send);
    let mut r = loop {
        if Instant::now() >= deadline() {
            return slower();
        }
        tokio::select! {
            r = &mut send => break r.map_err(|e| err_text(&e))?,
            _ = tick() => {}
        }
    };
    if !r.status().is_success() {
        return Err(format!("HTTP {}", r.status().as_u16()));
    }
    let mut got: u64 = 0;
    loop {
        if Instant::now() >= deadline() {
            return if got == 0 { slower() } else { Ok(speed(got, false)) };
        }
        tokio::select! {
            c = r.chunk() => match c.map_err(|e| err_text(&e))? {
                Some(chunk) => got += chunk.len() as u64,
                None => break,
            },
            _ = tick() => {}
        }
    }
    let s = speed(got, true);
    limit_ms.fetch_min((s.secs * 1000.0).ceil() as u64, Ordering::Relaxed);
    Ok(s)
}

/// Speedtest di `list`, `SPEED_PARALLEL` alla volta, con un tetto condiviso che parte da
/// `SPEED_LIMIT` e scende al tempo del più veloce arrivato in fondo (vedi `speed_test`).
/// `on_result` riceve ogni esito appena arriva. Si ferma con `cancel`.
pub async fn speed_pool(list: Vec<GoodProxy>, cancel: &CancellationToken, mut on_result: impl FnMut(&GoodProxy, Result<Speed, String>)) {
    let limit_ms = std::sync::Arc::new(AtomicU64::new(SPEED_LIMIT.as_millis() as u64));
    let mut tests = futures::stream::iter(list)
        .map(|p| {
            let limit_ms = limit_ms.clone();
            async move {
                let r = speed_test(&p, &limit_ms).await;
                (p, r)
            }
        })
        .buffer_unordered(SPEED_PARALLEL);
    loop {
        tokio::select! {
            r = tests.next() => match r {
                Some((p, r)) => on_result(&p, r),
                None => return,
            },
            _ = cancel.cancelled() => return,
        }
    }
}

/// Il migliore fra gli esiti di uno speedtest: un download completo batte sempre uno fermato al
/// tetto, poi conta la velocità.
pub fn faster(a: &Speed, b: &Speed) -> bool {
    (a.complete, a.bytes_per_sec()) > (b.complete, b.bytes_per_sec())
}

/// Prova un candidato. `echo_idx` sceglie da quale sito di eco partire (rotazione).
pub async fn test_one(c: &Candidate, real_ip: &str, cfg: &TestConfig, cancel: &CancellationToken, echo_idx: usize) -> Outcome {
    if cancel.is_cancelled() {
        return Outcome::Cancelled;
    }
    tokio::select! {
        o = run_test(c, real_ip, cfg, echo_idx) => o,
        _ = cancel.cancelled() => Outcome::Cancelled,
    }
}

/// Pool a N corsie: appena un candidato finisce, la sua corsia prende il successivo.
pub async fn test_pool(
    cands: Vec<Candidate>,
    real_ip: String,
    cfg: TestConfig,
    cancel: CancellationToken,
    mut on_result: impl FnMut(&Candidate, &Outcome) + Send,
) {
    let lanes = cfg.concurrency.clamp(1, crate::state::MAX_PARALLEL);
    let (real_ip, cfg) = (std::sync::Arc::new(real_ip), std::sync::Arc::new(cfg));
    let mut stream = futures::stream::iter(cands.into_iter().enumerate().map(|(i, c)| {
        let (real_ip, cfg, cancel) = (real_ip.clone(), cfg.clone(), cancel.clone());
        async move {
            let o = test_one(&c, &real_ip, &cfg, &cancel, i).await;
            (c, o)
        }
    }))
    .buffer_unordered(lanes);
    while let Some((c, o)) = stream.next().await {
        on_result(&c, &o);
    }
}
