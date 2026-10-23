//! Parser dei formati delle liste di proxy. Non vanno mai in panico: le righe
//! spazzatura (commenti, "error code: 502", IP o porte invalidi) vengono scartate.

use crate::countries::name_to_alpha2;
use crate::model::{Candidate, Endpoint, Format, Protocol};
use serde_json::Value;
use std::net::Ipv4Addr;

/// Interpreta il testo di un endpoint. I `sources` dei candidati restano vuoti.
pub fn parse(text: &str, ep: &Endpoint) -> Vec<Candidate> {
    let mut list = parse_format(text, ep);
    if ep.https {
        list.iter_mut().filter(|c| c.protocol == Protocol::Http).for_each(|c| c.https = true);
    }
    list
}

fn parse_format(text: &str, ep: &Endpoint) -> Vec<Candidate> {
    match ep.format {
        Format::Monosans => parse_monosans(text),
        Format::Proxifly => parse_proxifly(text),
        Format::Vakhov => parse_vakhov(text),
        Format::IpPort => lines(text).filter_map(|l| parse_ip_port(l, ep.protocol)).collect(),
        Format::IpPortCountry => lines(text).filter_map(|l| parse_ip_port_country(l, ep.protocol)).collect(),
        Format::ProtoUrl => lines(text).filter_map(parse_proto_url).collect(),
        Format::Auto => lines(text)
            .filter_map(|l| {
                if l.contains("://") {
                    parse_proto_url(l)
                } else {
                    parse_ip_port(l, ep.protocol)
                }
            })
            .collect(),
    }
}

fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("//"))
}

fn ip(s: &str) -> Option<Ipv4Addr> {
    s.trim().parse::<Ipv4Addr>().ok().filter(|a| !a.is_unspecified())
}

fn port(s: &str) -> Option<u16> {
    s.trim().parse::<u16>().ok().filter(|p| *p != 0)
}

fn port_value(v: &Value) -> Option<u16> {
    match v {
        Value::Number(n) => n.as_u64().filter(|p| (1..=65535).contains(p)).map(|p| p as u16),
        Value::String(s) => port(s),
        _ => None,
    }
}

fn country(s: &str) -> String {
    name_to_alpha2(s.trim())
}

fn mk(protocol: Protocol, ip_s: &str, port_s: &str) -> Option<Candidate> {
    Some(Candidate::new(protocol, ip(ip_s)?, port(port_s)?))
}

fn parse_ip_port(line: &str, proto: Option<Protocol>) -> Option<Candidate> {
    let mut it = line.split(':');
    let (a, p) = (it.next()?, it.next()?);
    mk(proto.unwrap_or(Protocol::Http), a, p)
}

fn parse_ip_port_country(line: &str, proto: Option<Protocol>) -> Option<Candidate> {
    let mut it = line.splitn(3, ':');
    let (a, p) = (it.next()?, it.next()?);
    let mut c = mk(proto.unwrap_or(Protocol::Http), a, p)?;
    c.country = it.next().map(country).unwrap_or_default();
    Some(c)
}

fn parse_proto_url(line: &str) -> Option<Candidate> {
    let (scheme, rest) = line.split_once("://")?;
    let proto = Protocol::parse(scheme)?;
    // ignora eventuali credenziali o percorsi
    let rest = rest.rsplit('@').next()?.split('/').next()?;
    let (a, p) = rest.split_once(':')?;
    let mut c = mk(proto, a, p)?;
    c.https = Protocol::declares_https(scheme);
    Some(c)
}

fn json_array(text: &str) -> Vec<Value> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Array(a)) => a,
        _ => Vec::new(),
    }
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    let mut cur = v;
    for k in path {
        match cur.get(k) {
            Some(n) => cur = n,
            None => return "",
        }
    }
    cur.as_str().unwrap_or("")
}

fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn parse_monosans(text: &str) -> Vec<Candidate> {
    json_array(text)
        .iter()
        .filter_map(|r| {
            let name = r.get("protocol")?.as_str()?;
            let mut c = Candidate::new(Protocol::parse(name)?, ip(r.get("host")?.as_str()?)?, port_value(r.get("port")?)?);
            c.https = Protocol::declares_https(name);
            c.country = country(str_at(r, &["geolocation", "country", "iso_code"]));
            c.latency_ms = num(r.get("timeout")).filter(|t| *t >= 0.0).map(|t| (t * 1000.0).round() as u32);
            Some(c)
        })
        .collect()
}

fn parse_proxifly(text: &str) -> Vec<Candidate> {
    json_array(text)
        .iter()
        .filter_map(|r| {
            let name = r.get("protocol")?.as_str()?;
            let mut c = Candidate::new(Protocol::parse(name)?, ip(r.get("ip")?.as_str()?)?, port_value(r.get("port")?)?);
            c.https = Protocol::declares_https(name);
            c.country = country(str_at(r, &["geolocation", "country"]));
            c.uptime = num(r.get("score")).map(|s| (s * 100.0).round().clamp(0.0, 100.0) as u8);
            Some(c)
        })
        .collect()
}

fn parse_vakhov(text: &str) -> Vec<Candidate> {
    let mut out = Vec::new();
    for r in json_array(text) {
        let (Some(addr), Some(p)) = (r.get("ip").and_then(|v| v.as_str()).and_then(ip), r.get("port").and_then(port_value))
        else {
            continue;
        };
        let flag = |k: &str| matches!(r.get(k), Some(Value::String(s)) if s == "1") || r.get(k).and_then(|v| v.as_u64()) == Some(1);
        let up = num(r.get("checks_up")).unwrap_or(0.0);
        let down = num(r.get("checks_down")).unwrap_or(0.0);
        let base = |proto| {
            let mut c = Candidate::new(proto, addr, p);
            c.country = country(str_at(&r, &["country_code"]));
            c.latency_ms = num(r.get("delay")).filter(|d| *d > 0.0).map(|d| d.round() as u32);
            c.uptime = (up + down > 0.0).then(|| (up / (up + down) * 100.0).round() as u8);
            c
        };
        if flag("socks5") {
            out.push(base(Protocol::Socks5));
        }
        if flag("socks4") {
            out.push(base(Protocol::Socks4));
        }
        if flag("http") || flag("ssl") {
            let mut c = base(Protocol::Http);
            c.https = flag("ssl");
            out.push(c);
        }
    }
    out
}
