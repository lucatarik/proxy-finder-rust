//! Icona nel vassoio di sistema (StatusNotifierItem): collega e scollega il relay e imposta il
//! proxy di sistema GNOME (`gsettings`). Il lavoro vero lo fa `proxy-finder --auto` lanciato come
//! processo figlio: stesse regole, stessi dati e stesso cambio di proxy della riga di comando.

use ksni::TrayMethods;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug)]
enum Cmd {
    Connect,
    Change,
    Disconnect,
    Quit,
}

#[derive(Clone, Debug, PartialEq)]
enum Status {
    Off,
    /// Ricerca in corso (collegamento o cambio proxy).
    Busy(String),
    On { exit_ip: String, changing: bool },
    Error(String),
}

struct Tray {
    status: Status,
    port: u16,
    tx: mpsc::UnboundedSender<Cmd>,
}

impl Tray {
    fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }

    fn status_text(&self) -> String {
        match &self.status {
            Status::Off => "Scollegato".into(),
            Status::Busy(s) => s.clone(),
            Status::On { exit_ip, changing: false } => format!("Collegato, uscita {exit_ip}"),
            Status::On { exit_ip, changing: true } => format!("Collegato, uscita {exit_ip} (cerco un altro proxy…)"),
            Status::Error(e) => format!("Errore: {e}"),
        }
    }
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "proxy-finder".into()
    }
    fn title(&self) -> String {
        "Proxy Finder".into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        let rgb = match &self.status {
            Status::Off => (0x9e, 0x9e, 0x9e),
            Status::Busy(_) | Status::On { changing: true, .. } => (0xf2, 0xb1, 0x34),
            Status::On { .. } => (0x2e, 0xb8, 0x5c),
            Status::Error(_) => (0xe0, 0x45, 0x3a),
        };
        vec![dot_icon(32, rgb)]
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Proxy Finder".into(),
            description: format!("{}\nRelay: 127.0.0.1:{}", self.status_text(), self.port),
            ..Default::default()
        }
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        let on = matches!(self.status, Status::On { changing: false, .. });
        let idle = matches!(self.status, Status::Off | Status::Error(_));
        vec![
            StandardItem { label: self.status_text(), enabled: false, ..Default::default() }.into(),
            MenuItem::Separator,
            StandardItem {
                label: "Collega".into(),
                enabled: idle,
                activate: Box::new(|t: &mut Self| t.send(Cmd::Connect)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Cambia proxy".into(),
                enabled: on,
                activate: Box::new(|t: &mut Self| t.send(Cmd::Change)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Scollega".into(),
                enabled: !idle,
                activate: Box::new(|t: &mut Self| t.send(Cmd::Disconnect)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem { label: "Esci".into(), activate: Box::new(|t: &mut Self| t.send(Cmd::Quit)), ..Default::default() }.into(),
        ]
    }
}

/// Pallino pieno del colore dato, con bordo scuro (ARGB32, come vuole StatusNotifierItem).
fn dot_icon(size: i32, (r, g, b): (u8, u8, u8)) -> ksni::Icon {
    let c = (size as f32 - 1.0) / 2.0;
    let radius = size as f32 * 0.42;
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            // bordo antialias di un pixel
            let alpha = ((radius + 0.5 - d).clamp(0.0, 1.0) * 255.0) as u8;
            let edge = d > radius - 2.0;
            let k = |v: u8| if edge { v / 2 } else { v };
            data.extend_from_slice(&[alpha, k(r), k(g), k(b)]);
        }
    }
    ksni::Icon { width: size, height: size, data }
}

/// Proxy di sistema GNOME (`org.gnome.system.proxy`): lo seguono Firefox con "usa le
/// impostazioni di sistema" e i programmi GTK/GIO.
mod sysproxy {
    use std::process::Command;

    const KEYS: [(&str, &str); 5] = [
        ("org.gnome.system.proxy", "mode"),
        ("org.gnome.system.proxy.http", "host"),
        ("org.gnome.system.proxy.http", "port"),
        ("org.gnome.system.proxy.https", "host"),
        ("org.gnome.system.proxy.https", "port"),
    ];

    /// Valori com'erano prima di `apply`, in sintassi GVariant (come li stampa `gsettings get`).
    pub struct Saved(Vec<String>);

