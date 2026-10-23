//! Stato persistente (sorgenti, buoni, lista scaricata, storico, impostazioni) e regole per
//! applicare gli esiti dei job. Condiviso da GUI e CLI, così si comportano allo stesso modo.

use crate::asn::AsnTable;
use crate::job::JobKind;
use crate::model::{now_secs, Candidate, GoodProxy, NetStats, Source};
use crate::rating;
use crate::store::{Order, Settings, SortBy, Store};
use crate::tester::{Outcome, Speed};
use rand::seq::SliceRandom;
use std::collections::HashSet;
use std::net::Ipv4Addr;
use std::sync::Arc;

pub struct State {
    pub store: Option<Store>,
    pub sources: Vec<Source>,
    pub proxies: Vec<GoodProxy>,
    pub candidates: Vec<Candidate>,
    pub history: HashSet<String>,
    pub settings: Settings,
    /// Rating per rete (ASN e /24).
    pub net: NetStats,
    /// Tabella IP -> ASN; senza, il rating per rete usa solo le /24.
    pub asn: Option<Arc<AsnTable>>,
}

/// Conteggi di un test: provati, buoni, aggiunti (nuovi tra i buoni).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TestCount {
    pub tested: usize,
    pub good: usize,
    pub added: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Sources,
    Proxies,
    Candidates,
    History,
    Settings,
    Net,
}

pub const ALL_PARTS: [Part; 6] = [Part::Sources, Part::Proxies, Part::Candidates, Part::History, Part::Settings, Part::Net];

impl State {
    /// Carica dallo store (o dai default se manca). I candidati già nello storico vengono tolti:
    /// se l'app è stata chiusa a metà test non vanno riprovati, il rating conta una volta sola.
    pub fn load(store: Option<Store>) -> State {
        let mut st = match &store {
            Some(s) => State {
                sources: s.load_sources(),
                proxies: s.load_proxies(),
                candidates: s.load_candidates(),
                history: s.load_history(),
                settings: s.load_settings(),
                net: s.load_net(),
                asn: s.load_asn().map(Arc::new),
                store: None,
            },
            None => State {
                sources: crate::sources::default_sources(),
                proxies: Vec::new(),
                candidates: Vec::new(),
                history: HashSet::new(),
                settings: Settings::default(),
                net: NetStats::default(),
                asn: None,
                store: None,
            },
        };
        st.store = store;
        let h = &st.history;
        st.candidates.retain(|c| !h.contains(&c.key()));
        st
    }

    /// Salva le parti indicate; restituisce gli errori (vuoto = tutto ok).
    pub fn save(&self, parts: &[Part]) -> Vec<String> {
        let Some(s) = &self.store else { return Vec::new() };
        parts
            .iter()
            .filter_map(|p| {
                let r = match p {
                    Part::Sources => s.save_sources(&self.sources),
                    Part::Proxies => s.save_proxies(&self.proxies),
                    Part::Candidates => s.save_candidates(&self.candidates),
                    Part::History => s.save_history(&self.history),
                    Part::Settings => s.save_settings(&self.settings),
                    Part::Net => s.save_net(&self.net),
                };
                r.err().map(|e| format!("salvataggio {p:?}: {e}"))
            })
            .collect()
    }

    pub fn data_dir(&self) -> String {
        self.store.as_ref().map(|s| s.dir.display().to_string()).unwrap_or_default()
    }

    /// Chiavi da non riscaricare: già provate o già tra i buoni.
    pub fn known_keys(&self) -> HashSet<String> {
        let mut k = self.history.clone();
        k.extend(self.proxies.iter().map(|p| p.key()));
        k
    }

    /// Sorgenti abilitate, le migliori prima (aprono il round-robin del merge).
    pub fn enabled_sources(&self) -> Vec<Source> {
        let mut l: Vec<Source> = self.sources.iter().filter(|s| s.enabled).cloned().collect();
        l.sort_by(|a, b| rating::source_rating(b).total_cmp(&rating::source_rating(a)));
        l
    }

    /// Registra l'esito del download di una sorgente.
    pub fn record_download(&mut self, id: &str, count: usize, error: Option<String>) {
        if let Some(s) = self.sources.iter_mut().find(|s| s.id == id) {
            s.stats.last_download = Some(now_secs());
            if error.is_none() {
                s.stats.last_count = count as u64;
            }
            s.stats.last_error = error;
        }
    }

