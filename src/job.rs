//! Job asincroni (download, test, ricontrollo, connessione, verifica IP) che girano sul
//! runtime tokio e mandano eventi alla UI. Un job non tocca mai lo stato: la UI applica gli eventi.

use crate::asn::{self, AsnTable};
use crate::model::{Candidate, GoodProxy, Source};
use crate::sources::{exclude_known, fetch_all, http_client, merge};
use crate::tester::{real_ip, speed_pool, test_one, test_pool, Outcome, Speed, TestConfig};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Download,
    Test,
    Recheck,
    Connect,
    VerifyIp,
    Speed,
}

impl JobKind {
    pub fn label(&self) -> &'static str {
        match self {
            JobKind::Download => "Download",
            JobKind::Test => "Test",
            JobKind::Recheck => "Ricontrollo",
            JobKind::Connect => "Connessione",
            JobKind::VerifyIp => "Verifica IP",
            JobKind::Speed => "Speedtest",
        }
    }
}

#[derive(Clone, Debug)]
pub enum JobEvent {
    Started(JobKind),
    Progress { done: usize, total: usize, found: usize, phase: String },
    Log(String),
    SourceDownloaded { id: String, count: usize, error: Option<String> },
    /// Lista unita e già filtrata (senza provati né buoni).
    Downloaded { list: Vec<Candidate>, total: usize },
    /// Tabella IP -> ASN scaricata: la tabella letta e il file compresso da salvare.
    AsnTable { table: Arc<AsnTable>, gz: Arc<Vec<u8>> },
    /// Esito della prova di un candidato.
    Tested { cand: Candidate, outcome: Outcome },
    /// Esito dello speedtest di un proxy buono.
    Speed { key: String, result: Result<Speed, String> },
    /// Proxy scelto dalla connessione, appena riverificato.
    Connected(GoodProxy),
    RealIp(String),
    /// Verifica IP: IP reale e IP visto attraverso il proxy attivo.
    Ips { real: String, exit: Option<String> },
    Error(String),
    Finished { kind: JobKind, summary: String, cancelled: bool },
}

pub type Repaint = Arc<dyn Fn() + Send + Sync>;

pub struct JobHandle {
    pub kind: JobKind,
    pub cancel: CancellationToken,
    pub rx: mpsc::Receiver<JobEvent>,
}

#[derive(Clone)]
struct Tx {
    tx: mpsc::Sender<JobEvent>,
    repaint: Repaint,
}

impl Tx {
    fn send(&self, e: JobEvent) {
        let _ = self.tx.send(e);
        (self.repaint)();
    }
    fn finish(&self, kind: JobKind, summary: impl Into<String>, cancelled: bool) {
        self.send(JobEvent::Finished { kind, summary: summary.into(), cancelled });
    }
}

fn spawn<F, Fut>(rt: &Handle, repaint: Repaint, kind: JobKind, f: F) -> JobHandle
where
    F: FnOnce(Tx, CancellationToken) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    let cancel = CancellationToken::new();
    let tx = Tx { tx, repaint };
    tx.send(JobEvent::Started(kind));
    rt.spawn(f(tx, cancel.clone()));
    JobHandle { kind, cancel, rx }
}

async fn ensure_real_ip(tx: &Tx, known: Option<String>) -> Option<String> {
    if let Some(ip) = known {
        return Some(ip);
    }
    match real_ip(&http_client()).await {
        Ok(ip) => {
            tx.send(JobEvent::RealIp(ip.clone()));
            Some(ip)
        }
        Err(e) => {
            tx.send(JobEvent::Error(e));
            None
        }
    }
}

