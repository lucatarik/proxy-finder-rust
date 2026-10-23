//! Score dei proxy e rating delle sorgenti.
//!
//! Il rating di una sorgente è una media bayesiana degli score dei proxy che ha fornito:
//! ogni proxy buono aggiunge il suo score (0..1) al numeratore, ogni proxy morto aggiunge 0;
//! entrambi contano 1 al denominatore. Il prior pesa come `PRIOR_WEIGHT` prove fittizie, così
//! poche prove non spostano troppo il valore e le liste enormi non vanno a meno infinito.

use crate::model::{Candidate, Source};
use crate::tester::Outcome;
use std::collections::HashMap;

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

/// Ordina per priorità a fasce di 0.05: dentro una fascia resta l'ordine di partenza
/// (l'alternanza tra sorgenti del merge), così una lista enorme con metadati appena
/// migliori non monopolizza la scansione.
pub fn order_by_priority(mut list: Vec<Candidate>, ratings: &HashMap<String, f64>) -> Vec<Candidate> {
    list.sort_by_cached_key(|c| std::cmp::Reverse((candidate_priority(c, ratings) * 20.0).round() as i64));
    list
}