    fn get(schema: &str, key: &str) -> Result<String, String> {
        let o = Command::new("gsettings").args(["get", schema, key]).output().map_err(|e| format!("gsettings: {e}"))?;
        if !o.status.success() {
            return Err(format!("gsettings get {schema} {key}: {}", String::from_utf8_lossy(&o.stderr).trim()));
        }
        Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
    }

    fn set(schema: &str, key: &str, value: &str) -> Result<(), String> {
        let o = Command::new("gsettings").args(["set", schema, key, value]).output().map_err(|e| format!("gsettings: {e}"))?;
        if !o.status.success() {
            return Err(format!("gsettings set {schema} {key}: {}", String::from_utf8_lossy(&o.stderr).trim()));
        }
        Ok(())
    }

    /// Punta il proxy di sistema (http e https) al relay; restituisce i valori di prima.
    pub fn apply(port: u16) -> Result<Saved, String> {
        let saved = KEYS.iter().map(|(s, k)| get(s, k)).collect::<Result<Vec<_>, _>>()?;
        for schema in ["org.gnome.system.proxy.http", "org.gnome.system.proxy.https"] {
            set(schema, "host", "'127.0.0.1'")?;
            set(schema, "port", &port.to_string())?;
        }
        set("org.gnome.system.proxy", "mode", "'manual'")?;
        Ok(Saved(saved))
    }

    pub fn restore(saved: Saved) -> Result<(), String> {
        // prima la modalità, così nessun programma vede il relay spento come proxy attivo
        for ((schema, key), value) in KEYS.iter().zip(&saved.0) {
            set(schema, key, value)?;
        }
        Ok(())
    }

    /// Dopo una chiusura brusca il proxy di sistema può essere rimasto sul relay: se punta
    /// proprio a 127.0.0.1:`port` e il relay non risponde, lo spegne.
    pub fn reset_if_stale(port: u16) {
        let ours = get("org.gnome.system.proxy", "mode").as_deref() == Ok("'manual'")
            && get("org.gnome.system.proxy.http", "host").as_deref() == Ok("'127.0.0.1'")
            && get("org.gnome.system.proxy.http", "port").as_deref() == Ok(port.to_string().as_str());
        if ours && std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            let _ = set("org.gnome.system.proxy", "mode", "'none'");
        }
    }
}

/// Esito di un processo `--auto`.
enum Event {
    /// Riga su stdout: l'IP di uscita (anche dopo un cambio di proxy).
    Ip(String),
    /// Il processo principale (quello che tiene il relay) è uscito: codice e stderr.
    RelayExited(Option<i32>, String),
    /// Il processo di "Cambia proxy" è uscito.
    ChangeExited(Option<i32>, String),
}

