//! Sorgenti predefinite, download e unione delle liste.

use crate::model::{now_secs, Candidate, Endpoint, Format, Protocol, Source, SourceStats};
use crate::parse::parse;
use futures::StreamExt;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const FETCH_TIMEOUT: Duration = Duration::from_secs(25);
pub const MAX_BODY: usize = 32 * 1024 * 1024;

const R: &str = "https://raw.githubusercontent.com";

fn ep(url: String, format: Format, protocol: Option<Protocol>) -> Endpoint {
    Endpoint { url, format, protocol, https: false }
}

/// Un endpoint `ipPort` per ogni coppia (nome file, protocollo). Il file "https" dichiara
/// proxy http che reggono https.
fn per_proto(fmt: Format, url: impl Fn(&str) -> String, files: &[(&str, Protocol)]) -> Vec<Endpoint> {
    files
        .iter()
        .map(|(f, p)| Endpoint { https: Protocol::declares_https(f), ..ep(url(f), fmt, Some(*p)) })
        .collect()
}

fn src(id: &str, name: &str, repo: &str, prior: f64, enabled: bool, endpoints: Vec<Endpoint>) -> Source {
    Source {
        id: id.into(),
        name: name.into(),
        repo: repo.into(),
        endpoints,
        enabled,
        builtin: true,
        prior,
        stats: SourceStats::default(),
    }
}

const Q3: f64 = 0.5;
const Q2: f64 = 0.35;
pub const CUSTOM_PRIOR: f64 = 0.3;

const HTTP_S4_S5: [(&str, Protocol); 3] = [("http", Protocol::Http), ("socks4", Protocol::Socks4), ("socks5", Protocol::Socks5)];

