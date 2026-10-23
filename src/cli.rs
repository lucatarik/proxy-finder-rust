//! Modalità a riga di comando: stessi dati e stesse regole della GUI (`proxy_finder::state`).

use proxy_finder::countries;
use proxy_finder::job::{self, JobEvent, JobHandle, JobKind, Repaint};
use proxy_finder::model::{Endpoint, Format, GoodProxy, GroupStats, Protocol};
use proxy_finder::rating::{self, source_rating};
use proxy_finder::relay;
use proxy_finder::sources::custom_source;
use proxy_finder::state::{round8, Part, State, TestCount, MAX_PARALLEL};
use proxy_finder::store::{restore_default_sources, Order, SortBy, Store};
use proxy_finder::tester::{Outcome, TestConfig};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Modalità auto: proxy provati per lotto (tutti in parallelo) e buoni con https da trovare.
const AUTO_BATCH: usize = 128;
const AUTO_WANT: usize = 10;
/// `nets` mostra solo le reti con almeno tante prove.
const NETS_MIN_SHOW: u64 = 20;
/// Pausa casuale (secondi) tra la scansione e la prova di velocità che sceglie il proxy.
const AUTO_JITTER_SECS: std::ops::RangeInclusive<u64> = 3..=8;

pub const HELP: &str = "\
proxy-finder — trova, prova e usa proxy pubblici

USO
  proxy-finder                       apre la GUI
  proxy-finder <comando> [opzioni]   modalità a riga di comando

COMANDI
  status                             riepilogo: lista scaricata, buoni, storico, sorgenti
  sources [list]                     sorgenti con rating
  sources enable|disable <id>...|--all
  sources add <nome> <url> [--format F] [--protocol P]
  sources remove <id>...
  sources reset [<id>...]            azzera il rating (tutte se nessun id)
  sources restore                    rimette le predefinite (tiene rating e personali)
  download                           scarica le sorgenti abilitate
  test [opzioni]                     prova i proxy scaricati
  list [--all] [--json] [--sort latency|added]
  recheck                            riprova tutti i buoni
  export [FILE] [--json]             esporta i buoni funzionanti (stdout se manca FILE)
  remove <proto://ip:porta>...       rimuove dei buoni
  prune                              rimuove i buoni che non rispondono
  nets [asn|24] [-n N]               rating per rete: le N reti (ASN o /24) migliori e peggiori
  clear list|history|nets            svuota lista scaricata, storico dei test o rating per rete
  ip [proto://ip:porta]              IP reale (e IP di uscita attraverso un proxy)
  relay [--port N] [proto://ip:porta]
                                     avvia il relay locale e lo collega (default: il più veloce)
  auto [opzioni] (anche --auto)      prova 128 proxy nuovi in parallelo (prima socks5 e quelli
                                     dichiarati https), al 10° con https attende qualche
                                     secondo, li riprova tutti, sceglie il più veloce e
                                     avvia il relay; se un relay da riga di comando è già attivo
                                     su quella porta, gli cambia il proxy. Stampa solo l'IP di
                                     uscita (tutto il resto con -v)
  tray [--port N]                    (solo Linux) icona nel vassoio di sistema: Collega (come --auto, e imposta
                                     il proxy di sistema GNOME), Cambia proxy, Scollega
  gui                                apre la GUI
  help                               questo testo

OPZIONI DI test (valgono per questa esecuzione; --save le rende predefinite)
  -n, --count N          quanti provare (multiplo di 8, almeno 8, nessun massimo)
  -p, --protocol P       http | socks4 | socks5 | all
  -c, --country CC       es. ITA oppure ITA,USA (anche IT,US); all = tutti
  -j, --parallel N       prove in parallelo (multiplo di 8, 8..512)
  -t, --timeout T        rapidi (5/8 s) | normali (10/15 s) | pazienti (15/25 s)
      --order O          priority | random
      --retry-timeout / --no-retry-timeout
      --drop-http-only / --keep-http-only
      --save             salva queste opzioni come predefinite

OPZIONI DI auto
  --port N               porta del relay (default quella delle impostazioni)
  -v, --verbose          stampa avanzamento, esiti e dettagli del relay
  -p, -c, -t, --order    come per test; quanti, parallelo e \"scarta solo http\" sono fissi

OPZIONI GENERALI
  -q, --quiet            mostra solo i proxy buoni e il riepilogo
  Ctrl+C                 annulla il job: le prove interrotte non contano

Formati per sources add: auto, ipPort, ipPortCountry, protoUrl, monosans, proxifly, vakhov.
Dati in ~/.config/proxy-finder (o $PROXY_FINDER_HOME), condivisi con la GUI.";

/// `println!` che non va in panico se stdout è chiuso (es. `proxy-finder list | head`): esce in silenzio.
macro_rules! out {
    ($($t:tt)*) => {{
        use std::io::Write;
        if writeln!(std::io::stdout(), $($t)*).is_err() {
            std::process::exit(0);
        }
    }};
}

/// Esito di un job eseguito da `drive`.
struct Driven {
    /// Annullato dall'utente (Ctrl+C), non dalla fermata anticipata.
    cancelled: bool,
    count: TestCount,
    connected: Option<GoodProxy>,
    /// Buoni con https trovati (job Test e Recheck).
    found: Vec<GoodProxy>,
    /// Errori segnalati dal job (in modalità `silent` non vengono stampati).
    errors: Vec<String>,
}

struct Ctx {
    rt: tokio::runtime::Runtime,
    st: State,
    quiet: bool,
    /// `-v`: con `auto` stampa tutto; senza, `auto` è `silent`.
    verbose: bool,
    /// Niente avanzamento né riepiloghi: solo gli errori veri (stderr) e ciò che il comando
    /// stampa di proposito (per `auto`, l'IP di uscita).
    silent: bool,
}

type Res = Result<i32, String>;

fn usage(msg: impl Into<String>) -> String {
    format!("{}\nVedi `proxy-finder help`.", msg.into())
}

/// Punto d'ingresso: restituisce il codice di uscita.
pub fn run(args: Vec<String>) -> i32 {
    let quiet = args.iter().any(|a| a == "-q" || a == "--quiet");
    let verbose = args.iter().any(|a| a == "-v" || a == "--verbose");
    let args: Vec<String> = args.into_iter().filter(|a| !matches!(a.as_str(), "-q" | "--quiet" | "-v" | "--verbose")).collect();
    let cmd = match args.first().map(String::as_str).unwrap_or("help") {
        "--auto" => "auto",
        c => c,
    };
    if matches!(cmd, "help" | "-h" | "--help") {
        out!("{HELP}");
        return 0;
    }
    let store = match Store::open() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("errore: cartella dati non disponibile: {e}");
            return 1;
        }
    };
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("runtime tokio");
    let mut cx = Ctx { rt, st: State::load(Some(store)), quiet, verbose, silent: false };
    let rest = &args[1..];
    let r = match cmd {
        "status" => cx.status(),
        "sources" => cx.sources(rest),
        "download" => cx.download(),
        "test" => cx.test(rest),
        "list" => cx.list(rest),
        "recheck" => cx.recheck(),
        "export" => cx.export(rest),
        "remove" => cx.remove(rest),
        "prune" => cx.prune(),
        "clear" => cx.clear(rest),
        "nets" => cx.nets(rest),
        "ip" => cx.ip(rest),
        "relay" => cx.relay(rest),
        "auto" => cx.auto(rest),
        "tray" => cx.tray(rest),
        other => Err(usage(format!("comando sconosciuto: {other}"))),
    };
    match r {
        Ok(code) => code,
        Err(e) => {
            eprintln!("errore: {e}");
            if e.contains("proxy-finder help") {
                2
            } else {
                1
            }
        }
    }
}

