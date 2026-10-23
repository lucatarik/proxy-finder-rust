//! Interfaccia eframe: stato dell'app, applicazione degli eventi dei job, schede.

mod proxies;
mod scan;
mod settings;
mod sources;

use eframe::egui::{self, Color32, RichText};
use proxy_finder::job::{self, JobEvent, JobHandle, JobKind, Repaint};
use proxy_finder::model::{now_secs, Candidate, GoodProxy};
use proxy_finder::relay::{self, Upstream};
use proxy_finder::state::{Part, State, TestCount, ALL_PARTS};
use proxy_finder::store::Store;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use tokio_util::sync::CancellationToken;

pub const GREEN: Color32 = Color32::from_rgb(0x2e, 0xa0, 0x43);
pub const YELLOW: Color32 = Color32::from_rgb(0xd4, 0xa0, 0x17);
pub const RED: Color32 = Color32::from_rgb(0xd9, 0x3f, 0x3f);
const LOG_MAX: usize = 200;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Proxy,
    Scan,
    Sources,
    Settings,
}

#[derive(Default)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub found: usize,
    pub phase: String,
}

/// Campi del modulo "Aggiungi sorgente".
pub struct NewSource {
    pub name: String,
    pub url: String,
    pub format: proxy_finder::model::Format,
    pub protocol: Option<proxy_finder::model::Protocol>,
}

pub struct App {
    rt: tokio::runtime::Runtime,
    pub st: State,
    pub real_ip: Option<String>,
    pub exit_ip: Option<String>,

    job: Option<JobHandle>,
    pub progress: Progress,
    pub log: VecDeque<String>,
    pub last_summary: String,
    pub errors: Vec<String>,
    /// Esiti dell'ultimo test.
    pub test_stats: TestCount,

    upstream: Upstream,
    pub active: Option<GoodProxy>,
    relay_cancel: Option<CancellationToken>,
    pub relay_addr: Option<SocketAddr>,

    pub tab: Tab,
    pub new_source: NewSource,
    pub confirm_remove: Option<String>,
    pub confirm_clear_list: bool,
    pub export_path: String,
    pub relay_port_edit: u16,
    /// Cresce a ogni modifica di `candidates`: invalida la cache della scheda Scansione.
    pub cand_gen: u64,
    pub scan_cache: Option<scan::ScanCache>,
    /// Atlante delle bandiere (assets/flags.png), una cella per voce di `countries`.
    flags: egui::TextureHandle,
    repaint: Repaint,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> App {
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("runtime tokio");
        let ctx = cc.egui_ctx.clone();
        let repaint: Repaint = Arc::new(move || ctx.request_repaint());
        let mut errors = Vec::new();
        let store = match Store::open() {
            Ok(s) => Some(s),
            Err(e) => {
                errors.push(format!("cartella dati non disponibile: {e}"));
                None
            }
        };
        let st = State::load(store);
        let export_path = st
            .store
            .as_ref()
            .map(|s| s.dir.join("export.txt").display().to_string())
            .unwrap_or_else(|| "export.txt".into());
        let relay_port_edit = st.settings.relay_port;
        let mut app = App {
            rt,
            st,
            real_ip: None,
            exit_ip: None,
            job: None,
            progress: Progress::default(),
            log: VecDeque::new(),
            last_summary: String::new(),
            errors,
            test_stats: TestCount::default(),
            upstream: Arc::new(RwLock::new(None)),
            active: None,
            relay_cancel: None,
            relay_addr: None,
            tab: Tab::Proxy,
            new_source: NewSource {
                name: String::new(),
                url: String::new(),
                format: proxy_finder::model::Format::Auto,
                protocol: None,
            },
            confirm_remove: None,
            confirm_clear_list: false,
            export_path,
            relay_port_edit,
            cand_gen: 0,
            scan_cache: None,
            flags: load_flags(&cc.egui_ctx),
            repaint,
        };
        app.start_relay();
        app
    }

    // ---------- persistenza ----------

    fn persist(&mut self, parts: &[Part]) {
        let errs = self.st.save(parts);
        self.errors.extend(errs);
    }

    pub fn save_sources(&mut self) {
        self.persist(&[Part::Sources]);
    }
    pub fn save_settings(&mut self) {
        self.persist(&[Part::Settings]);
    }
    pub fn save_proxies(&mut self) {
        self.persist(&[Part::Proxies]);
    }
    pub fn save_candidates_history(&mut self) {
        self.persist(&[Part::Candidates, Part::History]);
    }

