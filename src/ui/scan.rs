//! Scheda Scansione: lista scaricata, opzioni e avvio dei test, registro live.

use super::{App, GREEN, RED};
use eframe::egui::{self, RichText};
use proxy_finder::job::JobKind;
use proxy_finder::model::Protocol;
use proxy_finder::state::{round8, MAX_PARALLEL};
use proxy_finder::store::Order;
use std::collections::BTreeMap;
use std::time::Duration;

/// Preset dei timeout (http, https) in secondi.
const PRESETS: [(&str, u64, u64); 3] = [("rapidi", 5, 8), ("normali", 10, 15), ("pazienti", 15, 25)];


/// Riepilogo della lista scaricata: ricalcolarlo a ogni frame su decine di migliaia di voci costa troppo.
pub struct ScanCache {
    key: (u64, Option<Protocol>, String),
    by_proto: String,
    by_source: String,
    available: usize,
}

impl App {
    fn scan_cache(&mut self) -> &ScanCache {
        let key = (self.cand_gen, self.st.settings.test_protocol, self.st.settings.test_country.clone());
        if self.scan_cache.as_ref().is_none_or(|c| c.key != key) {
            let mut by_proto: BTreeMap<Protocol, usize> = BTreeMap::new();
            let mut by_source: BTreeMap<&str, usize> = BTreeMap::new();
            for c in &self.st.candidates {
                *by_proto.entry(c.protocol).or_default() += 1;
                for s in &c.sources {
                    *by_source.entry(s.as_str()).or_default() += 1;
                }
            }
            let mut srcs: Vec<_> = by_source.into_iter().collect();
            srcs.sort_by(|a, b| b.1.cmp(&a.1));
            self.scan_cache = Some(ScanCache {
                by_proto: by_proto.iter().map(|(p, c)| format!("{p}: {c}")).collect::<Vec<_>>().join(" · "),
                by_source: srcs.iter().map(|(s, c)| format!("{s} {c}")).collect::<Vec<_>>().join(", "),
                available: self.st.filtered_candidates().len(),
                key,
            });
        }
        self.scan_cache.as_ref().expect("cache appena riempita")
    }

