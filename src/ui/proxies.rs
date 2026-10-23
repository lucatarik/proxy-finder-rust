//! Scheda Proxy: i proxy verificati.

use super::{ago, latency_color, App, RED};
use eframe::egui::{self, Color32, RichText};
use proxy_finder::model::GoodProxy;
use proxy_finder::rating;
use proxy_finder::store::SortBy;

impl App {
    pub fn proxies_tab(&mut self, ui: &mut egui::Ui) {
        let busy = self.busy();
        let n_dead = self.st.proxies.iter().filter(|p| !p.alive).count();
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy && !self.st.proxies.is_empty(), egui::Button::new("Ricontrolla")).clicked() {
                self.start_recheck();
            }
            let n_speed = self.st.proxies.iter().filter(|p| p.alive && p.https).count();
            if ui
                .add_enabled(!busy && n_speed > 0, egui::Button::new(format!("Speedtest ({n_speed})")))
                .on_hover_text("Scarica 2 MB attraverso ogni proxy funzionante con https, 3 alla volta, al più 15 s ciascuno, e salva la velocità di tutti.")
                .clicked()
            {
                self.start_speed();
            }
            if ui
                .add_enabled(!busy && n_dead > 0, egui::Button::new(format!("Elimina non funzionanti ({n_dead})")))
                .clicked()
            {
                self.remove_dead();
            }
            ui.separator();
            ui.label("Ordina per");
            let before = self.st.settings.sort;
            ui.selectable_value(&mut self.st.settings.sort, SortBy::Latency, "latenza");
            ui.selectable_value(&mut self.st.settings.sort, SortBy::Speed, "velocità");
            ui.selectable_value(&mut self.st.settings.sort, SortBy::Added, "aggiunta");
            if before != self.st.settings.sort {
                self.save_settings();
            }
            ui.separator();
            if ui.button("Copia tutti").on_hover_text("proto://ip:porta, uno per riga (solo funzionanti)").clicked() {
                ui.ctx().copy_text(self.export_txt());
            }
        });
        ui.horizontal(|ui| {
            ui.label("Esporta in");
            ui.add(egui::TextEdit::singleline(&mut self.export_path).desired_width(360.0));
            if ui.button("txt").clicked() {
                let r = std::fs::write(&self.export_path, self.export_txt());
                self.report_export(r);
            }
            if ui.button("json").clicked() {
                let path = std::path::Path::new(&self.export_path).with_extension("json");
                let r = serde_json::to_string_pretty(&self.st.proxies)
                    .map_err(std::io::Error::other)
                    .and_then(|j| std::fs::write(&path, j));
                self.report_export(r);
            }
        });
        ui.separator();

        if self.st.proxies.is_empty() {
            ui.add_space(20.0);
            ui.label("Nessun proxy verificato. Vai in Scansione: scarica le liste e avvia un test.");
            return;
        }

        let list = self.st.sorted_proxies();
        let active = self.active.as_ref().map(|a| a.key());
        let mut connect: Option<GoodProxy> = None;
        let mut remove: Option<String> = None;
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("proxies").striped(true).num_columns(10).spacing([12.0, 6.0]).show(ui, |ui| {
                for h in ["Paese", "Indirizzo", "Tipo", "Latenza", "Download", "IP uscita", "Score", "Sorgenti", "Verificato", ""] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for p in &list {
                    let key = p.key();
                    let is_active = active.as_deref() == Some(key.as_str());
                    let dim = |t: RichText| if p.alive { t } else { t.weak() };
                    self.country(ui, &p.country, !p.alive);

                    let mut addr = RichText::new(format!("{}:{}", p.ip, p.port)).monospace();
                    if is_active {
                        addr = addr.strong().color(super::GREEN);
                    }
                    if !p.https {
                        addr = addr.underline().color(RED);
                    }
                    let resp = ui
                        .add_enabled(!self.busy() && !is_active, egui::Button::new(dim(addr)).frame(false))
                        .on_hover_text(if is_active { "Proxy attivo".to_string() } else { format!("Riverifica e connetti a {key}") });
                    if resp.clicked() {
                        connect = Some(p.clone());
                    }

                    ui.horizontal(|ui| {
                        ui.label(dim(RichText::new(p.protocol.as_str().to_uppercase()).small()));
                        if !p.https {
                            ui.label(RichText::new(" SOLO HTTP ").small().strong().color(Color32::WHITE).background_color(RED))
                                .on_hover_text("Passa il traffico in chiaro ma non una connessione TLS verso la porta 443: siti https non raggiungibili.");
                        }
                    });
                    if p.alive {
                        ui.label(RichText::new(format!("{} ms", p.latency_ms)).color(latency_color(p.latency_ms)));
                    } else {
                        ui.label(RichText::new("non risponde").color(RED));
                    }
                    let speed = RichText::new(rating::speed_text(p.speed_bps));
                    ui.label(dim(if p.speed_bps == Some(0) { speed.color(RED) } else { speed }));
                    ui.label(dim(RichText::new(&p.exit_ip).monospace()));
                    ui.label(dim(RichText::new(format!("{:.0}", p.score * 100.0))));
                    let short = match p.sources.len() {
                        0..=2 => p.sources.join(", "),
                        n => format!("{}, {} +{}", p.sources[0], p.sources[1], n - 2),
                    };
                    ui.label(dim(RichText::new(short).small())).on_hover_text(p.sources.join("\n"));
                    ui.label(dim(RichText::new(ago(p.checked_at)).small()));
                    ui.horizontal(|ui| {
                        if ui.small_button("copia").on_hover_text("Copia proto://ip:porta").clicked() {
                            ui.ctx().copy_text(key.clone());
                        }
                        if ui.add_enabled(!is_active, egui::Button::new("rimuovi").small()).on_hover_text("Rimuovi").clicked() {
                            remove = Some(key.clone());
                        }
                    });
                    ui.end_row();
                }
            });
        });
        if let Some(p) = connect {
            self.start_connect(Some(p));
        }
        if let Some(k) = remove {
            self.remove_proxy(&k);
        }
    }

    fn export_txt(&self) -> String {
        let mut out: Vec<String> = self.st.sorted_proxies().iter().filter(|p| p.alive).map(|p| p.key()).collect();
        out.push(String::new());
        out.join("\n")
    }

    fn report_export(&mut self, r: std::io::Result<()>) {
        match r {
            Ok(()) => self.last_summary = "esportazione completata".into(),
            Err(e) => self.errors.push(format!("esportazione: {e}")),
        }
    }
}
