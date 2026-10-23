//! Score dei proxy e rating delle sorgenti.
//!
//! Il rating di una sorgente è una media bayesiana degli score dei proxy che ha fornito:
//! ogni proxy buono aggiunge il suo score (0..1) al numeratore, ogni proxy morto aggiunge 0;
//! entrambi contano 1 al denominatore. Il prior pesa come `PRIOR_WEIGHT` prove fittizie, così
//! poche prove non spostano troppo il valore e le liste enormi non vanno a meno infinito.
//!
//! Il rating per rete fa lo stesso conto sui gruppi di IP: l'ASN (il provider) e la /24. Una /24
//! parte dal rating del suo ASN, l'ASN dalla media di tutte le prove; il risultato diviso per la
//! media è il moltiplicatore ("lift") della priorità: > 1 rete migliore della media, < 1 peggiore.

use crate::asn::AsnTable;
use crate::model::{Candidate, GroupStats, NetStats, Source};
use crate::tester::Outcome;
use std::collections::HashMap;
use std::net::Ipv4Addr;

pub const PRIOR_WEIGHT: f64 = 5.0;

/// Bontà di un proxy in [0.025, 1]: latenza bassa e supporto https (CONNECT) contano.
pub fn proxy_score(latency_ms: u32, https: bool) -> f64 {
    let lat = (1.0 - latency_ms as f64 / 5000.0).clamp(0.05, 1.0);
    let proto = if https { 1.0 } else { 0.5 };
    lat * proto
}

pub fn source_rating(s: &Source) -> f64 {
    let r = (s.stats.score_sum + s.prior * PRIOR_WEIGHT) / (s.stats.tested as f64 + PRIOR_WEIGHT);
    r.clamp(0.0, 1.0)
}

pub fn record_good(s: &mut Source, score: f64) {
    s.stats.tested += 1;
    s.stats.good += 1;
    s.stats.score_sum += score.clamp(0.0, 1.0);
}

pub fn record_dead(s: &mut Source) {
    s.stats.tested += 1;
    s.stats.dead += 1;
}

pub fn reset(s: &mut Source) {
    s.stats.tested = 0;
    s.stats.good = 0;
    s.stats.dead = 0;
    s.stats.score_sum = 0.0;
}

/// Accredita l'esito della prima prova di un candidato a tutte le sue sorgenti.
/// `Cancelled` non conta.
pub fn apply_outcome(sources: &mut [Source], cand: &Candidate, outcome: &Outcome) {
    for s in sources.iter_mut().filter(|s| cand.sources.contains(&s.id)) {
        match outcome {
            Outcome::Good(g) => record_good(s, g.score),
            Outcome::Dead { .. } => record_dead(s),
            Outcome::Cancelled => {}
        }
    }
}

/// Peso del rating "di partenza" di un gruppo di rete, in prove fittizie.
pub const NET_PRIOR_WEIGHT: f64 = 50.0;
/// Prove (in tutto) sotto le quali il rating per rete non sposta ancora la priorità.
pub const NET_MIN_TESTED: u64 = 100;
/// Limiti del moltiplicatore di priorità di una rete.
pub const NET_LIFT_RANGE: (f64, f64) = (0.05, 20.0);

/// La /24 di un IP: l'indirizzo senza l'ultimo ottetto.
pub fn net24(ip: Ipv4Addr) -> u32 {
    u32::from(ip) >> 8
}

/// "a.b.c.0/24" per la chiave di `net24`.
pub fn net24_label(n: u32) -> String {
    format!("{}/24", Ipv4Addr::from(n << 8))
}

fn record_group(g: &mut GroupStats, outcome: &Outcome) {
    match outcome {
        Outcome::Good(p) => {
            g.tested += 1;
            g.good += 1;
            g.score_sum += p.score.clamp(0.0, 1.0);
        }
        Outcome::Dead { .. } => g.tested += 1,
        Outcome::Cancelled => {}
    }
}

/// Accredita l'esito della prima prova alla /24 e, se noto, all'ASN dell'IP. `Cancelled` non conta.
pub fn apply_net_outcome(stats: &mut NetStats, asn: Option<u32>, ip: Ipv4Addr, outcome: &Outcome) {
    if *outcome == Outcome::Cancelled {
        return;
    }
    record_group(stats.net24.entry(net24(ip)).or_default(), outcome);
    if let Some(a) = asn {
        record_group(stats.asn.entry(a).or_default(), outcome);
    }
}