pub fn default_sources() -> Vec<Source> {
    use Format::*;
    use Protocol::*;
    let gh = |p: &str| format!("https://github.com/{p}");
    vec![
        // --- originali dell'estensione ---
        src("monosans", "monosans / proxy-list", &gh("monosans/proxy-list"), Q3, true,
            vec![ep(format!("{R}/monosans/proxy-list/main/proxies.json"), Monosans, None)]),
        src("proxifly", "proxifly / free-proxy-list", &gh("proxifly/free-proxy-list"), Q3, true,
            // l'elenco completo (all/data.json) supera i 20 MB che jsDelivr serve: solo i socks5
            vec![ep("https://cdn.jsdelivr.net/gh/proxifly/free-proxy-list@main/proxies/protocols/socks5/data.txt".into(), ProtoUrl, None)]),
        src("vakhov", "vakhov / fresh-proxy-list", &gh("vakhov/fresh-proxy-list"), Q3, true,
            vec![ep(format!("{R}/vakhov/fresh-proxy-list/master/proxylist.json"), Vakhov, None)]),
        src("hideip", "zloi-user / hideip.me", &gh("zloi-user/hideip.me"), Q2, true,
            per_proto(IpPortCountry, |f| format!("{R}/zloi-user/hideip.me/main/{f}.txt"),
                &[("http", Http), ("https", Http), ("socks4", Socks4), ("socks5", Socks5)])),
        src("speedx", "TheSpeedX / PROXY-List", &gh("TheSpeedX/PROXY-List"), Q2, true,
            per_proto(IpPort, |f| format!("{R}/TheSpeedX/PROXY-List/master/{f}.txt"), &HTTP_S4_S5)),
        src("hookzof", "hookzof / socks5_list", &gh("hookzof/socks5_list"), Q2, true,
            vec![ep(format!("{R}/hookzof/socks5_list/master/proxy.txt"), IpPort, Some(Socks5))]),
        src("roosterkid", "roosterkid / openproxylist", &gh("roosterkid/openproxylist"), Q2, true,
            per_proto(IpPort, |f| format!("{R}/roosterkid/openproxylist/main/{f}_RAW.txt"),
                &[("HTTPS", Http), ("SOCKS4", Socks4), ("SOCKS5", Socks5)])),
        src("sunny9577", "sunny9577 / proxy-scraper", &gh("sunny9577/proxy-scraper"), Q2, true,
            per_proto(IpPort, |f| format!("{R}/sunny9577/proxy-scraper/master/generated/{f}_proxies.txt"), &HTTP_S4_S5)),
        src("zaeem", "Zaeem20 / FREE_PROXIES_LIST", &gh("Zaeem20/FREE_PROXIES_LIST"), Q2, true,
            per_proto(IpPort, |f| format!("{R}/Zaeem20/FREE_PROXIES_LIST/master/{f}.txt"), &HTTP_S4_S5)),
        src("proxyscrape", "ProxyScrape API v4", "https://proxyscrape.com/free-proxy-list", Q3, true,
            vec![ep("https://api.proxyscrape.com/v4/free-proxy-list/get?request=display_proxies&proxy_format=protocolipport&format=text&timeout=2000".into(), ProtoUrl, None)]),
        // --- alternative GitHub ---
        src("jetkai", "jetkai / proxy-list", &gh("jetkai/proxy-list"), Q2, true,
            per_proto(IpPort, |f| format!("{R}/jetkai/proxy-list/main/online-proxies/txt/proxies-{f}.txt"), &HTTP_S4_S5)),
        src("rdavydov", "rdavydov / proxy-list", &gh("rdavydov/proxy-list"), Q2, true,
            per_proto(IpPort, |f| format!("{R}/rdavydov/proxy-list/main/proxies/{f}.txt"), &HTTP_S4_S5)),
        src("iplocate", "iplocate / free-proxy-list", &gh("iplocate/free-proxy-list"), Q2, true,
            vec![ep(format!("{R}/iplocate/free-proxy-list/main/all-proxies.txt"), ProtoUrl, None)]),
        src("dpangestuw", "dpangestuw / Free-Proxy", &gh("dpangestuw/Free-Proxy"), Q2, true,
            per_proto(ProtoUrl, |f| format!("{R}/dpangestuw/Free-Proxy/main/{f}_proxies.txt"), &HTTP_S4_S5)),
        src("r00tee", "r00tee / Proxy-List", &gh("r00tee/Proxy-List"), Q2, true,
            per_proto(IpPort, |f| format!("{R}/r00tee/Proxy-List/main/{f}.txt"),
                &[("Https", Http), ("Socks4", Socks4), ("Socks5", Socks5)])),
        src("databay", "databay-labs / free-proxy-list", &gh("databay-labs/free-proxy-list"), Q2, true,
            per_proto(IpPort, |f| format!("{R}/databay-labs/free-proxy-list/master/{f}.txt"),
                &[("http", Http), ("socks5", Socks5)])),
        src("vann-dev", "Vann-Dev / proxy-list", &gh("Vann-Dev/proxy-list"), Q2, true,
            per_proto(IpPort, |f| format!("{R}/Vann-Dev/proxy-list/main/proxies/{f}.txt"), &HTTP_S4_S5)),
        src("elliottophellia", "elliottophellia / proxylist", &gh("elliottophellia/proxylist"), Q2, true,
            per_proto(IpPort, |f| format!("{R}/elliottophellia/proxylist/master/results/{f}/global/{f}_checked.txt"), &HTTP_S4_S5)),
        src("clarketm", "clarketm / proxy-list", &gh("clarketm/proxy-list"), Q2, true,
            vec![ep(format!("{R}/clarketm/proxy-list/master/proxy-list-raw.txt"), IpPort, Some(Http))]),
        src("prxchk", "prxchk / proxy-list", &gh("prxchk/proxy-list"), Q2, true,
            vec![ep(format!("{R}/prxchk/proxy-list/main/http.txt"), IpPort, Some(Http))]),
        src("anonym0us", "Anonym0usWork1221 / Free-Proxies", &gh("Anonym0usWork1221/Free-Proxies"), Q2, true,
            per_proto(IpPort, |f| format!("{R}/Anonym0usWork1221/Free-Proxies/main/proxy_files/{f}_proxies.txt"), &HTTP_S4_S5)),
        src("tuanminpay", "TuanMinPay / live-proxy", &gh("TuanMinPay/live-proxy"), Q2, true,
            per_proto(IpPort, |f| format!("{R}/TuanMinPay/live-proxy/master/{f}.txt"), &HTTP_S4_S5)),
        src("ercindedeoglu", "ErcinDedeoglu / proxies", &gh("ErcinDedeoglu/proxies"), Q2, true,
            per_proto(IpPort, |f| format!("{R}/ErcinDedeoglu/proxies/main/proxies/{f}.txt"),
                &[("http", Http), ("https", Http), ("socks4", Socks4), ("socks5", Socks5)])),
    ]
}