    pub fn data_dir(&self) -> String {
        self.st.data_dir()
    }

    // ---------- relay ----------

    pub fn start_relay(&mut self) {
        if let Some(c) = self.relay_cancel.take() {
            c.cancel();
        }
        self.relay_addr = None;
        let addr = SocketAddr::from(([127, 0, 0, 1], self.st.settings.relay_port));
        let listener = self.rt.block_on(relay::bind_relay(addr));
        match listener {
            Ok(l) => {
                let cancel = CancellationToken::new();
                self.rt.spawn(relay::serve(l, self.upstream.clone(), cancel.clone()));
                self.relay_cancel = Some(cancel);
                self.relay_addr = Some(addr);
            }
            Err(e) => self.errors.push(format!("relay su {addr} non avviato: {e}")),
        }
    }

    fn set_active(&mut self, p: Option<GoodProxy>) {
        if let Ok(mut g) = self.upstream.write() {
            *g = p.clone();
        }
        self.active = p;
        self.exit_ip = None;
    }

    pub fn disconnect(&mut self) {
        self.set_active(None);
    }

    // ---------- job ----------

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn running_kind(&self) -> Option<JobKind> {
        self.job.as_ref().map(|j| j.kind)
    }

    pub fn cancel_job(&mut self) {
        if let Some(j) = &self.job {
            j.cancel.cancel();
        }
    }

    fn begin(&mut self, h: JobHandle) {
        self.progress = Progress::default();
        self.last_summary.clear();
        self.job = Some(h);
    }

    pub fn start_download(&mut self) {
        if self.busy() {
            return;
        }
        let enabled = self.st.enabled_sources();
        if enabled.is_empty() {
            self.errors.push("nessuna sorgente abilitata".into());
            return;
        }
        let known = self.st.known_keys();
        let h = job::start_download(self.rt.handle(), self.repaint.clone(), enabled, known, self.st.asn_stale());
        self.begin(h);
    }

    pub fn start_test(&mut self, cands: Vec<Candidate>) {
        if self.busy() || cands.is_empty() {
            return;
        }
        self.test_stats = TestCount::default();
        let h = job::start_test(self.rt.handle(), self.repaint.clone(), cands, self.st.settings.test.clone(), self.real_ip.clone());
        self.begin(h);
    }

    pub fn start_recheck(&mut self) {
        if self.busy() || self.st.proxies.is_empty() {
            return;
        }
        let mut cfg = self.st.settings.test.clone();
        normal_timeouts(&mut cfg);
        let h = job::start_recheck(self.rt.handle(), self.repaint.clone(), self.st.proxies.clone(), cfg, self.real_ip.clone());
        self.begin(h);
    }

    /// Speedtest dei proxy funzionanti con https.
    pub fn start_speed(&mut self) {
        if self.busy() {
            return;
        }
        let list = self.st.fastest_https();
        if list.is_empty() {
            self.errors.push("nessun proxy https funzionante da misurare".into());
            return;
        }
        let h = job::start_speed(self.rt.handle(), self.repaint.clone(), list, false);
        self.begin(h);
    }

    /// Riverifica e connette: con `only` un proxy preciso, altrimenti il più veloce con https.
    pub fn start_connect(&mut self, only: Option<GoodProxy>) {
        if self.busy() {
            return;
        }
        let list = match only {
            Some(p) => vec![p],
            None => self.st.fastest_https(),
        };
        if list.is_empty() {
            self.errors.push("nessun proxy https funzionante: avvia una scansione".into());
            return;
        }
        let mut cfg = self.st.settings.test.clone();
        normal_timeouts(&mut cfg);
        let h = job::start_connect(self.rt.handle(), self.repaint.clone(), list, cfg, self.real_ip.clone());
        self.begin(h);
    }

    pub fn start_verify_ip(&mut self) {
        if self.busy() {
            return;
        }
        let h = job::start_verify_ip(self.rt.handle(), self.repaint.clone(), self.active.clone());
        self.begin(h);
    }