    /// Aggiunge o aggiorna un buono. Restituisce true se è nuovo.
    pub fn upsert_good(&mut self, g: &GoodProxy, merge_sources: bool) -> bool {
        match self.proxies.iter_mut().find(|p| p.key() == g.key()) {
            Some(p) => {
                p.latency_ms = g.latency_ms;
                p.exit_ip = g.exit_ip.clone();
                p.country = g.country.clone();
                p.https = g.https;
                p.score = g.score;
                p.checked_at = g.checked_at;
                p.alive = true;
                if merge_sources {
                    for s in &g.sources {
                        if !p.sources.contains(s) {
                            p.sources.push(s.clone());
                        }
                    }
                }
                false
            }
            None => {
                self.proxies.push(g.clone());
                true
            }
        }
    }

    /// Applica l'esito di una prova. Solo il Test (prima prova) muove rating e storico;
    /// ricontrollo e connessione aggiornano solo i buoni. Gli annullati non contano.
    pub fn apply_tested(&mut self, kind: JobKind, cand: &Candidate, outcome: &Outcome, count: &mut TestCount) {
        if *outcome == Outcome::Cancelled {
            return;
        }
        match kind {
            JobKind::Test => {
                rating::apply_outcome(&mut self.sources, cand, outcome);
                let asn = self.asn_of(cand.ip);
                rating::apply_net_outcome(&mut self.net, asn, cand.ip, outcome);
                self.history.insert(cand.key());
                count.tested += 1;
                if let Outcome::Good(g) = outcome {
                    count.good += 1;
                    if self.upsert_good(g, true) {
                        count.added += 1;
                    }
                }
            }
            JobKind::Recheck | JobKind::Connect => match outcome {
                Outcome::Good(g) => {
                    count.tested += 1;
                    count.good += 1;
                    self.upsert_good(g, false);
                }
                Outcome::Dead { .. } => {
                    count.tested += 1;
                    let k = cand.key();
                    if let Some(p) = self.proxies.iter_mut().find(|p| p.key() == k) {
                        p.alive = false;
                        p.checked_at = now_secs();
                    }
                }
                Outcome::Cancelled => {}
            },
            _ => {}
        }
    }

    /// Chiusura di un job: toglie i provati dalla lista e salva. Restituisce gli errori di salvataggio.
    pub fn finish(&mut self, kind: JobKind) -> Vec<String> {
        match kind {
            JobKind::Download => self.save(&[Part::Sources, Part::Candidates]),
            JobKind::Test => {
                let h = &self.history;
                self.candidates.retain(|c| !h.contains(&c.key()));
                self.save(&[Part::Sources, Part::Proxies, Part::Candidates, Part::History, Part::Net])
            }
            JobKind::Recheck | JobKind::Connect | JobKind::Speed => self.save(&[Part::Proxies]),
            JobKind::VerifyIp => Vec::new(),
        }
    }

    pub fn asn_of(&self, ip: Ipv4Addr) -> Option<u32> {
        self.asn.as_ref().and_then(|t| t.lookup(ip))
    }

    /// La tabella ASN manca o è più vecchia di `asn::MAX_AGE`: il prossimo download la riscarica.
    pub fn asn_stale(&self) -> bool {
        let Some(s) = &self.store else { return self.asn.is_none() };
        self.asn.is_none() || s.asn_modified().and_then(|t| t.elapsed().ok()).is_none_or(|age| age > crate::asn::MAX_AGE)
    }

    /// Tabella ASN appena scaricata: la usa e la salva compressa. Restituisce gli errori di salvataggio.
    pub fn set_asn(&mut self, table: Arc<AsnTable>, gz: &[u8]) -> Vec<String> {
        self.asn = Some(table);
        match &self.store {
            Some(s) => s.save_asn(gz).err().map(|e| format!("salvataggio tabella ASN: {e}")).into_iter().collect(),
            None => Vec::new(),
        }
    }

    /// Rimuove un buono (mai quello attivo). Restituisce true se l'ha tolto.
    pub fn remove_proxy(&mut self, key: &str, active: Option<&str>) -> bool {
        if active == Some(key) {
            return false;
        }
        let before = self.proxies.len();
        self.proxies.retain(|p| p.key() != key);
        before != self.proxies.len()
    }