/// Valore dell'opzione `flag` (o `alt`) in `args`, se presente.
fn opt<'a>(args: &'a [String], flag: &str, alt: &str) -> Result<Option<&'a str>, String> {
    match args.iter().position(|a| a == flag || (!alt.is_empty() && a == alt)) {
        Some(i) => args.get(i + 1).map(|v| Some(v.as_str())).ok_or_else(|| usage(format!("{flag} vuole un valore"))),
        None => Ok(None),
    }
}

fn has(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

/// Argomenti posizionali: tutto ciò che non è un'opzione né il valore di un'opzione con valore.
fn positional(args: &[String], with_value: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
        } else if with_value.contains(&a.as_str()) {
            skip = true;
        } else if !a.starts_with('-') {
            out.push(a.clone());
        }
    }
    out
}

fn parse_proxy(s: &str) -> Result<GoodProxy, String> {
    let ep = Endpoint { url: String::new(), format: Format::ProtoUrl, protocol: None, https: false };
    let c = proxy_finder::parse::parse(s, &ep)
        .pop()
        .ok_or_else(|| usage(format!("proxy non valido: {s} (atteso proto://ip:porta)")))?;
    Ok(GoodProxy {
        protocol: c.protocol,
        ip: c.ip,
        port: c.port,
        latency_ms: 0,
        exit_ip: String::new(),
        country: String::new(),
        https: true,
        score: 0.0,
        sources: Vec::new(),
        added_at: proxy_finder::model::now_secs(),
        checked_at: 0,
        alive: true,
    })
}

fn timeouts_normal(cfg: &mut TestConfig) {
    cfg.http_timeout = Duration::from_secs(10);
    cfg.https_timeout = Duration::from_secs(15);
    cfg.retry_after_timeout = true;
}

/// Bandiera e alpha-3, es. "🇮🇹 ITA" (larghezza visibile fissa: 6 colonne).
/// "N min fa" per un istante di sistema.
fn ago_sys(t: std::time::SystemTime) -> String {
    let d = t.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    match d {
        0..=59 => format!("{d} s fa"),
        60..=3599 => format!("{} min fa", d / 60),
        3600..=86_399 => format!("{} h fa", d / 3600),
        _ => format!("{} g fa", d / 86_400),
    }
}

fn cc(c: &str) -> String {
    let flag = countries::flag_emoji(c);
    format!("{} {:<3}", if flag.is_empty() { "  " } else { &flag }, countries::alpha3(c))
}