fn spawn_auto(port: u16) -> std::io::Result<Child> {
    let exe = std::env::current_exe()?;
    Command::new(exe)
        .args(["--auto", "--port", &port.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
}

/// Legge stdout (righe = IP) e stderr del figlio, poi ne aspetta l'uscita e la segnala.
/// Il figlio resta in `slot` per poterlo fermare: qui si prende solo il pid.
fn watch(mut child: Child, ev: mpsc::UnboundedSender<Event>, relay: bool) -> Option<u32> {
    let pid = child.id();
    let out = child.stdout.take();
    let err = child.stderr.take();
    tokio::spawn(async move {
        let mut stderr = String::new();
        let read_err = async {
            if let Some(mut e) = err {
                let _ = e.read_to_string(&mut stderr).await;
            }
        };
        let read_out = async {
            if let Some(o) = out {
                let mut lines = BufReader::new(o).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    let l = l.trim().to_string();
                    if !l.is_empty() {
                        let _ = ev.send(Event::Ip(l));
                    }
                }
            }
        };
        tokio::join!(read_err, read_out);
        let code = child.wait().await.ok().and_then(|s| s.code());
        let msg = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim_start_matches("errore: ").to_string();
        let _ = ev.send(if relay { Event::RelayExited(code, msg) } else { Event::ChangeExited(code, msg) });
    });
    pid
}

/// Ctrl+C al figlio: annulla le prove o ferma il relay come da terminale (relay.json compreso).
fn interrupt(pid: Option<u32>) {
    if let Some(p) = pid {
        let _ = std::process::Command::new("kill").args(["-INT", &p.to_string()]).status();
    }
}

pub async fn run(port: u16) -> i32 {
    sysproxy::reset_if_stale(port);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let handle = match (Tray { status: Status::Off, port, tx: tx.clone() }).spawn().await {
        Ok(h) => h,
        Err(e) => {
            eprintln!("errore: vassoio di sistema non disponibile: {e}");
            return 1;
        }
    };
    let set_status = |s: Status| {
        let h = handle.clone();
        async move {
            h.update(|t| t.status = s).await;
        }
    };

    let (ev_tx, mut ev) = mpsc::unbounded_channel();
    let mut relay_pid: Option<u32> = None;
    let mut change_pid: Option<u32> = None;
    let mut saved: Option<sysproxy::Saved> = None;
    let mut exit_ip = String::new();
    let mut quitting = false;
    // fermato da noi (Scollega, Esci): un'uscita senza questo flag è inattesa
    let mut stopping = false;

    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
    let terminated = async {
        match term.as_mut() {
            Some(t) => {
                t.recv().await;
            }
            None => std::future::pending().await,
        }
    };
    tokio::pin!(terminated);

    loop {
        tokio::select! {
            Some(cmd) = rx.recv() => match cmd {
                Cmd::Connect if relay_pid.is_none() => match spawn_auto(port) {
                    Ok(child) => {
                        relay_pid = watch(child, ev_tx.clone(), true);
                        stopping = false;
                        set_status(Status::Busy("Cerco un proxy…".into())).await;
                    }
                    Err(e) => set_status(Status::Error(format!("avvio non riuscito: {e}"))).await,
                },
                Cmd::Change if relay_pid.is_some() && change_pid.is_none() => match spawn_auto(port) {
                    Ok(child) => {
                        change_pid = watch(child, ev_tx.clone(), false);
                        set_status(Status::On { exit_ip: exit_ip.clone(), changing: true }).await;
                    }
                    Err(e) => set_status(Status::Error(format!("avvio non riuscito: {e}"))).await,
                },
                Cmd::Disconnect => {
                    stopping = true;
                    interrupt(change_pid);
                    interrupt(relay_pid);
                    if relay_pid.is_none() {
                        set_status(Status::Off).await;
                    }
                }
                Cmd::Quit => {
                    quitting = true;
                    stopping = true;
                    interrupt(change_pid);
                    interrupt(relay_pid);
                    if relay_pid.is_none() {
                        break;
                    }
                }
                _ => {}
            },
            Some(e) = ev.recv() => match e {
                Event::Ip(ip) => {
                    exit_ip = ip;
                    if saved.is_none() {
                        match sysproxy::apply(port) {
                            Ok(s) => saved = Some(s),
                            Err(e) => {
                                set_status(Status::Error(format!("relay attivo ma proxy di sistema non impostato: {e}"))).await;
                                continue;
                            }
                        }
                    }
                    set_status(Status::On { exit_ip: exit_ip.clone(), changing: change_pid.is_some() }).await;
                }
                Event::ChangeExited(code, msg) => {
                    change_pid = None;
                    if relay_pid.is_some() {
                        // riuscito: il nuovo IP arriva dal processo del relay
                        let s = if code == Some(0) || exit_ip.is_empty() {
                            Status::On { exit_ip: exit_ip.clone(), changing: false }
                        } else {
                            Status::Error(format!("cambio non riuscito ({msg}); uscita ancora {exit_ip}"))
                        };
                        set_status(s).await;
                    }
                }
                Event::RelayExited(code, msg) => {
                    relay_pid = None;
                    exit_ip.clear();
                    interrupt(change_pid);
                    let restore_err = saved.take().and_then(|s| sysproxy::restore(s).err());
                    if quitting {
                        break;
                    }
                    let s = match (code, restore_err) {
                        (_, Some(e)) => Status::Error(format!("proxy di sistema non ripristinato: {e}")),
                        _ if stopping => Status::Off,
                        // --auto esce subito con 0 solo se ha cambiato il proxy a un relay già attivo
                        (Some(0), None) => Status::Error(format!("la porta {port} è già di un altro relay proxy-finder: gli ho cambiato il proxy")),
                        (_, None) => Status::Error(if msg.is_empty() { "relay terminato".into() } else { msg }),
                    };
                    set_status(s).await;
                }
            },
            _ = &mut terminated => {
                quitting = true;
                stopping = true;
                interrupt(change_pid);
                interrupt(relay_pid);
                if relay_pid.is_none() {
                    break;
                }
            }
        }
    }
    // ultima rete di sicurezza: il proxy di sistema non deve restare sul relay spento
    if let Some(s) = saved.take() {
        let _ = sysproxy::restore(s);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    0
}