    fn poll_job(&mut self) {
        let Some(j) = &self.job else { return };
        let kind = j.kind;
        let events: Vec<JobEvent> = j.rx.try_iter().collect();
        let mut finished = false;
        for e in events {
            match e {
                JobEvent::Started(_) => {}
                JobEvent::Progress { done, total, found, phase } => self.progress = Progress { done, total, found, phase },
                JobEvent::Log(l) => {
                    self.log.push_front(l);
                    self.log.truncate(LOG_MAX);
                }
                JobEvent::SourceDownloaded { id, count, error } => self.st.record_download(&id, count, error),
                JobEvent::Downloaded { list, .. } => {
                    self.st.candidates = list;
                    self.cand_gen += 1;
                }
                JobEvent::AsnTable { table, gz } => {
                    let errs = self.st.set_asn(table, &gz);
                    self.errors.extend(errs);
                }
                JobEvent::Tested { cand, outcome } => self.st.apply_tested(kind, &cand, &outcome, &mut self.test_stats),
                JobEvent::Speed { key, result } => self.st.apply_speed(&key, &result),
                JobEvent::Connected(g) => {
                    self.st.upsert_good(&g, false);
                    self.set_active(Some(g));
                }
                JobEvent::RealIp(ip) => self.real_ip = Some(ip),
                JobEvent::Ips { real, exit } => {
                    self.real_ip = Some(real);
                    self.exit_ip = exit;
                }
                JobEvent::Error(e) => self.errors.push(e),
                JobEvent::Finished { summary, cancelled, .. } => {
                    self.last_summary = if cancelled { format!("{summary} (annullato)") } else { summary };
                    finished = true;
                }
            }
        }
        if finished {
            self.job = None;
            let errs = self.st.finish(kind);
            self.errors.extend(errs);
            self.cand_gen += 1;
        }
    }

    pub fn remove_proxy(&mut self, key: &str) {
        let active = self.active.as_ref().map(|a| a.key());
        if self.st.remove_proxy(key, active.as_deref()) {
            self.save_proxies();
        }
    }

    pub fn remove_dead(&mut self) {
        let active = self.active.as_ref().map(|a| a.key());
        self.st.remove_dead(active.as_deref());
        self.save_proxies();
    }

    // ---------- disegno ----------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Proxy Finder");
            ui.separator();
            match &self.active {
                Some(a) => {
                    let color = if a.https { GREEN } else { RED };
                    dot(ui, color);
                    ui.label(format!("Connesso a {}  (", a.key()));
                    self.country(ui, &a.country, false);
                    ui.label(format!("· {} ms)", a.latency_ms));
                    if !a.https {
                        ui.label(RichText::new(" SOLO HTTP ").strong().color(Color32::WHITE).background_color(RED));
                    }
                }
                None => {
                    dot(ui, Color32::GRAY);
                    ui.label("Disconnesso");
                }
            }
            ui.separator();
            match self.relay_addr {
                Some(a) => ui.label(format!("relay http://{a}")),
                None => ui.label(RichText::new("relay spento").color(RED)),
            };
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.active.is_some()
                    && ui.add_enabled(!self.busy(), egui::Button::new("Disconnetti")).clicked() {
                        self.disconnect();
                    }
                let label = if self.running_kind() == Some(JobKind::Connect) { "Connessione…" } else { "Connetti al più veloce" };
                if ui.add_enabled(!self.busy(), egui::Button::new(label)).clicked() {
                    self.start_connect(None);
                }
            });
        });
        if self.active.as_ref().is_some_and(|a| !a.https) {
            egui::Frame::new().fill(RED).inner_margin(6.0).show(ui, |ui| {
                ui.label(
                    RichText::new("Proxy SOLO HTTP: i siti https non si aprono e quelli http passano IN CHIARO. Chi gestisce il proxy può leggere e modificare tutto.")
                        .color(Color32::WHITE),
                );
            });
        }
        let mut dismiss = None;
        for (i, e) in self.errors.iter().enumerate().rev().take(3) {
            egui::Frame::new().fill(RED.gamma_multiply(0.25)).inner_margin(4.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(e).color(RED));
                    if ui.small_button("chiudi").clicked() {
                        dismiss = Some(i);
                    }
                });
            });
        }
        if let Some(i) = dismiss {
            self.errors.remove(i);
        }
    }

    /// Barra di avanzamento del job in corso (o riepilogo dell'ultimo).
    pub fn job_bar(&mut self, ui: &mut egui::Ui) {
        if let Some(kind) = self.running_kind() {
            ui.horizontal(|ui| {
                let p = &self.progress;
                let frac = if p.total > 0 { p.done as f32 / p.total as f32 } else { 0.0 };
                let text = if p.total > 0 {
                    format!("{} · {}: {}/{} · trovati {}", kind.label(), p.phase, p.done, p.total, p.found)
                } else {
                    format!("{}…", kind.label())
                };
                ui.add(egui::ProgressBar::new(frac).text(text).desired_width(ui.available_width() - 90.0).animate(true));
                if ui.button("Annulla").clicked() {
                    self.cancel_job();
                }
            });
        } else if !self.last_summary.is_empty() {
            ui.label(RichText::new(&self.last_summary).italics());
        }
    }
}

