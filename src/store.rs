//! Persistenza JSON nella cartella di configurazione.

use crate::asn::{self, AsnTable};
use crate::model::{Candidate, GoodProxy, NetStats, Protocol, Source};
use crate::sources::default_sources;
use crate::tester::TestConfig;
use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Order {
    Priority,
    Random,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortBy {
    Latency,
    Added,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub test: TestConfig,
    pub order: Order,
    pub relay_port: u16,
    pub sort: SortBy,
    /// Quanti candidati provare per scansione (multiplo di 8).
    pub test_count: usize,
    /// Filtro protocollo per la scansione (None = tutti).
    pub test_protocol: Option<Protocol>,
    /// Filtro paese alpha-2 ("" = tutti).
    pub test_country: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            test: TestConfig::default(),
            order: Order::Priority,
            relay_port: 8899,
            sort: SortBy::Latency,
            test_count: 64,
            test_protocol: None,
            test_country: String::new(),
        }
    }
}

/// Forma compatta di un candidato: `[proto, ip, port, country, latency|null, uptime|null, [sources], https]`.
type CompactCandidate = (Protocol, Ipv4Addr, u16, String, Option<u32>, Option<u8>, Vec<String>, bool);

/// Riga di `candidates.json`: le versioni fino alla 0.1.6 non avevano `https` (7 elementi).
#[derive(Deserialize)]
#[serde(untagged)]
enum CandidateRow {
    Current(CompactCandidate),
    Old((Protocol, Ipv4Addr, u16, String, Option<u32>, Option<u8>, Vec<String>)),
}

#[derive(Clone)]
pub struct Store {
    pub dir: PathBuf,
}

/// Relay avviato da riga di comando: porta, processo che lo tiene aperto e proxy attivo.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelayInfo {
    pub port: u16,
    pub pid: u32,
    pub upstream: GoodProxy,
}

/// Richiesta, da un'altra istanza, al relay sulla porta `port` di passare a `upstream`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SwitchRequest {
    pub port: u16,
    pub upstream: GoodProxy,
}

impl Store {
    /// Cartella da `PROXY_FINDER_HOME` o quella di configurazione del sistema.
    pub fn open() -> Result<Store> {
        let dir = match std::env::var_os("PROXY_FINDER_HOME") {
            Some(d) => PathBuf::from(d),
            None => directories::ProjectDirs::from("dev", "lucatarik", "proxy-finder")
                .context("cartella di configurazione non trovata")?
                .config_dir()
                .to_path_buf(),
        };
        Store::at(dir)
    }

    pub fn at(dir: PathBuf) -> Result<Store> {
        std::fs::create_dir_all(&dir).with_context(|| format!("creazione di {}", dir.display()))?;
        Ok(Store { dir })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Legge un file JSON; se manca restituisce `None`, se è corrotto lo copia in `<file>.bad`.
    fn load<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        let p = self.path(name);
        let text = std::fs::read_to_string(&p).ok()?;
        match serde_json::from_str(&text) {
            Ok(v) => Some(v),
            Err(_) => {
                let _ = std::fs::copy(&p, self.path(&format!("{name}.bad")));
                None
            }
        }
    }

    fn save<T: Serialize + ?Sized>(&self, name: &str, value: &T) -> Result<()> {
        let data = serde_json::to_vec(value)?;
        write_atomic(&self.path(name), &data)
    }

    /// Le sorgenti salvate; quelle predefinite prendono sempre gli endpoint del codice
    /// (non si modificano dall'interfaccia e così ricevono le correzioni delle nuove versioni).
    pub fn load_sources(&self) -> Vec<Source> {
        let Some(mut list) = self.load::<Vec<Source>>("sources.json") else { return default_sources() };
        let defaults = default_sources();
        for s in list.iter_mut().filter(|s| s.builtin) {
            if let Some(d) = defaults.iter().find(|d| d.id == s.id) {
                s.endpoints = d.endpoints.clone();
            }
        }
        list
    }
    pub fn save_sources(&self, s: &[Source]) -> Result<()> {
        self.save("sources.json", s)
    }

    pub fn load_proxies(&self) -> Vec<GoodProxy> {
        self.load("proxies.json").unwrap_or_default()
    }
    pub fn save_proxies(&self, p: &[GoodProxy]) -> Result<()> {
        self.save("proxies.json", p)
    }

