//! Prova reale in rete: scarica alcune sorgenti, prova N proxy, stampa rating.
//! `cargo run --release --example smoke -- [quanti] [id_sorgente ...]`

use proxy_finder::{rating, sources, tester};
use std::collections::HashSet;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let n: usize = args.first().and_then(|a| a.parse().ok()).unwrap_or(64);
    let ids: Vec<&str> = if args.len() > 1 { args[1..].iter().map(|s| s.as_str()).collect() } else { vec!["monosans", "proxifly", "jetkai", "iplocate"] };
    let mut srcs: Vec<_> = sources::default_sources().into_iter().filter(|s| ids.contains(&s.id.as_str())).collect();
    let client = sources::http_client();
    let cancel = CancellationToken::new();
    let res = sources::fetch_all(&client, &srcs, &cancel, |r| match &r.result {
        Ok(l) => println!("{:<16} {} voci", r.id, l.len()),
        Err(e) => println!("{:<16} ERRORE {e}", r.id),
    })
    .await;
    let lists = res.into_iter().filter_map(|r| r.result.ok()).map(|l| sources::exclude_known(l, &HashSet::new())).collect();
    let list = sources::merge(lists);
    println!("unione: {}", list.len());
    let ip = tester::real_ip(&client).await.expect("ip reale");
    println!("IP reale {ip}");
    let r = rating::ratings(&srcs);
    let mut l = rating::order_by_priority(list, &r);
    l.truncate(n);
    let t0 = std::time::Instant::now();
    let mut good = 0;
    let mut outs = Vec::new();
    tester::test_pool(l, ip, tester::TestConfig::default(), cancel, |c, o| {
        if let tester::Outcome::Good(g) = o {
            good += 1;
            println!("OK {} {} ms https={} uscita {} {}", c.key(), g.latency_ms, g.https, g.exit_ip, g.country);
        }
        outs.push((c.clone(), o.clone()));
    })
    .await;
    for (c, o) in &outs {
        rating::apply_outcome(&mut srcs, c, o);
    }
    println!("{good}/{} buoni in {:.1}s", outs.len(), t0.elapsed().as_secs_f64());
    for s in &srcs {
        println!("{:<16} rating {:.1}  provati {} buoni {} morti {}", s.id, rating::source_rating(s) * 100.0, s.stats.tested, s.stats.good, s.stats.dead);
    }
}