/// Scarica le sorgenti date, unisce le liste e toglie le chiavi già note.
/// Scarica le sorgenti e, con `fetch_asn`, anche la tabella ASN (un errore lì non ferma il download).
pub fn start_download(rt: &Handle, repaint: Repaint, sources: Vec<Source>, known: HashSet<String>, fetch_asn: bool) -> JobHandle {
    spawn(rt, repaint, JobKind::Download, move |tx, cancel| async move {
        let client = http_client();
        let total = sources.len();
        let mut done = 0;
        let asn_job = async {
            if !fetch_asn {
                return;
            }
            let fetched = tokio::select! {
                r = asn::fetch(&client) => r,
                _ = cancel.cancelled() => return,
            };
            let msg = match fetched {
                Ok((gz, table)) => {
                    let msg = format!("tabella ASN: {} intervalli di IP", table.len());
                    tx.send(JobEvent::AsnTable { table: Arc::new(table), gz: Arc::new(gz) });
                    msg
                }
                Err(e) => format!("tabella ASN: errore {e} (resta quella di prima)"),
            };
            tx.send(JobEvent::Log(msg));
        };
        let lists_job = fetch_all(&client, &sources, &cancel, |r| {
            done += 1;
            let (count, error) = match &r.result {
                Ok(l) => (l.len(), None),
                Err(e) => (0, Some(e.clone())),
            };
            tx.send(JobEvent::SourceDownloaded { id: r.id.clone(), count, error: error.clone() });
            tx.send(JobEvent::Log(match error {
                None => format!("{}: {count} voci", r.id),
                Some(e) => format!("{}: errore {e}", r.id),
            }));
            tx.send(JobEvent::Progress { done, total, found: 0, phase: "download".into() });
        });
        let (results, ()) = tokio::join!(lists_job, asn_job);
        if cancel.is_cancelled() {
            tx.finish(JobKind::Download, "download annullato", true);
            return;
        }
        // le sorgenti con rating più alto vanno per prime nel round-robin
        let lists: Vec<Vec<Candidate>> = sources
            .iter()
            .filter_map(|s| results.iter().find(|r| r.id == s.id))
            .filter_map(|r| r.result.as_ref().ok().cloned())
            .map(|l| exclude_known(l, &known))
            .collect();
        let list = merge(lists);
        let total = list.len();
        let ok = results.iter().filter(|r| r.result.is_ok()).count();
        tx.send(JobEvent::Downloaded { list, total });
        tx.finish(JobKind::Download, format!("{total} proxy da {ok}/{} sorgenti", results.len()), false);
    })
}

fn run_pool(rt: &Handle, repaint: Repaint, kind: JobKind, cands: Vec<Candidate>, cfg: TestConfig, known_ip: Option<String>) -> JobHandle {
    spawn(rt, repaint, kind, move |tx, cancel| async move {
        let Some(ip) = ensure_real_ip(&tx, known_ip).await else {
            tx.finish(kind, "IP reale non disponibile: test non avviato", false);
            return;
        };
        let total = cands.len();
        let done = AtomicUsize::new(0);
        let found = AtomicUsize::new(0);
        let phase = if kind == JobKind::Test { "test" } else { "ricontrollo" };
        tx.send(JobEvent::Progress { done: 0, total, found: 0, phase: phase.into() });
        test_pool(cands, ip, cfg, cancel.clone(), |c, o| {
            if *o == Outcome::Cancelled {
                return;
            }
            let d = done.fetch_add(1, Ordering::Relaxed) + 1;
            let f = found.load(Ordering::Relaxed) + matches!(o, Outcome::Good(_)) as usize;
            found.store(f, Ordering::Relaxed);
            tx.send(JobEvent::Log(log_line(c, o)));
            tx.send(JobEvent::Tested { cand: c.clone(), outcome: o.clone() });
            tx.send(JobEvent::Progress { done: d, total, found: f, phase: phase.into() });
        })
        .await;
        let (d, f) = (done.load(Ordering::Relaxed), found.load(Ordering::Relaxed));
        tx.finish(kind, format!("{d} provati · {f} buoni"), cancel.is_cancelled());
    })
}

pub fn log_line(c: &Candidate, o: &Outcome) -> String {
    let cc = crate::countries::alpha3(&c.country);
    match o {
        Outcome::Good(g) => format!(
            "OK   {cc} {}  {} ms  uscita {}{}",
            c.key(),
            g.latency_ms,
            g.exit_ip,
            if g.https { "" } else { "  [SOLO HTTP]" }
        ),
        Outcome::Dead { reason } => format!("KO   {cc} {}  {reason}", c.key()),
        Outcome::Cancelled => format!("--   {cc} {}  annullato", c.key()),
    }
}

/// Prima scansione dei candidati: gli esiti aggiornano rating e storico (lo fa la UI).
pub fn start_test(rt: &Handle, repaint: Repaint, cands: Vec<Candidate>, cfg: TestConfig, real_ip: Option<String>) -> JobHandle {
    run_pool(rt, repaint, JobKind::Test, cands, cfg, real_ip)
}

/// Rimette alla prova i buoni: non tocca i rating.
pub fn start_recheck(rt: &Handle, repaint: Repaint, proxies: Vec<GoodProxy>, cfg: TestConfig, real_ip: Option<String>) -> JobHandle {
    let cands = proxies.iter().map(GoodProxy::as_candidate).collect();
    run_pool(rt, repaint, JobKind::Recheck, cands, cfg, real_ip)
}

