//! Tabella IP -> ASN (rete del provider) per il rating per rete. Viene da iptoasn.com
//! (pubblico dominio), si scarica insieme alle sorgenti e si tiene compressa nella cartella dati.

use crate::sources::fetch_bytes;
use flate2::read::GzDecoder;
use std::collections::HashMap;
use std::io::Read;
use std::net::Ipv4Addr;
use std::time::Duration;

pub const URL: &str = "https://iptoasn.com/data/ip2asn-v4.tsv.gz";
/// Nome del file nella cartella dati.
pub const FILE: &str = "ip2asn-v4.tsv.gz";
/// Oltre questa età il download delle sorgenti la riscarica.
pub const MAX_AGE: Duration = Duration::from_secs(7 * 86_400);

/// Intervalli IPv4 ordinati per inizio, ciascuno con il suo ASN (0 = non instradato).
#[derive(Default)]
pub struct AsnTable {
    starts: Vec<u32>,
    ends: Vec<u32>,
    asns: Vec<u32>,
    names: HashMap<u32, String>,
}

impl std::fmt::Debug for AsnTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AsnTable({} intervalli)", self.len())
    }
}

impl AsnTable {
    /// Righe `inizio \t fine \t asn \t paese \t nome`; quelle malformate si saltano.
    pub fn parse(text: &str) -> AsnTable {
        let mut rows: Vec<(u32, u32, u32)> = Vec::new();
        let mut names = HashMap::new();
        for line in text.lines() {
            let mut f = line.split('\t');
            let (Some(a), Some(b), Some(n)) = (f.next(), f.next(), f.next()) else { continue };
            let (Ok(a), Ok(b), Ok(n)) = (a.parse::<Ipv4Addr>(), b.parse::<Ipv4Addr>(), n.parse::<u32>()) else { continue };
            let (a, b) = (u32::from(a), u32::from(b));
            if a > b {
                continue;
            }
            rows.push((a, b, n));
            if let Some(name) = f.nth(1).filter(|s| !s.is_empty()) {
                names.entry(n).or_insert_with(|| name.to_string());
            }
        }
        rows.sort_unstable_by_key(|r| r.0);
        AsnTable {
            starts: rows.iter().map(|r| r.0).collect(),
            ends: rows.iter().map(|r| r.1).collect(),
            asns: rows.iter().map(|r| r.2).collect(),
            names,
        }
    }

    /// Decomprime e legge il file `.tsv.gz`; errore se non contiene nessun intervallo.
    pub fn from_gz(gz: &[u8]) -> Result<AsnTable, String> {
        let mut text = String::new();
        GzDecoder::new(gz).read_to_string(&mut text).map_err(|e| format!("tabella ASN illeggibile: {e}"))?;
        let t = AsnTable::parse(&text);
        if t.is_empty() {
            return Err("tabella ASN vuota".into());
        }
        Ok(t)
    }

    pub fn lookup(&self, ip: Ipv4Addr) -> Option<u32> {
        let x = u32::from(ip);
        let i = self.starts.partition_point(|&s| s <= x).checked_sub(1)?;
        (self.ends[i] >= x).then_some(self.asns[i])
    }

    pub fn name(&self, asn: u32) -> &str {
        self.names.get(&asn).map(String::as_str).unwrap_or("")
    }

    pub fn len(&self) -> usize {
        self.starts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.starts.is_empty()
    }
}

/// Scarica la tabella e la legge (fuori dal runtime: sono ~500.000 righe).
/// Restituisce anche il file compresso, da salvare così com'è.
pub async fn fetch(client: &reqwest::Client) -> Result<(Vec<u8>, AsnTable), String> {
    let gz = fetch_bytes(client, URL).await?;
    tokio::task::spawn_blocking(move || AsnTable::from_gz(&gz).map(|t| (gz, t)))
        .await
        .map_err(|e| e.to_string())?
}