    pub fn load_candidates(&self) -> Vec<Candidate> {
        let rows: Vec<CandidateRow> = self.load("candidates.json").unwrap_or_default();
        rows.into_iter()
            .map(|r| match r {
                CandidateRow::Current(t) => t,
                CandidateRow::Old((p, i, po, c, l, u, s)) => (p, i, po, c, l, u, s, false),
            })
            .map(|(protocol, ip, port, country, latency_ms, uptime, sources, https)| Candidate {
                protocol,
                ip,
                port,
                country,
                latency_ms,
                uptime,
                sources,
                https,
            })
            .collect()
    }
    pub fn save_candidates(&self, c: &[Candidate]) -> Result<()> {
        let rows: Vec<CompactCandidate> = c
            .iter()
            .map(|c| (c.protocol, c.ip, c.port, c.country.clone(), c.latency_ms, c.uptime, c.sources.clone(), c.https))
            .collect();
        self.save("candidates.json", &rows)
    }

    pub fn load_history(&self) -> HashSet<String> {
        self.load::<Vec<String>>("history.json").unwrap_or_default().into_iter().collect()
    }
    pub fn save_history(&self, h: &HashSet<String>) -> Result<()> {
        let mut v: Vec<&String> = h.iter().collect();
        v.sort();
        self.save("history.json", &v)
    }

    /// Data di modifica di `relay.json` (`None` se manca): per rileggerlo solo quando cambia.
    pub fn relay_modified(&self) -> Option<std::time::SystemTime> {
        modified(&self.path("relay.json"))
    }
    pub fn load_relay(&self) -> Option<RelayInfo> {
        self.load("relay.json")
    }
    pub fn save_relay(&self, r: &RelayInfo) -> Result<()> {
        self.save("relay.json", r)
    }
    pub fn clear_relay(&self) {
        let _ = std::fs::remove_file(self.path("relay.json"));
    }

    /// Data di modifica di `relay-switch.json` (`None` se manca: nessuna richiesta).
    pub fn switch_modified(&self) -> Option<std::time::SystemTime> {
        modified(&self.path("relay-switch.json"))
    }
    pub fn save_switch(&self, r: &SwitchRequest) -> Result<()> {
        self.save("relay-switch.json", r)
    }
    /// Toglie e restituisce la richiesta di cambio per `port`; le richieste per altre porte restano.
    pub fn take_switch(&self, port: u16) -> Option<SwitchRequest> {
        let r: SwitchRequest = self.load("relay-switch.json")?;
        if r.port != port {
            return None;
        }
        self.clear_switch();
        Some(r)
    }
    pub fn clear_switch(&self) {
        let _ = std::fs::remove_file(self.path("relay-switch.json"));
    }

    pub fn load_net(&self) -> NetStats {
        self.load("net-ratings.json").unwrap_or_default()
    }
    pub fn save_net(&self, n: &NetStats) -> Result<()> {
        self.save("net-ratings.json", n)
    }

    /// Tabella ASN salvata; se illeggibile la sposta in `<file>.bad` (si riscarica col download).
    pub fn load_asn(&self) -> Option<AsnTable> {
        let p = self.path(asn::FILE);
        let gz = std::fs::read(&p).ok()?;
        match AsnTable::from_gz(&gz) {
            Ok(t) => Some(t),
            Err(_) => {
                let _ = std::fs::rename(&p, self.path(&format!("{}.bad", asn::FILE)));
                None
            }
        }
    }
    pub fn save_asn(&self, gz: &[u8]) -> Result<()> {
        write_atomic(&self.path(asn::FILE), gz)
    }
    pub fn asn_modified(&self) -> Option<std::time::SystemTime> {
        modified(&self.path(asn::FILE))
    }

    pub fn load_settings(&self) -> Settings {
        self.load("settings.json").unwrap_or_default()
    }
    pub fn save_settings(&self, s: &Settings) -> Result<()> {
        self.save("settings.json", s)
    }
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, data).with_context(|| format!("scrittura di {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rinomina in {}", path.display()))?;
    Ok(())
}

/// Rimette le sorgenti predefinite com'erano (endpoint, nome, prior), conservando le loro
/// statistiche e lo stato abilitata; le sorgenti personalizzate restano in coda.
pub fn restore_default_sources(sources: &mut Vec<Source>) {
    let old = std::mem::take(sources);
    let mut out = default_sources();
    for d in out.iter_mut() {
        if let Some(prev) = old.iter().find(|s| s.id == d.id) {
            d.stats = prev.stats.clone();
            d.enabled = prev.enabled;
        }
    }
    out.extend(old.into_iter().filter(|s| !s.builtin));
    *sources = out;
}