/// Slug unico per una sorgente aggiunta a mano.
pub fn slug(name: &str, existing: &[Source]) -> String {
    let mut base: String = name
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while base.contains("--") {
        base = base.replace("--", "-");
    }
    let base = base.trim_matches('-').to_string();
    let base = if base.is_empty() { "sorgente".to_string() } else { base };
    let mut id = base.clone();
    let mut n = 2;
    while existing.iter().any(|s| s.id == id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// Nuova sorgente personalizzata.
pub fn custom_source(name: &str, url: &str, format: Format, protocol: Option<Protocol>, existing: &[Source]) -> Source {
    Source {
        id: slug(name, existing),
        name: name.trim().to_string(),
        repo: url.trim().to_string(),
        endpoints: vec![Endpoint { url: url.trim().to_string(), format, protocol, https: false }],
        enabled: true,
        builtin: false,
        prior: CUSTOM_PRIOR,
        stats: SourceStats::default(),
    }
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("proxy-finder/", env!("CARGO_PKG_VERSION")))
        .timeout(FETCH_TIMEOUT)
        .no_proxy()
        .build()
        .expect("client http")
}

async fn fetch_text(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let body = fetch_bytes(client, url).await?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Scarica `url` (al massimo 32 MB).
pub async fn fetch_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let mut res = client.get(url).send().await.map_err(|e| short_err(&e))?;
    if !res.status().is_success() {
        return Err(format!("HTTP {}", res.status().as_u16()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(|e| short_err(&e))? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BODY {
            return Err("risposta oltre 32 MB".into());
        }
    }
    Ok(body)
}

fn short_err(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timeout".into()
    } else if e.is_connect() {
        "connessione fallita".into()
    } else {
        e.to_string()
    }
}

/// Scarica e normalizza tutti gli endpoint di una sorgente. Errori parziali tollerati:
/// si fallisce solo se non arriva nessuna voce e almeno un endpoint ha dato errore.
pub async fn fetch_source(client: &reqwest::Client, s: &Source, cancel: &CancellationToken) -> Result<Vec<Candidate>, String> {
    let futs = s.endpoints.iter().map(|ep| async move {
        let text = fetch_text(client, &ep.url).await?;
        Ok::<_, String>(parse(&text, ep))
    });
    let results = tokio::select! {
        r = futures::future::join_all(futs) => r,
        _ = cancel.cancelled() => return Err("annullato".into()),
    };
    let mut errors = Vec::new();
    let mut lists = Vec::new();
    for (ep, r) in s.endpoints.iter().zip(results) {
        match r {
            Ok(l) => lists.push(l),
            Err(e) => errors.push(format!("{}: {e}", ep.url.rsplit('/').next().unwrap_or(&ep.url))),
        }
    }
    let mut seen: HashMap<String, Candidate> = HashMap::new();
    let mut order = Vec::new();
    for mut c in lists.into_iter().flatten() {
        c.sources = vec![s.id.clone()];
        let k = c.key();
        match seen.get_mut(&k) {
            Some(prev) => {
                if c.richness() > prev.richness() {
                    *prev = c;
                }
            }
            None => {
                order.push(k.clone());
                seen.insert(k, c);
            }
        }
    }
    if seen.is_empty() && !errors.is_empty() {
        return Err(errors.join(" | "));
    }
    let mut out: Vec<Candidate> = order.into_iter().filter_map(|k| seen.remove(&k)).collect();
    // prima chi ha latenza dichiarata, poi uptime
    out.sort_by(|a, b| match (a.latency_ms, b.latency_ms) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        _ => b.uptime.unwrap_or(0).cmp(&a.uptime.unwrap_or(0)),
    });
    Ok(out)
}

/// Esito del download di una sorgente.
pub struct SourceResult {
    pub id: String,
    pub result: Result<Vec<Candidate>, String>,
}

#[allow(clippy::redundant_iter_cloned)]
/// Scarica più sorgenti, 6 alla volta, chiamando `on_done` per ognuna appena finisce.
pub async fn fetch_all(
    client: &reqwest::Client,
    sources: &[Source],
    cancel: &CancellationToken,
    mut on_done: impl FnMut(&SourceResult),
) -> Vec<SourceResult> {
    // futuri che possiedono i dati: con i prestiti il job non è `Send` per tokio::spawn
    let mut stream = futures::stream::iter(sources.iter().cloned().map(|s| {
        let (client, cancel) = (client.clone(), cancel.clone());
        async move { SourceResult { id: s.id.clone(), result: fetch_source(&client, &s, &cancel).await } }
    }))
    .buffer_unordered(6);
    let mut out = Vec::new();
    while let Some(r) = stream.next().await {
        on_done(&r);
        out.push(r);
    }
    out
}

/// Aggiorna le statistiche di download delle sorgenti.
pub fn record_download(sources: &mut [Source], r: &SourceResult) {
    if let Some(s) = sources.iter_mut().find(|s| s.id == r.id) {
        s.stats.last_download = Some(now_secs());
        match &r.result {
            Ok(l) => {
                s.stats.last_count = l.len() as u64;
                s.stats.last_error = None;
            }
            Err(e) => s.stats.last_error = Some(e.clone()),
        }
    }
}

fn merge_into(prev: &mut Candidate, c: Candidate) {
    for s in &c.sources {
        if !prev.sources.contains(s) {
            prev.sources.push(s.clone());
        }
    }
    prev.https |= c.https;
    if prev.country.is_empty() && !c.country.is_empty() {
        prev.country = c.country;
    }
    if prev.latency_ms.is_none() {
        prev.latency_ms = c.latency_ms;
    }
    if prev.uptime.is_none() {
        prev.uptime = c.uptime;
    }
}

/// Unisce più liste: dedup per chiave (unendo `sources` e metadati) e interleave
/// round-robin, una voce per lista a turno, senza tetto.
pub fn merge(lists: Vec<Vec<Candidate>>) -> Vec<Candidate> {
    let mut queues: Vec<VecDeque<Candidate>> = lists.into_iter().map(VecDeque::from).collect();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Candidate> = Vec::new();
    loop {
        let mut any = false;
        for q in queues.iter_mut() {
            // salta i duplicati finché la lista non offre una voce nuova
            while let Some(c) = q.pop_front() {
                any = true;
                let k = c.key();
                if let Some(&i) = index.get(&k) {
                    merge_into(&mut out[i], c);
                    continue;
                }
                index.insert(k, out.len());
                out.push(c);
                break;
            }
        }
        if !any {
            break;
        }
    }
    out
}

/// Toglie i candidati già provati o già tra i buoni.
pub fn exclude_known(list: Vec<Candidate>, known: &HashSet<String>) -> Vec<Candidate> {
    list.into_iter().filter(|c| !known.contains(&c.key())).collect()
}