    pub fn scan_tab(&mut self, ui: &mut egui::Ui) {
        let busy = self.busy();
        ui.columns(2, |cols| {
            self.download_box(&mut cols[0], busy);
            self.test_box(&mut cols[1], busy);
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Registro").strong());
            if self.test_stats.tested > 0 && self.running_kind() != Some(JobKind::Test) {
                let (t, g, a) = (self.test_stats.tested, self.test_stats.good, self.test_stats.added);
                ui.label(format!("ultimo test: {t} provati · {g} buoni · {a} aggiunti"));
                if a > 0 && ui.link("vai ai Proxy").clicked() {
                    self.tab = super::Tab::Proxy;
                }
            }
        });
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for l in &self.log {
                let color = if l.starts_with("OK") { GREEN } else if l.starts_with("KO") { ui.visuals().weak_text_color() } else { ui.visuals().text_color() };
                ui.label(RichText::new(l).monospace().color(color));
            }
        });
    }

    fn download_box(&mut self, ui: &mut egui::Ui, busy: bool) {
        ui.heading("Lista scaricata");
        let n = self.st.candidates.len();
        let enabled = self.st.sources.iter().filter(|s| s.enabled).count();
        ui.label(format!("{n} proxy non ancora provati"));
        let cache = self.scan_cache();
        if n > 0 {
            ui.label(&cache.by_proto);
            ui.label(RichText::new(&cache.by_source).small()).on_hover_text("voci per sorgente (un proxy può stare in più liste)");
        }
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let label = if self.running_kind() == Some(JobKind::Download) { "Download…".to_string() } else { format!("Scarica abilitate ({enabled})") };
            if ui.add_enabled(!busy && enabled > 0, egui::Button::new(label)).clicked() {
                self.start_download();
            }
            if ui.add_enabled(!busy && n > 0, egui::Button::new("Svuota lista")).clicked() {
                self.confirm_clear_list = true;
            }
            let h = self.st.history.len();
            if ui
                .add_enabled(!busy && h > 0, egui::Button::new(format!("Svuota storico ({h})")))
                .on_hover_text("I proxy già provati possono tornare in lista al prossimo download")
                .clicked()
            {
                self.st.history.clear();
                self.save_candidates_history();
            }
        });
        ui.label(
            RichText::new("Le liste vengono unite senza duplicati, una voce per sorgente a turno, senza limite. I proxy già provati non tornano finché non svuoti lo storico.")
                .small()
                .weak(),
        );
    }

    fn test_box(&mut self, ui: &mut egui::Ui, busy: bool) {
        ui.heading("Test");
        let available = self.scan_cache().available;
        let s = &mut self.st.settings;
        let mut changed = false;
        egui::Grid::new("testopts").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            ui.label("Protocollo");
            egui::ComboBox::from_id_salt("proto")
                .selected_text(s.test_protocol.map(|p| p.to_string()).unwrap_or_else(|| "tutti".into()))
                .show_ui(ui, |ui| {
                    changed |= ui.selectable_value(&mut s.test_protocol, None, "tutti").changed();
                    for p in Protocol::ALL {
                        changed |= ui.selectable_value(&mut s.test_protocol, Some(p), p.as_str()).changed();
                    }
                });
            ui.end_row();

            ui.label("Paese");
            let r = ui.add(egui::TextEdit::singleline(&mut s.test_country).hint_text("es. ITA, USA (vuoto = tutti)").desired_width(160.0));
            if r.changed() {
                s.test_country = s.test_country.to_ascii_uppercase().chars().filter(|c| c.is_ascii_alphabetic() || *c == ',' || *c == ' ').collect();
                changed = true;
            }
            ui.end_row();

            ui.label("Quanti");
            ui.horizontal(|ui| {
                let mut count = s.test_count;
                if ui.add(egui::DragValue::new(&mut count).range(8..=usize::MAX).speed(8.0)).changed() {
                    s.test_count = round8(count);
                    changed = true;
                }
                if ui.add_enabled(available > 0, egui::Button::new("tutti")).on_hover_text("Prova tutti i candidati disponibili").clicked() {
                    s.test_count = round8(available);
                    changed = true;
                }
            });
            ui.end_row();

            ui.label("In parallelo");
            let mut par = s.test.concurrency;
            if ui.add(egui::DragValue::new(&mut par).range(8..=MAX_PARALLEL).speed(8.0)).changed() {
                s.test.concurrency = round8(par).min(MAX_PARALLEL);
                changed = true;
            }
            ui.end_row();

            ui.label("Timeout");
            ui.horizontal(|ui| {
                for (name, h, hs) in PRESETS {
                    let on = s.test.http_timeout.as_secs() == h && s.test.https_timeout.as_secs() == hs;
                    if ui.selectable_label(on, format!("{name} {h}/{hs} s")).clicked() {
                        s.test.http_timeout = Duration::from_secs(h);
                        s.test.https_timeout = Duration::from_secs(hs);
                        changed = true;
                    }
                }
            });
            ui.end_row();

            ui.label("Ordine");
            ui.horizontal(|ui| {
                changed |= ui.selectable_value(&mut s.order, Order::Priority, "prima i più promettenti").changed();
                changed |= ui.selectable_value(&mut s.order, Order::Random, "casuale").changed();
            });
            ui.end_row();
        });
        changed |= ui.checkbox(&mut s.test.retry_after_timeout, "Riprova anche dopo un timeout").changed();
        changed |= ui
            .checkbox(&mut s.test.drop_http_only, "Scarta i proxy che non reggono https")
            .on_hover_text("Vale per tutti i protocolli, socks compresi: scarta chi passa la prova in chiaro (porta 80) ma non riesce ad aprire una connessione TLS verso un sito https (porta 443 bloccata, TLS fallita o troppo lenta).")
            .changed();
        if changed {
            self.save_settings();
        }

        let (mut start, mut cancel) = (false, false);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let n = available.min(self.st.settings.test_count);
            let label = if self.running_kind() == Some(JobKind::Test) { "Test in corso…".to_string() } else { format!("Avvia test ({n} di {available})") };
            start = ui.add_enabled(!busy && n > 0, egui::Button::new(RichText::new(label).strong())).clicked();
            cancel = busy && ui.button(RichText::new("Annulla").color(RED)).clicked();
        });
        if start {
            let picked = self.st.pick_candidates();
            self.start_test(picked);
        }
        if cancel {
            self.cancel_job();
        }
        if let Some(ip) = &self.real_ip {
            ui.label(RichText::new(format!("IP reale: {ip}")).small().weak());
        }
    }
}