/// Speedtest dei proxy dati (vedi `tester::speed_pool`): misura la velocità di download, non
/// tocca rating né latenza.
pub fn start_speed(rt: &Handle, repaint: Repaint, proxies: Vec<GoodProxy>) -> JobHandle {
    spawn(rt, repaint, JobKind::Speed, move |tx, cancel| async move {
        let total = proxies.len();
        let (mut done, mut best): (usize, Option<(String, Speed)>) = (0, None);
        tx.send(JobEvent::Progress { done, total, found: 0, phase: "speedtest".into() });
        speed_pool(proxies, &cancel, |p, r| {
            done += 1;
            let key = p.key();
            tx.send(JobEvent::Log(match &r {
                Ok(s) => format!(
                    "OK   {key}  {:.2} MB/s in {:.1} s{}",
                    s.bytes_per_sec() / 1e6,
                    s.secs,
                    if s.complete { "" } else { " (fermato al tetto)" }
                ),
                Err(e) => format!("KO   {key}  scartato: {e}"),
            }));
            if let Ok(s) = &r {
                if best.as_ref().is_none_or(|(_, b)| crate::tester::faster(s, b)) {
                    best = Some((key.clone(), *s));
                }
            }
            tx.send(JobEvent::Speed { key, result: r });
            tx.send(JobEvent::Progress { done, total, found: 0, phase: "speedtest".into() });
        })
        .await;
        let summary = match &best {
            Some((k, s)) => format!("{done}/{total} misurati · il più veloce: {k} ({:.2} MB/s)", s.bytes_per_sec() / 1e6),
            None => format!("{done}/{total} misurati · nessun download riuscito"),
        };
        tx.finish(JobKind::Speed, summary, cancel.is_cancelled());
    })
}

/// Riverifica i proxy nell'ordine dato, 8 alla volta, e si ferma al primo che risponde.
pub fn start_connect(rt: &Handle, repaint: Repaint, proxies: Vec<GoodProxy>, cfg: TestConfig, real_ip: Option<String>) -> JobHandle {
    spawn(rt, repaint, JobKind::Connect, move |tx, cancel| async move {
        let Some(ip) = ensure_real_ip(&tx, real_ip).await else {
            tx.finish(JobKind::Connect, "IP reale non disponibile", false);
            return;
        };
        let total = proxies.len();
        let mut done = 0;
        for chunk in proxies.chunks(8) {
            let cands: Vec<Candidate> = chunk.iter().map(GoodProxy::as_candidate).collect();
            let outcomes = futures::future::join_all(
                cands.iter().enumerate().map(|(i, c)| test_one(c, &ip, &cfg, &cancel, done + i)),
            )
            .await;
            if cancel.is_cancelled() {
                tx.finish(JobKind::Connect, "connessione annullata", true);
                return;
            }
            for (c, o) in cands.iter().zip(&outcomes) {
                tx.send(JobEvent::Log(log_line(c, o)));
                tx.send(JobEvent::Tested { cand: c.clone(), outcome: o.clone() });
            }
            done += chunk.len();
            tx.send(JobEvent::Progress { done, total, found: 0, phase: "connessione".into() });
            if let Some(Outcome::Good(g)) = outcomes.into_iter().find(|o| matches!(o, Outcome::Good(_))) {
                let msg = format!("connesso a {}", g.key());
                tx.send(JobEvent::Connected(g));
                tx.finish(JobKind::Connect, msg, false);
                return;
            }
        }
        tx.send(JobEvent::Error("nessun proxy ha risposto".into()));
        tx.finish(JobKind::Connect, "nessun proxy ha risposto", false);
    })
}

/// IP reale (diretto) e IP di uscita attraverso `through` (se c'è).
pub fn start_verify_ip(rt: &Handle, repaint: Repaint, through: Option<GoodProxy>) -> JobHandle {
    spawn(rt, repaint, JobKind::VerifyIp, move |tx, cancel| async move {
        let work = async {
            let real = real_ip(&http_client()).await?;
            tx.send(JobEvent::RealIp(real.clone()));
            let exit = match through {
                Some(p) => {
                    let proxy = reqwest::Proxy::all(p.key()).map_err(|e| e.to_string())?;
                    let client = reqwest::Client::builder()
                        .proxy(proxy)
                        .timeout(std::time::Duration::from_secs(15))
                        .build()
                        .map_err(|e| e.to_string())?;
                    Some(real_ip(&client).await.unwrap_or_else(|e| format!("errore: {e}")))
                }
                None => None,
            };
            Ok::<_, String>((real, exit))
        };
        tokio::select! {
            r = work => match r {
                Ok((real, exit)) => {
                    let s = match &exit {
                        Some(e) => format!("IP reale {real} · uscita {e}"),
                        None => format!("IP reale {real} · nessun proxy attivo"),
                    };
                    tx.send(JobEvent::Ips { real, exit });
                    tx.finish(JobKind::VerifyIp, s, false);
                }
                Err(e) => {
                    tx.send(JobEvent::Error(e.clone()));
                    tx.finish(JobKind::VerifyIp, e, false);
                }
            },
            _ = cancel.cancelled() => tx.finish(JobKind::VerifyIp, "annullato", true),
        }
    })
}
