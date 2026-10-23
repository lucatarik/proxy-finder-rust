//! Tipi condivisi tra libreria e interfaccia.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Http,
    Socks4,
    Socks5,
}

impl Protocol {
    pub const ALL: [Protocol; 3] = [Protocol::Http, Protocol::Socks4, Protocol::Socks5];

    /// true se il nome usato dalla lista è "https" (che `parse` riduce a http).
    pub fn declares_https(s: &str) -> bool {
        s.trim().eq_ignore_ascii_case("https")
    }

    /// Normalizza i nomi usati dalle liste: https -> http, socks5h -> socks5, ecc.
    pub fn parse(s: &str) -> Option<Protocol> {
        match s.trim().to_ascii_lowercase().as_str() {
            "http" | "https" => Some(Protocol::Http),
            "socks4" | "socks4a" => Some(Protocol::Socks4),
            "socks5" | "socks5h" | "socks" => Some(Protocol::Socks5),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::Http => "http",
            Protocol::Socks4 => "socks4",
            Protocol::Socks5 => "socks5",
        }
    }
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Format {
    Monosans,
    Proxifly,
    Vakhov,
    IpPort,
    IpPortCountry,
    ProtoUrl,
    Auto,
}

impl Format {
    pub const ALL: [Format; 7] = [
        Format::Auto,
        Format::IpPort,
        Format::IpPortCountry,
        Format::ProtoUrl,
        Format::Monosans,
        Format::Proxifly,
        Format::Vakhov,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Format::Monosans => "JSON monosans",
            Format::Proxifly => "JSON proxifly",
            Format::Vakhov => "JSON vakhov",
            Format::IpPort => "ip:porta",
            Format::IpPortCountry => "ip:porta:paese",
            Format::ProtoUrl => "proto://ip:porta",
            Format::Auto => "Auto",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Endpoint {
    pub url: String,
    pub format: Format,
    pub protocol: Option<Protocol>,
    /// La lista dichiara proxy http che reggono https (es. il file `https.txt`).
    #[serde(default)]
    pub https: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SourceStats {
    pub tested: u64,
    pub good: u64,
    pub dead: u64,
    /// Somma degli score dei proxy buoni.
    pub score_sum: f64,
    /// Unix secs.
    pub last_download: Option<i64>,
    pub last_count: u64,
    pub last_error: Option<String>,
}

/// Esiti della prima prova di un gruppo di candidati (stesso ASN o stessa /24).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroupStats {
    pub tested: u64,
    pub good: u64,
    /// Somma degli score dei proxy buoni.
    pub score_sum: f64,
}

/// Rating per rete: esiti raggruppati per ASN (numero) e per /24 (IP senza l'ultimo ottetto,
/// `u32::from(ip) >> 8`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetStats {
    pub asn: BTreeMap<u32, GroupStats>,
    pub net24: BTreeMap<u32, GroupStats>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub endpoints: Vec<Endpoint>,
    pub enabled: bool,
    pub builtin: bool,
    /// Rating iniziale in [0,1].
    pub prior: f64,
    #[serde(default)]
    pub stats: SourceStats,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub protocol: Protocol,
    pub ip: Ipv4Addr,
    pub port: u16,
    /// Alpha-2 maiuscolo o "".
    pub country: String,
    /// Latenza dichiarata dalla sorgente.
    pub latency_ms: Option<u32>,
    /// 0..100 dichiarato dalla sorgente.
    pub uptime: Option<u8>,
    /// Tutte le sorgenti che elencano questo proxy.
    pub sources: Vec<String>,
    /// Una sorgente lo dichiara "https" (http che regge CONNECT): solo un indizio, il test decide.
    pub https: bool,
}

impl Candidate {
    pub fn new(protocol: Protocol, ip: Ipv4Addr, port: u16) -> Candidate {
        Candidate {
            protocol,
            ip,
            port,
            country: String::new(),
            latency_ms: None,
            uptime: None,
            sources: Vec::new(),
            https: false,
        }
    }

    pub fn key(&self) -> String {
        proxy_key(self.protocol, self.ip, self.port)
    }

    /// Promettente per una connessione cifrata: socks5, o http dichiarato https da una sorgente.
    pub fn tls_promising(&self) -> bool {
        self.protocol == Protocol::Socks5 || self.https
    }

    /// Quanti metadati porta la voce: serve a scegliere tra duplicati.
    pub fn richness(&self) -> u8 {
        (!self.country.is_empty()) as u8 + self.latency_ms.is_some() as u8 + self.uptime.is_some() as u8
    }
}

pub fn proxy_key(protocol: Protocol, ip: Ipv4Addr, port: u16) -> String {
    format!("{}://{}:{}", protocol.as_str(), ip, port)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoodProxy {
    pub protocol: Protocol,
    pub ip: Ipv4Addr,
    pub port: u16,
    /// Media dei due passaggi (o solo A se "solo http").
    pub latency_ms: u32,
    pub exit_ip: String,
    pub country: String,
    /// false = "SOLO HTTP": non regge CONNECT.
    pub https: bool,
    pub score: f64,
    pub sources: Vec<String>,
    pub added_at: i64,
    pub checked_at: i64,
    /// false dopo un ricontrollo fallito ("non risponde").
    pub alive: bool,
    /// Velocità di download dell'ultimo speedtest, in byte/s (0 = scartato o fallito,
    /// None = mai misurata).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_bps: Option<u64>,
}

impl GoodProxy {
    pub fn key(&self) -> String {
        proxy_key(self.protocol, self.ip, self.port)
    }

    pub fn url(&self) -> String {
        self.key()
    }

    pub fn as_candidate(&self) -> Candidate {
        Candidate {
            protocol: self.protocol,
            ip: self.ip,
            port: self.port,
            country: self.country.clone(),
            latency_ms: Some(self.latency_ms),
            uptime: None,
            sources: self.sources.clone(),
            https: self.https,
        }
    }
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
