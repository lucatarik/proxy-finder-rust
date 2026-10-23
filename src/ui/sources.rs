//! Scheda Sorgenti: elenco, rating, aggiunta e rimozione.

use super::{ago, rating_color, App, RED};
use eframe::egui::{self, RichText};
use proxy_finder::model::{Format, Protocol};
use proxy_finder::rating::{self, source_rating};
use proxy_finder::sources::custom_source;
use proxy_finder::store::restore_default_sources;

impl App {
    pub fn sources_tab(&mut self, ui: &mut egui::Ui) {
        let busy = self.busy();
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new("Ripristina predefinite")).on_hover_text("Rimette le sorgenti incluse (anche quelle rimosse) conservando rating e sorgenti personalizzate").clicked() {
                restore_default_sources(&mut self.st.sources);
                self.save_sources();
            }
            if ui.add_enabled(!busy, egui::Button::new("Azzera tutti i rating")).clicked() {
                self.st.sources.iter_mut().for_each(rating::reset);
                self.save_sources();
            }
            ui.separator();
            if ui.button("Abilita tutte").clicked() {
                self.st.sources.iter_mut().for_each(|s| s.enabled = true);
                self.save_sources();
            }
            if ui.button("Disabilita tutte").clicked() {
                self.st.sources.iter_mut().for_each(|s| s.enabled = false);
                self.save_sources();
            }
        });
        ui.label(
            RichText::new(
                "Rating = (somma degli score dei proxy buoni + prior × 5) / (proxy provati + 5). Score = (1 − latenza/5000) × (1 se regge https, 0,5 se solo http). Un proxy morto conta 0.",
            )
            .small()
            .weak(),
        );
        ui.separator();
        self.add_source_form(ui, busy);
        ui.separator();

        let mut order: Vec<usize> = (0..self.st.sources.len()).collect();
        order.sort_by(|&a, &b| source_rating(&self.st.sources[b]).total_cmp(&source_rating(&self.st.sources[a])));
        let mut changed = false;
        let mut reset: Option<usize> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("sources").striped(true).num_columns(7).spacing([12.0, 6.0]).show(ui, |ui| {
                for h in ["", "Sorgente", "Rating", "Provati / buoni / morti", "Ultimo download", "Endpoint", ""] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for i in order {
                    let s = &mut self.st.sources[i];
                    changed |= ui.add_enabled(!busy, egui::Checkbox::without_text(&mut s.enabled)).changed();
                    ui.horizontal(|ui| {
                        ui.hyperlink_to(&s.name, &s.repo);
                        if !s.builtin {
                            ui.label(RichText::new("personale").small().weak());
                        }
                    });
                    let r = source_rating(s);
                    ui.horizontal(|ui| {
                        ui.add(egui::ProgressBar::new(r as f32).desired_width(80.0).fill(rating_color(r)));
                        ui.label(RichText::new(format!("{:.1}", r * 100.0)).monospace());
                    });
                    let st = &s.stats;
                    let hit = if st.tested > 0 { format!("  ({:.0}% vivi)", st.good as f64 * 100.0 / st.tested as f64) } else { String::new() };
                    ui.label(format!("{} / {} / {}{hit}", st.tested, st.good, st.dead));
                    match (&st.last_error, st.last_download) {
                        (Some(e), _) => {
                            ui.label(RichText::new(format!("errore: {e}")).color(RED).small());
                        }
                        (None, Some(t)) => {
                            ui.label(format!("{} voci · {}", st.last_count, ago(t)));
                        }
                        (None, None) => {
                            ui.label(RichText::new("mai").weak());
                        }
                    }
                    let tip = s
                        .endpoints
                        .iter()
                        .map(|e| format!("{} [{}{}]", e.url, e.format.label(), e.protocol.map(|p| format!(", {p}")).unwrap_or_default()))
                        .collect::<Vec<_>>()
                        .join("\n");
                    ui.label(format!("{}", s.endpoints.len())).on_hover_text(tip);
                    ui.horizontal(|ui| {
                        if ui.small_button("azzera").on_hover_text("Azzera il rating di questa sorgente").clicked() {
                            reset = Some(i);
                        }
                        if ui.add_enabled(!busy, egui::Button::new("rimuovi").small()).on_hover_text("Rimuovi sorgente").clicked() {
                            self.confirm_remove = Some(s.id.clone());
                        }
                    });
                    ui.end_row();
                }
            });
        });
        if let Some(i) = reset {
            rating::reset(&mut self.st.sources[i]);
            changed = true;
        }
        if changed {
            self.save_sources();
        }
    }

    fn add_source_form(&mut self, ui: &mut egui::Ui, busy: bool) {
        ui.label(RichText::new("Aggiungi sorgente").strong());
        let mut add = false;
        let f = &mut self.new_source;
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::TextEdit::singleline(&mut f.name).hint_text("nome").desired_width(160.0));
            ui.add(egui::TextEdit::singleline(&mut f.url).hint_text("https://raw.githubusercontent.com/…/http.txt").desired_width(380.0));
            egui::ComboBox::from_id_salt("fmt").selected_text(f.format.label()).show_ui(ui, |ui| {
                for fmt in Format::ALL {
                    ui.selectable_value(&mut f.format, fmt, fmt.label());
                }
            });
            egui::ComboBox::from_id_salt("newproto")
                .selected_text(f.protocol.map(|p| p.to_string()).unwrap_or_else(|| "http (default)".into()))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut f.protocol, None, "http (default)");
                    for p in Protocol::ALL {
                        ui.selectable_value(&mut f.protocol, Some(p), p.as_str());
                    }
                })
                .response
                .on_hover_text("Protocollo per le righe ip:porta (ignorato dai formati che lo dichiarano)");
            let valid = !f.name.trim().is_empty() && (f.url.starts_with("https://") || f.url.starts_with("http://"));
            add = ui.add_enabled(!busy && valid, egui::Button::new("Aggiungi")).clicked();
        });
        if add {
            let f = &self.new_source;
            let s = custom_source(&f.name, &f.url, f.format, f.protocol, &self.st.sources);
            self.st.sources.push(s);
            self.new_source.name.clear();
            self.new_source.url.clear();
            self.save_sources();
        }
    }

    pub fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(id) = self.confirm_remove.clone() {
            let name = self.st.sources.iter().find(|s| s.id == id).map(|s| s.name.clone()).unwrap_or_default();
            let mut open = true;
            let mut decided = false;
            egui::Window::new("Rimuovi sorgente").collapsible(false).resizable(false).open(&mut open).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.label(format!("Rimuovere «{name}» e il suo rating?"));
                ui.label(RichText::new("Le predefinite si possono rimettere con «Ripristina predefinite».").small().weak());
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Rimuovi").color(RED)).clicked() {
                        self.st.sources.retain(|s| s.id != id);
                        decided = true;
                    }
                    if ui.button("Annulla").clicked() {
                        decided = true;
                    }
                });
            });
            if decided || !open {
                self.confirm_remove = None;
                self.save_sources();
            }
        }
        if self.confirm_clear_list {
            let mut open = true;
            let mut decided = false;
            egui::Window::new("Svuota lista").collapsible(false).resizable(false).open(&mut open).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.label(format!("Eliminare i {} proxy scaricati e non ancora provati?", self.st.candidates.len()));
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Svuota").color(RED)).clicked() {
                        self.st.candidates.clear();
                        self.cand_gen += 1;
                        self.save_candidates_history();
                        decided = true;
                    }
                    if ui.button("Annulla").clicked() {
                        decided = true;
                    }
                });
            });
            if decided || !open {
                self.confirm_clear_list = false;
            }
        }
    }
}