    /// Toglie i buoni marcati "non risponde", tranne l'attivo. Restituisce quanti.
    pub fn remove_dead(&mut self, active: Option<&str>) -> usize {
        let before = self.proxies.len();
        self.proxies.retain(|p| p.alive || Some(p.key().as_str()) == active);
        before - self.proxies.len()
    }

    /// Esito di uno speedtest: la velocità misurata (anche se fermato al tetto); 0 se scartato o
    /// fallito.
    pub fn apply_speed(&mut self, key: &str, r: &Result<Speed, String>) {
        if let Some(p) = self.proxies.iter_mut().find(|p| p.key() == key) {
            p.speed_bps = Some(r.as_ref().map_or(0, |s| s.bytes_per_sec() as u64));
        }
    }

    pub fn sorted_proxies(&self) -> Vec<GoodProxy> {
        let mut l = self.proxies.clone();
        match self.settings.sort {
            SortBy::Latency => l.sort_by_key(|p| (!p.alive, p.latency_ms)),
            SortBy::Added => l.sort_by_key(|p| std::cmp::Reverse(p.added_at)),
            // i più veloci prima, poi i mai misurati, ultimi i non funzionanti
            SortBy::Speed => l.sort_by_key(|p| (!p.alive, p.speed_bps.is_none(), std::cmp::Reverse(p.speed_bps), p.latency_ms)),
        }
        l
    }

    /// Proxy per "Connetti al più veloce" e per lo speedtest (il file di prova è in https):
    /// funzionanti, con https, in ordine di latenza.
    pub fn fastest_https(&self) -> Vec<GoodProxy> {
        let mut l: Vec<GoodProxy> = self.proxies.iter().filter(|p| p.https && p.alive).cloned().collect();
        l.sort_by_key(|p| p.latency_ms);
        l
    }

    /// Paesi del filtro, a 2 o 3 lettere: "ITA, US" -> ["IT", "US"].
    pub fn country_filter(&self) -> Vec<String> {
        self.settings.test_country.split([',', ' ']).filter_map(crate::countries::to_alpha2).collect()
    }

    /// Candidati che passano i filtri protocollo/paese delle impostazioni.
    pub fn filtered_candidates(&self) -> Vec<&Candidate> {
        let countries = self.country_filter();
        self.candidates
            .iter()
            .filter(|c| self.settings.test_protocol.is_none_or(|p| c.protocol == p))
            .filter(|c| countries.is_empty() || countries.contains(&c.country))
            .collect()
    }

    /// I candidati da provare: filtrati, ordinati (priorità a fasce o a caso), primi `test_count`.
    pub fn pick_candidates(&self) -> Vec<Candidate> {
        let mut pool = self.ordered_candidates();
        pool.truncate(self.settings.test_count);
        pool
    }

    /// Come `pick_candidates`, ma prima i promettenti per una connessione cifrata (socks5 o
    /// dichiarati https), ciascun gruppo nel suo ordine; finiti quelli si passa agli altri.
    pub fn pick_candidates_tls_first(&self) -> Vec<Candidate> {
        let (mut pool, rest): (Vec<Candidate>, Vec<Candidate>) = self.ordered_candidates().into_iter().partition(Candidate::tls_promising);
        pool.extend(rest);
        pool.truncate(self.settings.test_count);
        pool
    }

    fn ordered_candidates(&self) -> Vec<Candidate> {
        let mut pool: Vec<Candidate> = self.filtered_candidates().into_iter().cloned().collect();
        match self.settings.order {
            Order::Priority => {
                let net = rating::NetLift::new(&self.net, self.asn.as_deref());
                pool = rating::order_by_priority(pool, &rating::ratings(&self.sources), &net);
            }
            Order::Random => pool.shuffle(&mut rand::thread_rng()),
        }
        pool
    }
}

/// Tetto delle prove in parallelo (il numero di proxy da provare non ne ha).
pub const MAX_PARALLEL: usize = 512;

/// Arrotonda al multiplo di 8 superiore, almeno 8.
pub fn round8(n: usize) -> usize {
    n.div_ceil(8).max(1) * 8
}
