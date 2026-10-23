mod common;

use proxy_finder::model::*;
use proxy_finder::parse::parse;
use proxy_finder::rating::*;
use proxy_finder::relay;
use proxy_finder::sources::{custom_source, default_sources, merge, slug};
use proxy_finder::store::{restore_default_sources, Settings, Store};
use proxy_finder::tester::{parse_echo, test_one, test_pool, Outcome, TestConfig};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

fn ep(format: Format, protocol: Option<Protocol>) -> Endpoint {
    Endpoint { url: "x".into(), format, protocol, https: false }
}

fn cand(ip: &str, port: u16, src: &str) -> Candidate {
    let mut c = Candidate::new(Protocol::Http, ip.parse().unwrap(), port);
    c.sources = vec![src.into()];
    c
}

// ---------- A2: parser ----------

#[test]
fn parse_ip_port_skips_junk() {
    let text = "error code: 502\r\n# commento\n1.2.3.4:8080\r\n  5.6.7.8:3128  \n9.9.9.9:0\n1.1.1.1:70000\n300.1.1.1:80\nfoo\n\n";
    let l = parse(text, &ep(Format::IpPort, Some(Protocol::Socks5)));
    assert_eq!(l.len(), 2);
    assert_eq!(l[0].key(), "socks5://1.2.3.4:8080");
    assert_eq!(l[1].key(), "socks5://5.6.7.8:3128");
}

#[test]
fn parse_ip_port_country_maps_names() {
    let l = parse("1.2.3.4:80:Germany\n5.6.7.8:81:US\n9.9.9.9:82\n", &ep(Format::IpPortCountry, Some(Protocol::Http)));
    assert_eq!(l.len(), 3);
    assert_eq!(l[0].country, "DE");
    assert_eq!(l[1].country, "US");
    assert_eq!(l[2].country, "");
}

#[test]
fn parse_proto_url_and_auto() {
    let t = "socks5://1.2.3.4:1080\nhttps://5.6.7.8:443\nftp://1.1.1.1:21\nsocks4://bad:1\n";
    let l = parse(t, &ep(Format::ProtoUrl, None));
    assert_eq!(l.iter().map(|c| c.key()).collect::<Vec<_>>(), vec!["socks5://1.2.3.4:1080", "http://5.6.7.8:443"]);
    let a = parse("socks4://2.2.2.2:4145\n3.3.3.3:8080\n", &ep(Format::Auto, Some(Protocol::Socks5)));
    assert_eq!(a.iter().map(|c| c.key()).collect::<Vec<_>>(), vec!["socks4://2.2.2.2:4145", "socks5://3.3.3.3:8080"]);
}

#[test]
fn parse_json_formats() {
    let m = r#"[{"host":"1.2.3.4","port":8080,"protocol":"http","timeout":0.25,"geolocation":{"country":{"iso_code":"IT"}}},
               {"host":"x","port":1,"protocol":"http"}]"#;
    let l = parse(m, &ep(Format::Monosans, None));
    assert_eq!(l.len(), 1);
    assert_eq!(l[0].latency_ms, Some(250));
    assert_eq!(l[0].country, "IT");

    let p = r#"[{"ip":"5.6.7.8","port":1080,"protocol":"socks5","score":0.9,"geolocation":{"country":"FR"}}]"#;
    let l = parse(p, &ep(Format::Proxifly, None));
    assert_eq!(l[0].key(), "socks5://5.6.7.8:1080");
    assert_eq!(l[0].uptime, Some(90));
    assert_eq!(l[0].country, "FR");

    let v = r#"[{"ip":"9.8.7.6","port":"3128","http":"1","ssl":"0","socks4":"1","socks5":"0","country_code":"de","delay":"120","checks_up":"3","checks_down":"1"}]"#;
    let l = parse(v, &ep(Format::Vakhov, None));
    assert_eq!(l.len(), 2);
    assert!(l.iter().all(|c| c.uptime == Some(75) && c.latency_ms == Some(120) && c.country == "DE"));

    assert!(parse("not json", &ep(Format::Monosans, None)).is_empty());
    assert!(parse("{\"a\":1}", &ep(Format::Proxifly, None)).is_empty());
}

