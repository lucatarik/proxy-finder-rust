//! Scheda Impostazioni: relay, verifica IP, cartella dati.

use super::{App, GREEN, RED};
use eframe::egui::{self, RichText};

impl App {
    pub fn settings_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Relay locale");
        ui.label("Un proxy HTTP su 127.0.0.1 che inoltra tutto al proxy connesso. Configuralo nel browser o nel sistema come proxy HTTP/HTTPS.");
        ui.horizontal(|ui| {
            ui.label("Porta");
            ui.add(egui::DragValue::new(&mut self.relay_port_edit).range(1024..=65535));
            if ui.add_enabled(self.relay_port_edit != self.st.settings.relay_port || self.relay_addr.is_none(), egui::Button::new("Applica")).clicked() {
                self.st.settings.relay_port = self.relay_port_edit;
                self.save_settings();
                self.start_relay();
            }
            match self.relay_addr {
                Some(a) => ui.label(RichText::new(format!("in ascolto su {a}")).color(GREEN)),
                None => ui.label(RichText::new("spento").color(RED)),
            };
        });
        if let Some(a) = self.relay_addr {
            ui.horizontal(|ui| {
                let cmd = format!("chromium --proxy-server=\"http://{a}\"");
                ui.label(RichText::new(&cmd).monospace());
                if ui.small_button("copia").clicked() {
                    ui.ctx().copy_text(cmd);
                }
            });
            ui.label(RichText::new(format!("curl -x http://{a} https://api.ipify.org")).monospace().weak());
        }
        ui.label(
            RichText::new("Senza proxy connesso il relay risponde 503: niente traffico esce in chiaro dal relay.")
                .small()
                .weak(),
        );

        ui.separator();
        ui.heading("Verifica IP");
        ui.horizontal(|ui| {
            if ui.add_enabled(!self.busy(), egui::Button::new("Verifica IP")).clicked() {
                self.start_verify_ip();
            }
            ui.label(format!("IP reale: {}", self.real_ip.as_deref().unwrap_or("?")));
            ui.separator();
            match (&self.active, &self.exit_ip) {
                (None, _) => ui.label("nessun proxy attivo"),
                (Some(_), None) => ui.label("IP di uscita: ?"),
                (Some(_), Some(e)) => {
                    let same = self.real_ip.as_deref() == Some(e.as_str());
                    ui.label(RichText::new(format!("IP di uscita: {e}")).color(if same { RED } else { GREEN }))
                }
            };
        });

        ui.separator();
        ui.heading("Dati");
        ui.horizontal(|ui| {
            ui.label("Cartella");
            let dir = self.data_dir();
            ui.label(RichText::new(&dir).monospace());
            if ui.small_button("copia").clicked() {
                ui.ctx().copy_text(dir);
            }
        });
        ui.label(RichText::new("sources.json, proxies.json, candidates.json, history.json, settings.json. Variabile PROXY_FINDER_HOME per usarne un'altra.").small().weak());

        ui.separator();
        ui.heading("Avvertenze");
        ui.label("I proxy pubblici vedono i domini che visiti e, in http, anche il contenuto. Non usarli per banche, password o pagamenti.");
        ui.label("Con i proxy SOCKS la risoluzione DNS può avvenire in locale.");
        ui.label(RichText::new(format!("proxy-finder {}", proxy_finder::version())).small().weak());
    }
}