/// Pallino di stato disegnato (il font di default non ha U+25CF).
fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 5.0, color);
}

/// Timeout "normali" per ricontrollo e connessione.
fn normal_timeouts(cfg: &mut proxy_finder::tester::TestConfig) {
    cfg.http_timeout = std::time::Duration::from_secs(10);
    cfg.https_timeout = std::time::Duration::from_secs(15);
    cfg.retry_after_timeout = true;
    cfg.concurrency = cfg.concurrency.max(8);
}

fn load_flags(ctx: &egui::Context) -> egui::TextureHandle {
    let img = image::load_from_memory(include_bytes!("../../assets/flags.png")).expect("atlante bandiere").to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    ctx.load_texture("flags", color, egui::TextureOptions::LINEAR)
}

impl App {
    /// Bandiera e codice a 3 lettere del paese (`weak` per i proxy che non rispondono).
    pub fn country(&self, ui: &mut egui::Ui, cc: &str, weak: bool) {
        use proxy_finder::countries::{alpha3, flag_index, FLAG_COLS, FLAG_H, FLAG_W};
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let size = egui::vec2(20.0, 15.0);
            match flag_index(cc) {
                Some(i) => {
                    let [w, h] = self.flags.size().map(|v| v as f32);
                    let (col, row) = ((i as u32 % FLAG_COLS) as f32, (i as u32 / FLAG_COLS) as f32);
                    let (fw, fh) = (FLAG_W as f32, FLAG_H as f32);
                    let uv = egui::Rect::from_min_size(egui::pos2(col * fw / w, row * fh / h), egui::vec2(fw / w, fh / h));
                    let tint = if weak { Color32::from_white_alpha(110) } else { Color32::WHITE };
                    ui.add(egui::Image::new(egui::load::SizedTexture::new(self.flags.id(), size)).uv(uv).tint(tint));
                }
                None => {
                    ui.add_space(size.x);
                }
            }
            let t = RichText::new(alpha3(cc)).monospace();
            ui.label(if weak { t.weak() } else { t });
        });
    }
}

pub fn latency_color(ms: u32) -> Color32 {
    if ms < 300 {
        GREEN
    } else if ms < 1000 {
        YELLOW
    } else {
        RED
    }
}

pub fn rating_color(r: f64) -> Color32 {
    if r >= 0.4 {
        GREEN
    } else if r >= 0.2 {
        YELLOW
    } else {
        RED
    }
}

pub fn ago(ts: i64) -> String {
    let d = (now_secs() - ts).max(0);
    match d {
        0..=59 => format!("{d} s fa"),
        60..=3599 => format!("{} min fa", d / 60),
        3600..=86_399 => format!("{} h fa", d / 3600),
        _ => format!("{} g fa", d / 86_400),
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_job();
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui);
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let n_dead = self.st.proxies.iter().filter(|p| !p.alive).count();
                let tabs = [
                    (Tab::Proxy, format!("Proxy ({})", self.st.proxies.len() - n_dead)),
                    (Tab::Scan, format!("Scansione ({})", self.st.candidates.len())),
                    (Tab::Sources, format!("Sorgenti ({})", self.st.sources.iter().filter(|s| s.enabled).count())),
                    (Tab::Settings, "Impostazioni".to_string()),
                ];
                for (t, label) in tabs {
                    ui.selectable_value(&mut self.tab, t, label);
                }
            });
            ui.add_space(2.0);
        });
        egui::TopBottomPanel::bottom("job").show(ctx, |ui| {
            ui.add_space(3.0);
            self.job_bar(ui);
            ui.add_space(3.0);
        });
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Proxy => self.proxies_tab(ui),
            Tab::Scan => self.scan_tab(ui),
            Tab::Sources => self.sources_tab(ui),
            Tab::Settings => self.settings_tab(ui),
        });
        self.dialogs(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.cancel_job();
        if let Some(c) = &self.relay_cancel {
            c.cancel();
        }
        self.persist(&ALL_PARTS);
    }
}