#[test]
fn parse_echo_variants() {
    assert_eq!(parse_echo(r#"{"ip":"1.2.3.4","nazione":"IT"}"#), Some(("1.2.3.4".into(), "IT".into())));
    assert_eq!(parse_echo(r#"{"origin":"5.6.7.8, 9.9.9.9"}"#), Some(("5.6.7.8".into(), "".into())));
    assert_eq!(parse_echo("fl=1\nip=8.8.4.4\nts=1\n"), Some(("8.8.4.4".into(), "".into())));
    assert_eq!(parse_echo("  4.4.4.4\n"), Some(("4.4.4.4".into(), "".into())));
    assert_eq!(parse_echo("<html>nope</html>"), None);
}

// ---------- A3: merge ----------

#[test]
fn merge_dedups_sources_and_interleaves() {
    let a = vec![cand("1.1.1.1", 1, "a"), cand("1.1.1.2", 1, "a"), cand("9.9.9.9", 9, "a")];
    let mut dup = cand("9.9.9.9", 9, "b");
    dup.country = "NL".into();
    let b = vec![dup, cand("2.2.2.1", 1, "b")];
    let l = merge(vec![a, b]);
    let keys: Vec<_> = l.iter().map(|c| c.ip.to_string()).collect();
    assert_eq!(keys, vec!["1.1.1.1", "9.9.9.9", "1.1.1.2", "2.2.2.1"]);
    let nine = l.iter().find(|c| c.ip == Ipv4Addr::new(9, 9, 9, 9)).unwrap();
    assert_eq!(nine.sources, vec!["b".to_string(), "a".to_string()]);
    assert_eq!(nine.country, "NL");
}

#[test]
fn merge_has_no_cap() {
    let a: Vec<_> = (0..60_000u32).map(|i| cand(&std::net::Ipv4Addr::from(0x0100_0000 + i).to_string(), 1, "a")).collect();
    let b: Vec<_> = (0..10u32).map(|i| cand(&format!("2.2.2.{i}"), 1, "b")).collect();
    let l = merge(vec![a, b]);
    assert_eq!(l.len(), 60_010);
}

// ---------- A4: rating ----------

fn source(id: &str) -> Source {
    custom_source(id, "http://x", Format::Auto, None, &[]).tap_id(id)
}

trait TapId {
    fn tap_id(self, id: &str) -> Self;
}
impl TapId for Source {
    fn tap_id(mut self, id: &str) -> Self {
        self.id = id.into();
        self.prior = 0.3;
        self
    }
}

fn good(score_latency: u32, https: bool, sources: &[&str]) -> Outcome {
    Outcome::Good(GoodProxy {
        protocol: Protocol::Http,
        ip: Ipv4Addr::new(1, 1, 1, 1),
        port: 1,
        latency_ms: score_latency,
        exit_ip: "1.1.1.1".into(),
        country: String::new(),
        https,
        score: proxy_score(score_latency, https),
        sources: sources.iter().map(|s| s.to_string()).collect(),
        added_at: 0,
        checked_at: 0,
        alive: true,
        speed_bps: None,
    })
}

#[test]
fn proxy_score_formula() {
    assert!((proxy_score(0, true) - 1.0).abs() < 1e-9);
    assert!((proxy_score(2500, true) - 0.5).abs() < 1e-9);
    assert!((proxy_score(2500, false) - 0.25).abs() < 1e-9);
    assert!((proxy_score(100_000, true) - 0.05).abs() < 1e-9);
}

#[test]
fn rating_moves_with_goodness() {
    let c = cand("1.1.1.1", 1, "fast");
    let mut fast = vec![source("fast")];
    let mut slow = vec![source("fast")];
    let base = source_rating(&fast[0]);
    assert!((base - 0.3).abs() < 1e-9);
    apply_outcome(&mut fast, &c, &good(100, true, &["fast"]));
    apply_outcome(&mut slow, &c, &good(4500, false, &["fast"]));
    assert!(source_rating(&fast[0]) > base);
    assert!(source_rating(&fast[0]) > source_rating(&slow[0]));
    assert!(source_rating(&slow[0]) < base, "un buono molto lento vale meno del prior");

    let mut dead = vec![source("fast")];
    apply_outcome(&mut dead, &c, &Outcome::Dead { reason: "x".into() });
    assert!(source_rating(&dead[0]) < base);
    assert_eq!(dead[0].stats.dead, 1);

    for _ in 0..10_000 {
        apply_outcome(&mut dead, &c, &Outcome::Dead { reason: "x".into() });
        apply_outcome(&mut fast, &c, &good(0, true, &["fast"]));
    }
    let (d, f) = (source_rating(&dead[0]), source_rating(&fast[0]));
    assert!((0.0..=1.0).contains(&d) && (0.0..=1.0).contains(&f));
    assert!(d < 0.01 && f > 0.99);
}

#[test]
fn rating_credits_all_sources_and_ignores_cancelled() {
    let mut srcs = vec![source("a"), source("b"), source("c")];
    let mut c = cand("1.1.1.1", 1, "a");
    c.sources.push("b".into());
    apply_outcome(&mut srcs, &c, &good(500, true, &["a", "b"]));
    assert_eq!((srcs[0].stats.good, srcs[1].stats.good, srcs[2].stats.tested), (1, 1, 0));
    let before = srcs.clone();
    apply_outcome(&mut srcs, &c, &Outcome::Cancelled);
    assert_eq!(before, srcs);
}

#[test]
fn priority_order_keeps_interleave_within_bands() {
    let mut r = HashMap::new();
    r.insert("a".to_string(), 0.40);
    r.insert("b".to_string(), 0.39);
    r.insert("z".to_string(), 0.05);
    // ordine di unione: a, b, z, a, b
    let l = vec![cand("1.0.0.1", 1, "a"), cand("2.0.0.1", 1, "b"), cand("9.0.0.1", 1, "z"), cand("1.0.0.2", 1, "a"), cand("2.0.0.2", 1, "b")];
    let o: Vec<_> = order_by_priority(l, &r, &NetLift::none()).iter().map(|c| c.sources[0].clone()).collect();
    assert_eq!(o, vec!["a", "b", "a", "b", "z"]);
}

// ---------- rating per rete (ASN e /24) ----------

const ASN_TSV: &str = "10.0.0.0\t10.0.255.255\t200\tIT\tBUONA\n\
1.0.0.0\t1.0.0.255\t100\tUS\tPESSIMA NET\n\
garbage line\n\
2.0.0.0\t1.0.0.0\t5\tXX\tROVESCIATA\n\
20.0.0.0\t20.0.0.255\t0\tNone\tNot routed\n";

#[test]
fn asn_table_parse_and_lookup() {
    use proxy_finder::asn::AsnTable;
    let t = AsnTable::parse(ASN_TSV);
    assert_eq!(t.len(), 3, "righe malformate o rovesciate saltate");
    let ip = |s: &str| s.parse::<Ipv4Addr>().unwrap();
    assert_eq!(t.lookup(ip("1.0.0.0")), Some(100));
    assert_eq!(t.lookup(ip("1.0.0.255")), Some(100));
    assert_eq!(t.lookup(ip("10.0.7.1")), Some(200));
    assert_eq!(t.lookup(ip("20.0.0.9")), Some(0));
    assert_eq!(t.lookup(ip("1.0.1.0")), None);
    assert_eq!(t.lookup(ip("0.0.0.1")), None);
    assert_eq!(t.lookup(ip("255.255.255.255")), None);
    assert_eq!(t.name(100), "PESSIMA NET");

    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(ASN_TSV.as_bytes()).unwrap();
    let gz = enc.finish().unwrap();
    assert_eq!(AsnTable::from_gz(&gz).unwrap().lookup(ip("10.0.0.1")), Some(200));
    assert!(AsnTable::from_gz(b"non gzip").is_err());
    assert!(AsnTable::from_gz(&flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()).finish().unwrap()).is_err());
}

fn gz_asn() -> Vec<u8> {
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(ASN_TSV.as_bytes()).unwrap();
    enc.finish().unwrap()
}

/// 200 prove: la rete 10.0.0.0/16 (AS200) dà buoni veloci, 1.0.0.0/24 (AS100) solo morti.
fn net_stats(t: &proxy_finder::asn::AsnTable) -> NetStats {
    let mut n = NetStats::default();
    let dead = Outcome::Dead { reason: "x".into() };
    for i in 0..100u8 {
        let good_ip = Ipv4Addr::new(10, 0, i / 25, i);
        let o = if i % 2 == 0 { good(200, true, &["a"]) } else { dead.clone() };
        apply_net_outcome(&mut n, t.lookup(good_ip), good_ip, &o);
        let bad_ip = Ipv4Addr::new(1, 0, 0, i);
        apply_net_outcome(&mut n, t.lookup(bad_ip), bad_ip, &dead);
    }
    n
}

#[test]
fn net_lift_ranks_networks() {
    let t = proxy_finder::asn::AsnTable::parse(ASN_TSV);
    let n = net_stats(&t);
    assert_eq!((n.asn[&200].tested, n.asn[&200].good, n.asn[&100].good), (100, 50, 0));
    assert_eq!(n.net24.len(), 5, "4 /24 in 10.0.x e 1.0.0");

    let lift = NetLift::new(&n, Some(&t));
    let ip = |s: &str| s.parse::<Ipv4Addr>().unwrap();
    let (good_net, bad_net, unknown) = (lift.of(ip("10.0.1.9")), lift.of(ip("1.0.0.77")), lift.of(ip("99.9.9.9")));
    assert!(good_net > 1.5, "{good_net}");
    assert!(bad_net < 0.2, "{bad_net}");
    assert!((unknown - 1.0).abs() < 1e-9, "rete mai vista = media");
    // /24 nuova dentro un ASN noto: eredita il rating dell'ASN
    assert!(lift.of(ip("10.0.200.1")) > 1.5);
    // senza tabella ASN contano solo le /24
    let only24 = NetLift::new(&n, None);
    assert!(only24.of(ip("10.0.200.1")) - 1.0 < 1e-9);
    assert!(only24.of(ip("1.0.0.1")) < 0.4);

    // sotto NET_MIN_TESTED prove nessun effetto; gli annullati non contano
    let mut few = NetStats::default();
    apply_net_outcome(&mut few, Some(100), ip("1.0.0.1"), &good(100, true, &["a"]));
    apply_net_outcome(&mut few, Some(100), ip("1.0.0.1"), &Outcome::Cancelled);
    assert_eq!(few.net24.values().map(|g| g.tested).sum::<u64>(), 1);
    assert_eq!(NetLift::new(&few, Some(&t)).of(ip("1.0.0.1")), 1.0);
    assert!(NetLift::none().of(ip("1.0.0.1")) == 1.0);
    assert_eq!(net24_label(net24(ip("45.74.31.46"))), "45.74.31.0/24");
}

#[test]
fn priority_order_uses_net_lift() {
    let t = proxy_finder::asn::AsnTable::parse(ASN_TSV);
    let n = net_stats(&t);
    let mut r = HashMap::new();
    r.insert("a".to_string(), 0.30);
    r.insert("b".to_string(), 0.60);
    // stessa sorgente: la rete buona passa davanti; la sorgente migliore non basta a una rete pessima
    let l = vec![cand("1.0.0.200", 1, "b"), cand("50.0.0.1", 1, "a"), cand("10.0.3.200", 1, "a")];
    let o: Vec<String> = order_by_priority(l.clone(), &r, &NetLift::new(&n, Some(&t))).iter().map(|c| c.ip.to_string()).collect();
    assert_eq!(o, vec!["10.0.3.200", "50.0.0.1", "1.0.0.200"]);
    // senza rating per rete conta solo la sorgente
    let o: Vec<String> = order_by_priority(l, &r, &NetLift::none()).iter().map(|c| c.ip.to_string()).collect();
    assert_eq!(o[0], "1.0.0.200");
}

#[test]
fn state_net_rating_only_from_test_and_persisted() {
    use proxy_finder::job::JobKind;
    use proxy_finder::state::{State, TestCount};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().to_path_buf()).unwrap();
    let mut st = State::load(Some(store.clone()));
    assert!(st.asn.is_none() && st.asn_stale());
    let errs = st.set_asn(Arc::new(proxy_finder::asn::AsnTable::from_gz(&gz_asn()).unwrap()), &gz_asn());
    assert!(errs.is_empty() && !st.asn_stale());

    let c = cand("10.0.0.5", 80, "a");
    let mut count = TestCount::default();
    for kind in [JobKind::Recheck, JobKind::Connect] {
        st.apply_tested(kind, &c, &good(100, true, &["a"]), &mut count);
    }
    assert_eq!(st.net, NetStats::default(), "ricontrollo e connessione non toccano il rating per rete");
    st.apply_tested(JobKind::Test, &c, &good(100, true, &["a"]), &mut count);
    assert_eq!((st.net.asn[&200].good, st.net.net24[&net24(c.ip)].tested), (1, 1));
    assert!(st.finish(JobKind::Test).is_empty());

    let st2 = State::load(Some(store));
    assert_eq!(st2.net, st.net);
    assert_eq!(st2.asn_of(c.ip), Some(200));

    // tabella corrotta: messa da parte, il resto funziona
    std::fs::write(dir.path().join(proxy_finder::asn::FILE), b"rotto").unwrap();
    let st3 = State::load(Some(Store::at(dir.path().to_path_buf()).unwrap()));
    assert!(st3.asn.is_none() && st3.asn_stale());
    assert!(dir.path().join(format!("{}.bad", proxy_finder::asn::FILE)).exists());
}

#[test]
fn priority_uses_rating_and_metadata() {
    let mut r = HashMap::new();
    r.insert("hi".to_string(), 0.9);
    r.insert("lo".to_string(), 0.1);
    let mut a = cand("1.1.1.1", 1, "hi");
    let b = cand("1.1.1.2", 1, "lo");
    assert!(candidate_priority(&a, &r) > candidate_priority(&b, &r));
    a.latency_ms = Some(100);
    let mut a2 = a.clone();
    a2.latency_ms = Some(4000);
    assert!(candidate_priority(&a, &r) > candidate_priority(&a2, &r));
    let mut both = b.clone();
    both.sources.push("hi".into());
    assert!(candidate_priority(&both, &r) > candidate_priority(&b, &r));
}

// ---------- sorgenti ----------

#[test]
fn defaults_are_well_formed() {
    let d = default_sources();
    assert!(d.len() >= 23);
    let mut ids: Vec<_> = d.iter().map(|s| s.id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), d.len(), "id duplicati");
    assert!(d.iter().all(|s| s.builtin && !s.endpoints.is_empty() && s.endpoints.iter().all(|e| e.url.starts_with("https://"))));
    assert_eq!(slug("Mia Lista!!", &d), "mia-lista");
    let mut with = d.clone();
    with.push(custom_source("Mia Lista", "http://x", Format::Auto, None, &d));
    assert_eq!(slug("mia lista", &with), "mia-lista-2");
}

// ---------- A5: tester ----------

fn cfg(echo: std::net::SocketAddr, https_port: u16) -> TestConfig {
    TestConfig {
        concurrency: 3,
        http_timeout: Duration::from_secs(3),
        https_timeout: Duration::from_secs(2),
        http_echo: vec![format!("127.0.0.1:{}/", echo.port())],
        https_echo: vec![format!("127.0.0.1:{https_port}/")],
        ..TestConfig::default()
    }
}

fn proxy_cand(port: u16) -> Candidate {
    let mut c = Candidate::new(Protocol::Http, Ipv4Addr::LOCALHOST, port);
    c.sources = vec!["a".into()];
    c.country = "XX".into();
    c
}

#[tokio::test]
async fn tester_good_http_only() {
    let echo = common::echo_server(r#"{"ip":"203.0.113.7","nazione":"DE"}"#).await;
    let proxy = common::http_proxy().await;
    let closed = common::closed_port().await;
    let o = test_one(&proxy_cand(proxy.port()), "198.51.100.1", &cfg(echo, closed), &CancellationToken::new(), 0).await;
    match o {
        Outcome::Good(g) => {
            assert_eq!(g.exit_ip, "203.0.113.7");
            assert_eq!(g.country, "DE");
            assert!(!g.https);
            assert!((g.score - proxy_score(g.latency_ms, false)).abs() < 1e-9);
            assert_eq!(g.sources, vec!["a".to_string()]);
        }
        other => panic!("atteso Good, ottenuto {other:?}"),
    }
}

fn socks_cand(port: u16) -> Candidate {
    let mut c = proxy_cand(port);
    c.protocol = Protocol::Socks5;
    c
}

#[tokio::test]
async fn tester_socks5_good_http_only() {
    let echo = common::echo_server(r#"{"ip":"203.0.113.7","nazione":"DE"}"#).await;
    let (proxy, _) = common::socks5_proxy(0).await;
    let closed = common::closed_port().await;
    let o = test_one(&socks_cand(proxy.port()), "198.51.100.1", &cfg(echo, closed), &CancellationToken::new(), 0).await;
    assert!(matches!(&o, Outcome::Good(g) if !g.https && g.exit_ip == "203.0.113.7"), "{o:?}");
}

#[tokio::test]
async fn tester_retries_in_https_when_plain_http_is_refused() {
    // il SOCKS rifiuta la porta dell'eco http: il test deve riprovare subito in https
    let echo = common::echo_server(r#"{"ip":"203.0.113.7"}"#).await;
    let https_side = common::echo_server("niente tls").await;
    let (proxy, seen) = common::socks5_proxy(echo.port()).await;
    let o = test_one(&socks_cand(proxy.port()), "198.51.100.1", &cfg(echo, https_side.port()), &CancellationToken::new(), 0).await;
    let Outcome::Dead { reason } = o else { panic!("atteso Dead, ottenuto {o:?}") };
    assert!(reason.starts_with("http: ") && reason.contains("; https: "), "{reason}");
    assert!(seen.lock().unwrap().contains(&https_side.port()), "nessun tunnel verso la porta https");
}

#[tokio::test]
async fn tester_unreachable_proxy_skips_https() {
    let echo = common::echo_server(r#"{"ip":"203.0.113.7"}"#).await;
    let closed = common::closed_port().await;
    for c in [proxy_cand(closed), socks_cand(closed)] {
        let o = test_one(&c, "198.51.100.1", &cfg(echo, closed), &CancellationToken::new(), 0).await;
        let Outcome::Dead { reason } = o else { panic!("atteso Dead, ottenuto {o:?}") };
        assert!(!reason.contains("https"), "{}: {reason}", c.protocol.as_str());
    }
}

#[tokio::test]
async fn tester_negative_cases() {
    let echo = common::echo_server(r#"{"ip":"203.0.113.7","nazione":"DE"}"#).await;
    let proxy = common::http_proxy().await;
    let closed = common::closed_port().await;
    let c = cfg(echo, closed);
    let cancel = CancellationToken::new();

    let strict = TestConfig { drop_http_only: true, ..c.clone() };
    assert!(matches!(test_one(&proxy_cand(proxy.port()), "198.51.100.1", &strict, &cancel, 0).await, Outcome::Dead { .. }));

    let dead_port = common::closed_port().await;
    assert!(matches!(test_one(&proxy_cand(dead_port), "198.51.100.1", &c, &cancel, 0).await, Outcome::Dead { .. }));

    // l'eco vede il nostro IP reale: proxy trasparente
    assert!(matches!(test_one(&proxy_cand(proxy.port()), "203.0.113.7", &c, &cancel, 0).await, Outcome::Dead { .. }));

    let gone = CancellationToken::new();
    gone.cancel();
    assert_eq!(test_one(&proxy_cand(proxy.port()), "198.51.100.1", &c, &gone, 0).await, Outcome::Cancelled);
}

#[tokio::test]
async fn tester_pool_reports_every_candidate() {
    let echo = common::echo_server("203.0.113.9").await;
    let proxy = common::http_proxy().await;
    let closed = common::closed_port().await;
    let cands: Vec<_> = (0..10).map(|i| if i % 2 == 0 { proxy_cand(proxy.port()) } else { proxy_cand(closed) }).collect();
    let mut good = 0;
    let mut total = 0;
    test_pool(cands, "198.51.100.1".into(), cfg(echo, closed), CancellationToken::new(), |_, o| {
        total += 1;
        if matches!(o, Outcome::Good(_)) {
            good += 1;
        }
    })
    .await;
    assert_eq!((total, good), (10, 5));
}

#[tokio::test]
async fn tester_pool_cancel_is_immediate() {
    // eco che non risponde mai: le prove restano appese finché non si annulla
    let hang = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hang_addr = hang.local_addr().unwrap();
    tokio::spawn(async move {
        let mut keep = Vec::new();
        while let Ok((s, _)) = hang.accept().await {
            keep.push(s);
        }
    });
    let proxy = common::http_proxy().await;
    let mut c = cfg(hang_addr, hang_addr.port());
    c.http_timeout = Duration::from_secs(30);
    let cancel = CancellationToken::new();
    let cancel2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        cancel2.cancel();
    });
    let t0 = std::time::Instant::now();
    let mut cancelled = 0;
    let cands: Vec<_> = (0..6).map(|_| proxy_cand(proxy.port())).collect();
    test_pool(cands, "198.51.100.1".into(), c, cancel, |_, o| {
        if *o == Outcome::Cancelled {
            cancelled += 1;
        }
    })
    .await;
    assert!(t0.elapsed() < Duration::from_secs(3));
    assert_eq!(cancelled, 6);
}

// ---------- A6: store ----------

#[test]
fn store_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let st = Store::at(dir.path().to_path_buf()).unwrap();
    assert_eq!(st.load_sources(), default_sources());
    assert!(st.load_proxies().is_empty());

    let mut c = cand("1.2.3.4", 8080, "a");
    c.latency_ms = Some(10);
    c.sources.push("b".into());
    st.save_candidates(&[c.clone()]).unwrap();
    assert_eq!(st.load_candidates(), vec![c]);
    let raw = std::fs::read_to_string(dir.path().join("candidates.json")).unwrap();
    assert_eq!(raw, r#"[["http","1.2.3.4",8080,"",10,null,["a","b"],false]]"#);

    let g = match good(300, true, &["a"]) {
        Outcome::Good(g) => g,
        _ => unreachable!(),
    };
    st.save_proxies(std::slice::from_ref(&g)).unwrap();
    assert_eq!(st.load_proxies(), vec![g]);

    let h: std::collections::HashSet<String> = ["http://1.1.1.1:1".to_string()].into();
    st.save_history(&h).unwrap();
    assert_eq!(st.load_history(), h);

    let mut s = Settings { relay_port: 9999, ..Default::default() };
    s.test.http_timeout = Duration::from_millis(1234);
    st.save_settings(&s).unwrap();
    assert_eq!(st.load_settings(), s);
}

#[test]
fn store_corrupt_file_falls_back() {
    let dir = tempfile::tempdir().unwrap();
    let st = Store::at(dir.path().to_path_buf()).unwrap();
    std::fs::write(dir.path().join("proxies.json"), "{garbage").unwrap();
    std::fs::write(dir.path().join("settings.json"), "[1,2").unwrap();
    assert!(st.load_proxies().is_empty());
    assert_eq!(st.load_settings(), Settings::default());
    assert!(dir.path().join("proxies.json.bad").exists());
    assert!(dir.path().join("settings.json.bad").exists());
}

#[test]
fn custom_sources_persist_and_restore_keeps_them() {
    let dir = tempfile::tempdir().unwrap();
    let st = Store::at(dir.path().to_path_buf()).unwrap();
    let mut s = st.load_sources();
    let n = s.len();
    let custom = custom_source("Mia", "https://example.com/l.txt", Format::Auto, Some(Protocol::Socks5), &s);
    s.push(custom.clone());
    s.retain(|x| x.id != "speedx");
    s.iter_mut().find(|x| x.id == "monosans").unwrap().stats.tested = 42;
    st.save_sources(&s).unwrap();

    let mut loaded = st.load_sources();
    assert_eq!(loaded.len(), n);
    assert!(loaded.iter().any(|x| x.id == custom.id));
    assert!(!loaded.iter().any(|x| x.id == "speedx"));

    restore_default_sources(&mut loaded);
    assert_eq!(loaded.len(), n + 1);
    assert!(loaded.iter().any(|x| x.id == "speedx"));
    assert_eq!(loaded.iter().find(|x| x.id == "monosans").unwrap().stats.tested, 42);
    assert_eq!(loaded.last().unwrap().id, custom.id);
}

// ---------- A7: relay ----------

async fn start_relay(up: Option<GoodProxy>) -> std::net::SocketAddr {
    let l = relay::bind_relay("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let addr = l.local_addr().unwrap();
    let upstream = Arc::new(RwLock::new(up));
    tokio::spawn(relay::serve(l, upstream, CancellationToken::new()));
    addr
}

fn upstream(port: u16) -> GoodProxy {
    match good(100, true, &[]) {
        Outcome::Good(mut g) => {
            g.ip = Ipv4Addr::LOCALHOST;
            g.port = port;
            g
        }
        _ => unreachable!(),
    }
}

async fn roundtrip(relay: std::net::SocketAddr, req: String) -> String {
    let mut s = tokio::net::TcpStream::connect(relay).await.unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_string(&mut out)).await;
    out
}

#[tokio::test]
async fn relay_forwards_absolute_get() {
    let echo = common::echo_server(r#"{"ip":"203.0.113.7"}"#).await;
    let proxy = common::http_proxy().await;
    let r = start_relay(Some(upstream(proxy.port()))).await;
    let out = roundtrip(r, format!("GET http://{echo}/ HTTP/1.1\r\nHost: {echo}\r\nConnection: close\r\n\r\n")).await;
    assert!(out.starts_with("HTTP/1.1 200"), "{out}");
    assert!(out.contains("203.0.113.7"));
}

#[tokio::test]
async fn relay_connect_tunnel() {
    let echo = common::echo_server(r#"{"ip":"203.0.113.8"}"#).await;
    let proxy = common::http_proxy().await;
    let r = start_relay(Some(upstream(proxy.port()))).await;
    let mut s = tokio::net::TcpStream::connect(r).await.unwrap();
    s.write_all(format!("CONNECT {echo} HTTP/1.1\r\nHost: {echo}\r\n\r\n").as_bytes()).await.unwrap();
    let mut buf = vec![0u8; 39];
    s.read_exact(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf).starts_with("HTTP/1.1 200"));
    s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut out = String::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_string(&mut out)).await;
    assert!(out.contains("203.0.113.8"), "{out}");
}

#[tokio::test]
async fn relay_without_upstream_is_503() {
    let r = start_relay(None).await;
    let out = roundtrip(r, "GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n".into()).await;
    assert!(out.starts_with("HTTP/1.1 503"), "{out}");
}

#[tokio::test]
async fn relay_refuses_non_loopback() {
    assert!(relay::bind_relay("0.0.0.0:0".parse().unwrap()).await.is_err());
}

// ---------- cambio proxy di un relay già attivo ----------

fn gp(ip: &str) -> GoodProxy {
    GoodProxy {
        protocol: Protocol::Socks5,
        ip: ip.parse().unwrap(),
        port: 1080,
        latency_ms: 100,
        exit_ip: ip.into(),
        country: "IT".into(),
        https: true,
        score: 0.9,
        sources: vec![],
        added_at: 0,
        checked_at: 0,
        alive: true,
        speed_bps: None,
    }
}

#[tokio::test]
async fn relay_switch_from_another_instance() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().to_path_buf()).unwrap();
    let (a, b) = (gp("1.1.1.1"), gp("2.2.2.2"));

    // nessun relay annunciato: la porta occupata è di qualcun altro, errore subito
    assert!(relay::request_switch(&store, 8899, &b, Duration::from_secs(5)).await.is_err());

    let up: relay::Upstream = Arc::new(RwLock::new(Some(a.clone())));
    let cancel = CancellationToken::new();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(relay::watch_switch(store.clone(), 8899, up.clone(), cancel.clone(), move |old, new| {
        log.lock().unwrap().push((old.map(GoodProxy::key), new.key()));
    }));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(store.load_relay().unwrap().upstream.key(), a.key());

    let pid = relay::request_switch(&store, 8899, &b, Duration::from_secs(5)).await.unwrap();
    assert_eq!(pid, std::process::id());
    assert_eq!(up.read().unwrap().as_ref().unwrap().key(), b.key());
    assert_eq!(*seen.lock().unwrap(), vec![(Some(a.key()), b.key())]);

    // richiesta per un'altra porta: non è di questo relay
    assert!(relay::request_switch(&store, 9999, &a, Duration::from_millis(300)).await.is_err());
    assert_eq!(up.read().unwrap().as_ref().unwrap().key(), b.key());
    cancel.cancel();
}

// ---------- proxy dichiarati https e ordine della modalità auto ----------

#[test]
fn https_hints_from_lists() {
    let l = parse("https://1.1.1.1:8080\nhttp://2.2.2.2:80\nsocks5://3.3.3.3:1080\n", &ep(Format::ProtoUrl, None));
    assert_eq!(l.iter().map(|c| c.https).collect::<Vec<_>>(), vec![true, false, false]);

    // file "https.txt": l'endpoint dichiara https per i suoi proxy http
    let file = Endpoint { https: true, ..ep(Format::IpPort, Some(Protocol::Http)) };
    assert!(parse("4.4.4.4:3128\n", &file).iter().all(|c| c.https && c.protocol == Protocol::Http));

    let mono = r#"[{"protocol":"https","host":"5.5.5.5","port":443},{"protocol":"http","host":"6.6.6.6","port":80}]"#;
    let l = parse(mono, &ep(Format::Monosans, None));
    assert_eq!(l.iter().map(|c| (c.protocol, c.https)).collect::<Vec<_>>(), vec![(Protocol::Http, true), (Protocol::Http, false)]);

    // le predefinite con un file https lo dichiarano
    let d = default_sources();
    let hideip = d.iter().find(|s| s.id == "hideip").unwrap();
    assert!(hideip.endpoints.iter().any(|e| e.https && e.url.ends_with("/https.txt")));
    assert!(hideip.endpoints.iter().filter(|e| !e.url.ends_with("/https.txt")).all(|e| !e.https));
}

#[test]
fn merge_keeps_https_hint() {
    let plain = cand("7.7.7.7", 80, "a");
    let mut hinted = cand("7.7.7.7", 80, "b");
    hinted.https = true;
    let l = merge(vec![vec![plain], vec![hinted]]);
    assert_eq!(l.len(), 1);
    assert!(l[0].https);
}

#[test]
fn candidates_file_compat_and_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    // formato fino alla 0.1.6: 7 elementi, senza https
    std::fs::write(dir.path().join("candidates.json"), r#"[["socks5","1.2.3.4",1080,"IT",null,null,["a"]]]"#).unwrap();
    let st = Store::at(dir.path().to_path_buf()).unwrap();
    let l = st.load_candidates();
    assert_eq!(l.len(), 1);
    assert!(!l[0].https);
    assert!(!dir.path().join("candidates.json.bad").exists());

    let mut c = cand("9.9.9.9", 8080, "b");
    c.https = true;
    st.save_candidates(&[c.clone()]).unwrap();
    assert_eq!(st.load_candidates(), vec![c]);
}

#[test]
fn builtin_endpoints_follow_the_code() {
    let dir = tempfile::tempdir().unwrap();
    let st = Store::at(dir.path().to_path_buf()).unwrap();
    let mut s = default_sources();
    for x in s.iter_mut() {
        for e in x.endpoints.iter_mut() {
            e.https = false; // come un sources.json scritto da una versione precedente
        }
    }
    let custom = custom_source("Mia", "https://example.com/l.txt", Format::Auto, None, &s);
    s.push(custom.clone());
    st.save_sources(&s).unwrap();
    let loaded = st.load_sources();
    assert_eq!(loaded.iter().find(|x| x.id == "hideip").unwrap().endpoints, default_sources().iter().find(|x| x.id == "hideip").unwrap().endpoints);
    assert_eq!(loaded.last().unwrap().endpoints, custom.endpoints);
}

#[test]
fn tls_promising_candidates_first() {
    let mut st = proxy_finder::state::State::load(None);
    let mk = |ip: &str, p: Protocol, https: bool| {
        let mut c = cand(ip, 1, "a");
        c.protocol = p;
        c.https = https;
        c
    };
    st.candidates = vec![
        mk("1.0.0.1", Protocol::Http, false),
        mk("1.0.0.2", Protocol::Socks4, false),
        mk("1.0.0.3", Protocol::Socks5, false),
        mk("1.0.0.4", Protocol::Http, true),
        mk("1.0.0.5", Protocol::Http, false),
    ];
    st.settings.test_count = 3;
    let keys: Vec<String> = st.pick_candidates_tls_first().iter().map(|c| c.ip.to_string()).collect();
    assert_eq!(keys, vec!["1.0.0.3", "1.0.0.4", "1.0.0.1"]);
}

// ---------- speedtest ----------

#[test]
fn speed_saved_and_sorted() {
    use proxy_finder::store::SortBy;
    use proxy_finder::tester::Speed;
    let mut st = proxy_finder::state::State::load(None);
    let (a, b, c, mut d) = (gp("1.1.1.1"), gp("2.2.2.2"), gp("3.3.3.3"), gp("4.4.4.4"));
    d.alive = false;
    st.proxies = vec![a.clone(), b.clone(), c.clone(), d.clone()];
    st.apply_speed(&a.key(), &Ok(Speed { bytes: 1_000_000, secs: 2.0, complete: false }));
    st.apply_speed(&b.key(), &Ok(Speed { bytes: 2_000_000, secs: 1.0, complete: true }));
    st.apply_speed(&d.key(), &Err("più lento del più veloce".into()));
    assert_eq!(st.proxies[0].speed_bps, Some(500_000));
    assert_eq!(st.proxies[3].speed_bps, Some(0));
    // un ricontrollo aggiorna la latenza ma tiene la velocità misurata
    st.upsert_good(&b, false);
    assert_eq!(st.proxies[1].speed_bps, Some(2_000_000));

    st.settings.sort = SortBy::Speed;
    let order: Vec<String> = st.sorted_proxies().iter().map(|p| p.ip.to_string()).collect();
    assert_eq!(order, ["2.2.2.2", "1.1.1.1", "3.3.3.3", "4.4.4.4"]);

    // vecchi file senza il campo: si leggono, e None non viene scritto
    let json = serde_json::to_string(&c).unwrap();
    assert!(!json.contains("speed_bps"));
    let back: GoodProxy = serde_json::from_str(&json).unwrap();
    assert_eq!(back.speed_bps, None);
}