/// Score medio per prova su tutte le prove (ogni prova ha una /24); `None` finché le prove sono
/// meno di `NET_MIN_TESTED` o nessuna è andata bene.
pub fn net_mean(stats: &NetStats) -> Option<f64> {
    let (tested, sum) = stats.net24.values().fold((0u64, 0.0), |(t, s), g| (t + g.tested, s + g.score_sum));
    (tested >= NET_MIN_TESTED && sum > 0.0).then(|| sum / tested as f64)
}

/// Media bayesiana dello score di un gruppo, che parte da `prior` (peso `NET_PRIOR_WEIGHT`).
pub fn group_rating(g: Option<&GroupStats>, prior: f64) -> f64 {
    match g {
        Some(g) => (g.score_sum + prior * NET_PRIOR_WEIGHT) / (g.tested as f64 + NET_PRIOR_WEIGHT),
        None => prior,
    }
}

/// Moltiplicatore di priorità per rete (ASN e /24), calcolato una volta per ordinamento.
pub struct NetLift<'a> {
    stats: &'a NetStats,
    asn: Option<&'a AsnTable>,
    mean: Option<f64>,
}

impl<'a> NetLift<'a> {
    pub fn new(stats: &'a NetStats, asn: Option<&'a AsnTable>) -> NetLift<'a> {
        NetLift { stats, asn, mean: net_mean(stats) }
    }

    /// Nessun effetto: ogni rete vale 1.
    pub fn none() -> NetLift<'static> {
        static EMPTY: std::sync::OnceLock<NetStats> = std::sync::OnceLock::new();
        NetLift { stats: EMPTY.get_or_init(NetStats::default), asn: None, mean: None }
    }

    /// Rating dell'ASN (dalla media) e della /24 (dal rating dell'ASN), diviso la media.
    pub fn of(&self, ip: Ipv4Addr) -> f64 {
        let Some(mean) = self.mean else { return 1.0 };
        let asn = self.asn.and_then(|t| t.lookup(ip));
        let asn_r = group_rating(asn.and_then(|a| self.stats.asn.get(&a)), mean);
        let net_r = group_rating(self.stats.net24.get(&net24(ip)), asn_r);
        (net_r / mean).clamp(NET_LIFT_RANGE.0, NET_LIFT_RANGE.1)
    }

    /// Moltiplicatore di un gruppo qualsiasi rispetto alla media (per mostrarlo).
    pub fn of_group(&self, g: &GroupStats) -> Option<f64> {
        let mean = self.mean?;
        Some((group_rating(Some(g), mean) / mean).clamp(NET_LIFT_RANGE.0, NET_LIFT_RANGE.1))
    }

    pub fn mean(&self) -> Option<f64> {
        self.mean
    }
}

pub fn ratings(sources: &[Source]) -> HashMap<String, f64> {
    sources.iter().map(|s| (s.id.clone(), source_rating(s))).collect()
}

/// Priorità di prova: miglior rating tra le sorgenti che elencano il candidato,
/// pesato dai metadati dichiarati (latenza, altrimenti uptime).
pub fn candidate_priority(c: &Candidate, ratings: &HashMap<String, f64>) -> f64 {
    let r = c
        .sources
        .iter()
        .filter_map(|s| ratings.get(s).copied())
        .fold(None, |acc: Option<f64>, v| Some(acc.map_or(v, |a| a.max(v))))
        .unwrap_or(0.3);
    let meta = match (c.latency_ms, c.uptime) {
        (Some(l), _) => (1.0 - l as f64 / 5000.0).clamp(0.0, 1.0),
        (None, Some(u)) => u as f64 / 100.0,
        _ => 0.5,
    };
    r * (0.5 + 0.5 * meta)
}

/// Ordina per priorità (sorgenti × rete) a fasce larghe circa il 28% (un quarto di unità sulla
/// scala logaritmica naturale): dentro una fascia resta l'ordine di partenza (l'alternanza tra
/// sorgenti del merge), così una lista enorme con metadati appena migliori non monopolizza la
/// scansione. Le fasce sono relative perché dopo molte prove i rating sono tutti piccoli.
pub fn order_by_priority(mut list: Vec<Candidate>, ratings: &HashMap<String, f64>, net: &NetLift) -> Vec<Candidate> {
    list.sort_by_cached_key(|c| {
        let p = candidate_priority(c, ratings) * net.of(c.ip);
        std::cmp::Reverse((p.max(1e-9).ln() * 4.0).round() as i64)
    });
    list
}

/// Velocità di download per GUI e CLI: "1.23 MB/s", "scartato" (0) o "—" (mai misurata).
pub fn speed_text(bps: Option<u64>) -> String {
    match bps {
        None => "—".into(),
        Some(0) => "scartato".into(),
        Some(b) => format!("{:.2} MB/s", b as f64 / 1e6),
    }
}