impl Ctx {
    fn save(&self, parts: &[Part]) -> Result<(), String> {
        let errs = self.st.save(parts);
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("; "))
        }
    }

    /// Esegue un job fino alla fine stampando gli eventi; Ctrl+C lo annulla.
    fn drive(&mut self, h: JobHandle) -> Driven {
        self.drive_until(h, None)
    }

    /// Come `drive`, ma con `stop_at` ferma il job appena ha trovato tanti buoni con https:
    /// le prove ancora in corso finiscono annullate (non contano e restano in lista).
    fn drive_until(&mut self, h: JobHandle, stop_at: Option<usize>) -> Driven {
        let cancel = h.cancel.clone();
        let watcher = self.rt.spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                eprintln!("\nannullamento…");
                cancel.cancel();
            }
        });
        let mut count = TestCount::default();
        let mut cancelled = false;
        let mut stopped = false;
        let mut connected = None;
        let mut found: Vec<GoodProxy> = Vec::new();
        let mut errors = Vec::new();
        let mut last_progress = std::time::Instant::now();
        for e in h.rx.iter() {
            match e {
                JobEvent::Log(l) => {
                    if !self.silent && (!self.quiet || l.starts_with("OK")) {
                        out!("{l}");
                    }
                }
                JobEvent::Progress { done, total, found, phase } => {
                    if !self.silent && self.quiet && total > 0 && (last_progress.elapsed() > Duration::from_secs(5) || done == total) {
                        eprintln!("[{phase}] {done}/{total} · trovati {found}");
                        last_progress = std::time::Instant::now();
                    }
                }
                JobEvent::SourceDownloaded { id, count, error } => self.st.record_download(&id, count, error),
                JobEvent::Downloaded { list, .. } => self.st.candidates = list,
                JobEvent::AsnTable { table, gz } => {
                    for e in self.st.set_asn(table, &gz) {
                        eprintln!("errore: {e}");
                    }
                }
                JobEvent::Tested { cand, outcome } => {
                    self.st.apply_tested(h.kind, &cand, &outcome, &mut count);
                    if let Outcome::Good(g) = &outcome {
                        if g.https && !stopped {
                            found.push(g.clone());
                            if stop_at.is_some_and(|n| found.len() >= n) {
                                stopped = true;
                                h.cancel.cancel();
                            }
                        }
                    }
                }
                JobEvent::Connected(g) => {
                    self.st.upsert_good(&g, false);
                    connected = Some(g);
                }
                JobEvent::RealIp(ip) => {
                    if !self.quiet && !self.silent {
                        eprintln!("IP reale: {ip}");
                    }
                }
                JobEvent::Ips { .. } | JobEvent::Started(_) => {}
                JobEvent::Error(e) => {
                    if !self.silent {
                        eprintln!("errore: {e}");
                    }
                    errors.push(e);
                }
                JobEvent::Finished { kind, summary, cancelled: c } => {
                    cancelled = c && !stopped;
                    // test e ricontrollo stampano già il loro riepilogo con i conteggi
                    if !self.silent && (cancelled || !matches!(kind, JobKind::Test | JobKind::Recheck)) {
                        out!("{}{}", summary, if c { " (annullato)" } else { "" });
                    }
                    break;
                }
            }
        }
        watcher.abort();
        for e in self.st.finish(h.kind) {
            eprintln!("errore: {e}");
        }
        Driven { cancelled, count, connected, found, errors }
    }

    fn noop() -> Repaint {
        Arc::new(|| {})
    }

    // ---------- comandi ----------

    fn status(&mut self) -> Res {
        let st = &self.st;
        let alive = st.proxies.iter().filter(|p| p.alive).count();
        let https = st.proxies.iter().filter(|p| p.alive && p.https).count();
        out!("cartella dati     {}", st.data_dir());
        out!("lista scaricata   {} proxy non ancora provati", st.candidates.len());
        let mut by_proto: BTreeMap<Protocol, usize> = BTreeMap::new();
        for c in &st.candidates {
            *by_proto.entry(c.protocol).or_default() += 1;
        }
        if !by_proto.is_empty() {
            out!("                  {}", by_proto.iter().map(|(p, n)| format!("{p}: {n}")).collect::<Vec<_>>().join(" · "));
        }
        out!("proxy buoni       {alive} funzionanti ({https} con https), {} non rispondono", st.proxies.len() - alive);
        out!("storico test      {} proxy già provati", st.history.len());
        let en = st.sources.iter().filter(|s| s.enabled).count();
        out!("sorgenti          {en} abilitate su {}", st.sources.len());
        let mut best: Vec<_> = st.sources.iter().filter(|s| s.stats.tested > 0).collect();
        best.sort_by(|a, b| source_rating(b).total_cmp(&source_rating(a)));
        if !best.is_empty() {
            let top: Vec<String> = best.iter().take(3).map(|s| format!("{} {:.1}", s.id, source_rating(s) * 100.0)).collect();
            out!("migliori          {}", top.join(", "));
        }
        let t = &st.settings.test;
        out!(
            "opzioni test      {} proxy · {} in parallelo · timeout {}/{} s · ordine {} · protocollo {} · paese {}",
            st.settings.test_count,
            t.concurrency,
            t.http_timeout.as_secs(),
            t.https_timeout.as_secs(),
            if st.settings.order == Order::Priority { "priority" } else { "random" },
            st.settings.test_protocol.map(|p| p.to_string()).unwrap_or_else(|| "all".into()),
            if st.settings.test_country.is_empty() { "all" } else { &st.settings.test_country },
        );
        match (&st.asn, st.store.as_ref().and_then(|s| s.asn_modified())) {
            (Some(t), m) => out!("tabella ASN       {} intervalli di IP{}", t.len(), m.map(|m| format!(", scaricata {}", ago_sys(m))).unwrap_or_default()),
            (None, _) => out!("tabella ASN       assente: si scarica col prossimo download"),
        }
        let tested: u64 = st.net.net24.values().map(|g| g.tested).sum();
        out!(
            "rating per rete   {} ASN e {} /24 da {tested} prove{}",
            st.net.asn.len(),
            st.net.net24.len(),
            if rating::net_mean(&st.net).is_some() { "" } else { " (attivo da 100 prove)" }
        );
        out!("relay             127.0.0.1:{}", st.settings.relay_port);
        Ok(0)
    }

    /// Le reti migliori e peggiori per moltiplicatore di priorità (solo quelle con abbastanza prove).
    fn nets(&mut self, args: &[String]) -> Res {
        let n: usize = match opt(args, "--count", "-n")? {
            Some(v) => v.parse().map_err(|_| usage(format!("numero non valido: {v}")))?,
            None => 10,
        };
        let which = positional(args, &["--count", "-n"]);
        let (show_asn, show_24) = match which.first().map(String::as_str) {
            None => (true, true),
            Some("asn") => (true, false),
            Some("24") => (false, true),
            Some(w) => return Err(usage(format!("uso: nets [asn|24] [-n N], non {w}"))),
        };
        let st = &self.st;
        let lift = rating::NetLift::new(&st.net, st.asn.as_deref());
        let tested: u64 = st.net.net24.values().map(|g| g.tested).sum();
        let Some(mean) = lift.mean() else {
            out!("rating per rete non ancora attivo: servono almeno {} prove con almeno un buono (ora {tested})", rating::NET_MIN_TESTED);
            return Ok(0);
        };
        out!("{tested} prove, score medio {:.4} per prova; x = moltiplicatore della priorità (reti con almeno {NETS_MIN_SHOW} prove)", mean);
        let table = |title: &str, rows: Vec<(String, &GroupStats)>| {
            let mut rows: Vec<(String, &GroupStats, f64)> =
                rows.into_iter().filter(|(_, g)| g.tested >= NETS_MIN_SHOW).filter_map(|(l, g)| Some((l, g, lift.of_group(g)?))).collect();
            rows.sort_by(|a, b| b.2.total_cmp(&a.2));
            out!("\n{title}: {} reti", rows.len());
            if rows.is_empty() {
                return;
            }
            out!("{:>6}  {:>7}  {:>5}  RETE", "x", "PROVATI", "BUONI");
            // le prime n e le ultime n; in mezzo "…"
            let skip = rows.len().saturating_sub(2 * n);
            for (i, (l, g, x)) in rows.iter().enumerate() {
                if skip > 0 && i == n {
                    out!("{:>6}  ({skip} reti)", "…");
                }
                if skip == 0 || i < n || i >= n + skip {
                    out!("{x:>6.2}  {:>7}  {:>5}  {l}", g.tested, g.good);
                }
            }
        };
        if show_asn {
            if st.asn.is_none() {
                out!("\ntabella ASN assente: si scarica col prossimo download");
            }
            let name = |a: u32| st.asn.as_ref().map(|t| t.name(a).to_string()).unwrap_or_default();
            table("ASN", st.net.asn.iter().map(|(a, g)| (format!("AS{a} {}", name(*a)), g)).collect());
        }
        if show_24 {
            table("/24", st.net.net24.iter().map(|(k, g)| (rating::net24_label(*k), g)).collect());
        }
        Ok(0)
    }

    fn sources(&mut self, args: &[String]) -> Res {
        let sub = args.first().map(String::as_str).unwrap_or("list");
        let rest = if args.is_empty() { &[][..] } else { &args[1..] };
        match sub {
            "list" => {
                let mut l: Vec<_> = self.st.sources.iter().collect();
                l.sort_by(|a, b| source_rating(b).total_cmp(&source_rating(a)));
                out!("{:<3} {:<16} {:>6}  {:>16}  {:<24} NOME", "ON", "ID", "RATING", "PROV/BUONI/MORTI", "ULTIMO DOWNLOAD");
                for s in l {
                    let last = match (&s.stats.last_error, s.stats.last_download) {
                        (Some(e), _) => format!("errore: {}", e.chars().take(30).collect::<String>()),
                        (None, Some(_)) => format!("{} voci", s.stats.last_count),
                        (None, None) => "mai".into(),
                    };
                    out!(
                        "{:<3} {:<16} {:>6.1}  {:>16}  {:<24} {}{}",
                        if s.enabled { "x" } else { "" },
                        s.id,
                        source_rating(s) * 100.0,
                        format!("{}/{}/{}", s.stats.tested, s.stats.good, s.stats.dead),
                        last,
                        s.name,
                        if s.builtin { "" } else { " [personale]" }
                    );
                }
                Ok(0)
            }
            "enable" | "disable" => {
                let on = sub == "enable";
                if has(rest, "--all") {
                    self.st.sources.iter_mut().for_each(|s| s.enabled = on);
                } else {
                    if rest.is_empty() {
                        return Err(usage("indica almeno un id o --all"));
                    }
                    for id in rest {
                        let s = self.st.sources.iter_mut().find(|s| &s.id == id).ok_or_else(|| format!("sorgente sconosciuta: {id}"))?;
                        s.enabled = on;
                    }
                }
                self.save(&[Part::Sources])?;
                out!("{} sorgenti abilitate", self.st.sources.iter().filter(|s| s.enabled).count());
                Ok(0)
            }
            "add" => {
                let pos = positional(rest, &["--format", "--protocol"]);
                let [name, url] = pos.as_slice() else {
                    return Err(usage("uso: sources add <nome> <url> [--format F] [--protocol P]"));
                };
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    return Err(usage("l'URL deve iniziare con http:// o https://"));
                }
                let format = match opt(rest, "--format", "")? {
                    None => Format::Auto,
                    Some(f) => Format::ALL
                        .into_iter()
                        .find(|x| serde_json::to_value(x).ok().and_then(|v| v.as_str().map(|s| s.eq_ignore_ascii_case(f))).unwrap_or(false))
                        .ok_or_else(|| usage(format!("formato sconosciuto: {f}")))?,
                };
                let protocol = match opt(rest, "--protocol", "")? {
                    None => None,
                    Some(p) => Some(Protocol::parse(p).ok_or_else(|| usage(format!("protocollo sconosciuto: {p}")))?),
                };
                let s = custom_source(name, url, format, protocol, &self.st.sources);
                out!("aggiunta sorgente {} ({})", s.id, s.name);
                self.st.sources.push(s);
                self.save(&[Part::Sources])?;
                Ok(0)
            }
            "remove" => {
                if rest.is_empty() {
                    return Err(usage("indica almeno un id"));
                }
                for id in rest {
                    let before = self.st.sources.len();
                    self.st.sources.retain(|s| &s.id != id);
                    if before == self.st.sources.len() {
                        return Err(format!("sorgente sconosciuta: {id}"));
                    }
                    out!("rimossa {id}");
                }
                self.save(&[Part::Sources])?;
                Ok(0)
            }
            "reset" => {
                for s in self.st.sources.iter_mut().filter(|s| rest.is_empty() || rest.contains(&s.id)) {
                    rating::reset(s);
                }
                self.save(&[Part::Sources])?;
                out!("rating azzerati");
                Ok(0)
            }
            "restore" => {
                restore_default_sources(&mut self.st.sources);
                self.save(&[Part::Sources])?;
                out!("{} sorgenti ({} predefinite)", self.st.sources.len(), self.st.sources.iter().filter(|s| s.builtin).count());
                Ok(0)
            }
            other => Err(usage(format!("sottocomando sconosciuto: sources {other}"))),
        }
    }

    fn download(&mut self) -> Res {
        let enabled = self.st.enabled_sources();
        if enabled.is_empty() {
            return Err("nessuna sorgente abilitata (sources enable <id>)".into());
        }
        let h = job::start_download(self.rt.handle(), Self::noop(), enabled, self.st.known_keys(), self.st.asn_stale());
        Ok(if self.drive(h).cancelled { 130 } else { 0 })
    }

    fn apply_test_opts(&mut self, args: &[String]) -> Result<(), String> {
        let s = &mut self.st.settings;
        if let Some(v) = opt(args, "--count", "-n")? {
            s.test_count = round8(v.parse().map_err(|_| usage(format!("numero non valido: {v}")))?);
        }
        if let Some(v) = opt(args, "--parallel", "-j")? {
            s.test.concurrency = round8(v.parse().map_err(|_| usage(format!("numero non valido: {v}")))?).min(MAX_PARALLEL);
        }
        if let Some(v) = opt(args, "--protocol", "-p")? {
            s.test_protocol = if v == "all" { None } else { Some(Protocol::parse(v).ok_or_else(|| usage(format!("protocollo sconosciuto: {v}")))?) };
        }
        if let Some(v) = opt(args, "--country", "-c")? {
            s.test_country = if v == "all" { String::new() } else { v.to_ascii_uppercase() };
        }
        if let Some(v) = opt(args, "--timeout", "-t")? {
            let (h, hs) = match v {
                "rapidi" | "fast" => (5, 8),
                "normali" | "normal" => (10, 15),
                "pazienti" | "patient" => (15, 25),
                _ => return Err(usage(format!("timeout sconosciuto: {v}"))),
            };
            s.test.http_timeout = Duration::from_secs(h);
            s.test.https_timeout = Duration::from_secs(hs);
        }
        if let Some(v) = opt(args, "--order", "")? {
            s.order = match v {
                "priority" => Order::Priority,
                "random" => Order::Random,
                _ => return Err(usage(format!("ordine sconosciuto: {v}"))),
            };
        }
        if has(args, "--retry-timeout") {
            s.test.retry_after_timeout = true;
        }
        if has(args, "--no-retry-timeout") {
            s.test.retry_after_timeout = false;
        }
        if has(args, "--drop-http-only") {
            s.test.drop_http_only = true;
        }
        if has(args, "--keep-http-only") {
            s.test.drop_http_only = false;
        }
        Ok(())
    }

    fn test(&mut self, args: &[String]) -> Res {
        self.apply_test_opts(args)?;
        if has(args, "--save") {
            self.save(&[Part::Settings])?;
        }
        let cands = self.st.pick_candidates();
        if cands.is_empty() {
            return Err(if self.st.candidates.is_empty() {
                "lista scaricata vuota: esegui prima `proxy-finder download`".into()
            } else {
                "nessun candidato con questi filtri".into()
            });
        }
        eprintln!("provo {} proxy su {} disponibili, {} in parallelo", cands.len(), self.st.filtered_candidates().len(), self.st.settings.test.concurrency);
        let h = job::start_test(self.rt.handle(), Self::noop(), cands, self.st.settings.test.clone(), None);
        let d = self.drive(h);
        let c = d.count;
        out!("{} provati · {} buoni · {} aggiunti", c.tested, c.good, c.added);
        Ok(if d.cancelled { 130 } else { 0 })
    }

    fn list(&mut self, args: &[String]) -> Res {
        if let Some(v) = opt(args, "--sort", "")? {
            self.st.settings.sort = match v {
                "latency" => SortBy::Latency,
                "added" => SortBy::Added,
                _ => return Err(usage(format!("ordinamento sconosciuto: {v}"))),
            };
        }
        let all = has(args, "--all");
        let l: Vec<GoodProxy> = self.st.sorted_proxies().into_iter().filter(|p| all || p.alive).collect();
        if has(args, "--json") {
            out!("{}", serde_json::to_string_pretty(&l).map_err(|e| e.to_string())?);
            return Ok(0);
        }
        if l.is_empty() {
            eprintln!("nessun proxy: esegui `proxy-finder download` e `proxy-finder test`");
            return Ok(0);
        }
        out!("{:<6} {:<28} {:>8}  {:<16} {:>5}  {:<10} SORGENTI", "PAESE", "PROXY", "LATENZA", "IP USCITA", "SCORE", "STATO");
        for p in l {
            let stato = if !p.alive { "non risp." } else if p.https { "ok" } else { "SOLO HTTP" };
            out!(
                "{} {:<28} {:>5} ms  {:<16} {:>5.0}  {:<10} {}",
                cc(&p.country),
                p.key(),
                p.latency_ms,
                p.exit_ip,
                p.score * 100.0,
                stato,
                p.sources.join(",")
            );
        }
        Ok(0)
    }

    fn recheck(&mut self) -> Res {
        if self.st.proxies.is_empty() {
            return Err("nessun proxy da ricontrollare".into());
        }
        let mut cfg = self.st.settings.test.clone();
        timeouts_normal(&mut cfg);
        let h = job::start_recheck(self.rt.handle(), Self::noop(), self.st.proxies.clone(), cfg, None);
        let d = self.drive(h);
        out!("{} rispondono su {}", d.count.good, d.count.tested);
        Ok(if d.cancelled { 130 } else { 0 })
    }

    fn export(&mut self, args: &[String]) -> Res {
        let alive: Vec<GoodProxy> = self.st.sorted_proxies().into_iter().filter(|p| p.alive).collect();
        let text = if has(args, "--json") {
            serde_json::to_string_pretty(&alive).map_err(|e| e.to_string())?
        } else {
            alive.iter().map(|p| p.key() + "\n").collect()
        };
        match positional(args, &[]).first() {
            Some(path) => {
                std::fs::write(path, text).map_err(|e| format!("{path}: {e}"))?;
                eprintln!("{} proxy esportati in {path}", alive.len());
            }
            None => {
                use std::io::Write;
                let _ = std::io::stdout().write_all(text.as_bytes());
            }
        }
        Ok(0)
    }

    fn remove(&mut self, args: &[String]) -> Res {
        if args.is_empty() {
            return Err(usage("indica almeno un proto://ip:porta"));
        }
        for k in args {
            if !self.st.remove_proxy(k, None) {
                return Err(format!("non trovato: {k}"));
            }
            out!("rimosso {k}");
        }
        self.save(&[Part::Proxies])?;
        Ok(0)
    }

    fn prune(&mut self) -> Res {
        let n = self.st.remove_dead(None);
        self.save(&[Part::Proxies])?;
        out!("rimossi {n} proxy che non rispondono");
        Ok(0)
    }

    fn clear(&mut self, args: &[String]) -> Res {
        match args.first().map(String::as_str) {
            Some("list") => {
                let n = self.st.candidates.len();
                self.st.candidates.clear();
                self.save(&[Part::Candidates])?;
                out!("lista scaricata svuotata ({n} voci)");
            }
            Some("history") => {
                let n = self.st.history.len();
                self.st.history.clear();
                self.save(&[Part::History])?;
                out!("storico svuotato ({n} voci): i proxy già provati possono tornare al prossimo download");
            }
            Some("nets") => {
                let n = self.st.net.asn.len() + self.st.net.net24.len();
                self.st.net = Default::default();
                self.save(&[Part::Net])?;
                out!("rating per rete azzerato ({n} reti)");
            }
            _ => return Err(usage("uso: clear list|history|nets")),
        }
        Ok(0)
    }

    fn ip(&mut self, args: &[String]) -> Res {
        let through = match args.first() {
            Some(k) => Some(parse_proxy(k)?),
            None => None,
        };
        let h = job::start_verify_ip(self.rt.handle(), Self::noop(), through);
        let mut code = 0;
        for e in h.rx.iter() {
            match e {
                JobEvent::Ips { real, exit } => {
                    out!("IP reale   {real}");
                    if let Some(x) = exit {
                        out!("IP uscita  {x}{}", if x == real { "  (ATTENZIONE: uguale all'IP reale)" } else { "" });
                    }
                }
                JobEvent::Error(e) => {
                    eprintln!("errore: {e}");
                    code = 1;
                }
                JobEvent::Finished { .. } => break,
                _ => {}
            }
        }
        Ok(code)
    }

    fn relay(&mut self, args: &[String]) -> Res {
        if let Some(p) = opt(args, "--port", "")? {
            self.st.settings.relay_port = p.parse().map_err(|_| usage(format!("porta non valida: {p}")))?;
        }
        let list = match positional(args, &["--port"]).first() {
            Some(k) => {
                let key = parse_proxy(k)?.key();
                vec![self.st.proxies.iter().find(|p| p.key() == key).cloned().unwrap_or(parse_proxy(k)?)]
            }
            None => self.st.fastest_https(),
        };
        if list.is_empty() {
            return Err("nessun proxy https funzionante: esegui `proxy-finder test`, oppure indica proto://ip:porta".into());
        }
        let mut cfg = self.st.settings.test.clone();
        timeouts_normal(&mut cfg);
        eprintln!("verifico {} proxy…", list.len().min(64));
        let h = job::start_connect(self.rt.handle(), Self::noop(), list.into_iter().take(64).collect(), cfg, None);
        let d = self.drive(h);
        if d.cancelled {
            return Ok(130);
        }
        let Some(up) = d.connected else {
            return Err("nessun proxy ha risposto".into());
        };
        self.serve_or_switch(up)
    }

    /// Avvia il relay verso `up` e resta in ascolto fino a Ctrl+C. Se la porta è già tenuta da
    /// un relay proxy-finder da riga di comando, gli fa usare `up` ed esce subito.
    fn serve_or_switch(&mut self, up: GoodProxy) -> Res {
        let port = self.st.settings.relay_port;
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let store = self.st.store.clone().ok_or("cartella dati non disponibile")?;
        let listener = match self.rt.block_on(relay::bind_relay(addr)) {
            Ok(l) => l,
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                let pid = self.rt.block_on(relay::request_switch(&store, port, &up, Duration::from_secs(5)))?;
                if self.silent {
                    out!("{}", up.exit_ip);
                } else {
                    out!("relay già attivo su http://{addr} (pid {pid}): ora usa {} ({}, {} ms, uscita {})", up.key(), cc(&up.country), up.latency_ms, up.exit_ip);
                }
                return Ok(0);
            }
            Err(e) => return Err(format!("relay su {addr}: {e}")),
        };

        let upstream = Arc::new(RwLock::new(Some(up.clone())));
        let cancel = CancellationToken::new();
        self.rt.spawn(relay::serve(listener, upstream.clone(), cancel.clone()));
        let silent = self.silent;
        self.rt.spawn(relay::watch_switch(store.clone(), port, upstream, cancel.clone(), move |old, new| {
            if silent {
                out!("{}", new.exit_ip);
            } else {
                let from = old.map(GoodProxy::key).unwrap_or_default();
                out!("cambio proxy richiesto da un'altra istanza: {from} -> {} ({}, {} ms, uscita {})", new.key(), cc(&new.country), new.latency_ms, new.exit_ip);
            }
        }));
        if silent {
            out!("{}", up.exit_ip);
        } else {
            out!("relay attivo su http://{addr} -> {} ({}, {} ms, uscita {})", up.key(), cc(&up.country), up.latency_ms, up.exit_ip);
            if !up.https {
                out!("ATTENZIONE: proxy SOLO HTTP, i siti https non si aprono e quelli http passano in chiaro");
            }
            out!("prova: curl -x http://{addr} https://api.ipify.org");
            out!("Ctrl+C per fermare");
        }
        self.rt.block_on(async {
            let _ = tokio::signal::ctrl_c().await;
        });
        cancel.cancel();
        store.clear_relay();
        if !silent {
            out!("\nrelay fermato");
        }
        Ok(0)
    }

    /// Icona nel vassoio di sistema per collegarsi e scollegarsi (vedi `tray.rs`).
    #[cfg(target_os = "linux")]
    fn tray(&mut self, args: &[String]) -> Res {
        let port = match opt(args, "--port", "")? {
            Some(p) => p.parse().map_err(|_| usage(format!("porta non valida: {p}")))?,
            None => self.st.settings.relay_port,
        };
        Ok(self.rt.block_on(crate::tray::run(port)))
    }

    #[cfg(not(target_os = "linux"))]
    fn tray(&mut self, _args: &[String]) -> Res {
        Err(usage("il vassoio di sistema è disponibile solo su Linux"))
    }

    /// Modalità automatica: prova proxy nuovi a lotti di `AUTO_BATCH`, prima i socks5 e quelli
    /// dichiarati https, finché ne trova `AUTO_WANT` con https (scaricando le liste se la lista è
    /// vuota); dopo una pausa casuale li riprova tutti e si collega al più veloce. Esiti, rating e
    /// buoni si salvano come per `test`; la seconda prova aggiorna i buoni come `recheck`.
    /// Senza `-v` stampa solo l'IP di uscita.
    fn auto(&mut self, args: &[String]) -> Res {
        self.silent = !self.verbose;
        self.apply_test_opts(args)?;
        if let Some(p) = opt(args, "--port", "")? {
            self.st.settings.relay_port = p.parse().map_err(|_| usage(format!("porta non valida: {p}")))?;
        }
        let s = &mut self.st.settings;
        s.test_count = AUTO_BATCH;
        s.test.concurrency = AUTO_BATCH;
        s.test.drop_http_only = true;

        let mut downloaded = false;
        loop {
            let Some((found, exhausted)) = self.auto_scan(&mut downloaded)? else {
                return Ok(130);
            };
            if found.is_empty() {
                return Err("nessun proxy con https trovato e lista esaurita (prova `proxy-finder clear history`)".into());
            }
            // i proxy pubblici muoiono in fretta e la latenza della scansione è rumorosa: dopo
            // qualche secondo si riprovano tutti insieme e si usa il più veloce di questa seconda
            // prova (job Recheck: non tocca i rating, salva i buoni e segna "non risponde" chi è
            // caduto)
            let wait = rand::Rng::gen_range(&mut rand::thread_rng(), AUTO_JITTER_SECS);
            if !self.silent {
                out!("{} proxy con https trovati: tra {wait} s li riprovo tutti e scelgo il più veloce", found.len());
            }
            if self.sleep_or_ctrl_c(Duration::from_secs(wait)) {
                return Ok(130);
            }
            let h = job::start_recheck(self.rt.handle(), Self::noop(), found, self.st.settings.test.clone(), None);
            let d = self.drive(h);
            if d.cancelled {
                return Ok(130);
            }
            if let Some(best) = d.found.into_iter().min_by_key(|p| p.latency_ms) {
                if !self.silent {
                    out!("{} provati · {} rispondono · scelto il più veloce: {} ({} ms, uscita {})", d.count.tested, d.count.good, best.key(), best.latency_ms, best.exit_ip);
                }
                return self.serve_or_switch(best);
            }
            if exhausted {
                return Err("nessuno dei proxy trovati risponde più e la lista è esaurita".into());
            }
            if !self.silent {
                eprintln!("nessuno dei proxy trovati risponde più: continuo la scansione");
            }
        }
    }

    /// Attende `d`; true se nel frattempo arriva Ctrl+C.
    fn sleep_or_ctrl_c(&self, d: Duration) -> bool {
        self.rt.block_on(async {
            tokio::select! {
                _ = tokio::time::sleep(d) => false,
                r = tokio::signal::ctrl_c() => r.is_ok(),
            }
        })
    }

    /// Scansione di `auto`: lotti di `AUTO_BATCH` finché trova `AUTO_WANT` proxy con https.
    /// Restituisce i trovati (ordinati per latenza) e se la lista è finita; `None` se annullata.
    fn auto_scan(&mut self, downloaded: &mut bool) -> Result<Option<(Vec<GoodProxy>, bool)>, String> {
        let mut found: Vec<GoodProxy> = Vec::new();
        while found.len() < AUTO_WANT {
            if self.st.filtered_candidates().is_empty() {
                if *downloaded {
                    found.sort_by_key(|p| p.latency_ms);
                    return Ok(Some((found, true)));
                }
                *downloaded = true;
                if !self.silent {
                    eprintln!("nessun proxy nuovo da provare: scarico le sorgenti");
                }
                if self.download()? != 0 {
                    return Ok(None);
                }
                continue;
            }
            let cands = self.st.pick_candidates_tls_first();
            if !self.silent {
                let promising = cands.iter().filter(|c| c.tls_promising()).count();
                eprintln!(
                    "provo {} proxy nuovi ({promising} socks5 o dichiarati https), {} in parallelo: mi fermo a {} con https",
                    cands.len(),
                    AUTO_BATCH,
                    AUTO_WANT - found.len()
                );
            }
            let h = job::start_test(self.rt.handle(), Self::noop(), cands, self.st.settings.test.clone(), None);
            let d = self.drive_until(h, Some(AUTO_WANT - found.len()));
            if d.cancelled {
                return Ok(None);
            }
            if d.count.tested == 0 {
                // es. IP reale non disponibile: senza questo controllo il ciclo non finirebbe
                return Err(d.errors.last().cloned().unwrap_or_else(|| "nessun proxy provato in questo giro".into()));
            }
            if !self.silent {
                out!("{} provati · {} buoni con https · {} aggiunti", d.count.tested, d.found.len(), d.count.added);
            }
            found.extend(d.found);
        }
        found.sort_by_key(|p| p.latency_ms);
        Ok(Some((found, false)))
    }
}

/// Usato dai test: verifica che i nomi dei formati accettati da `sources add` siano quelli serde.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_names_match_serde() {
        for f in Format::ALL {
            let name = serde_json::to_value(f).unwrap();
            assert!(HELP.contains(name.as_str().unwrap()), "{name} manca nell'help");
        }
    }

    #[test]
    fn positional_skips_option_values() {
        let a: Vec<String> = ["x", "--format", "auto", "y", "--flag"].iter().map(|s| s.to_string()).collect();
        assert_eq!(positional(&a, &["--format"]), vec!["x", "y"]);
    }

    #[test]
    fn parse_proxy_accepts_urls() {
        assert_eq!(parse_proxy("socks5://1.2.3.4:1080").unwrap().key(), "socks5://1.2.3.4:1080");
        assert!(parse_proxy("1.2.3.4").is_err());
    }
}
