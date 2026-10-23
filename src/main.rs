mod cli;
#[cfg(target_os = "linux")]
mod tray;
mod ui;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() && args[0] != "gui" {
        std::process::exit(cli::run(args));
    }
    if let Err(e) = gui() {
        eprintln!("errore: GUI non avviata: {e}\nSenza display usa la riga di comando: proxy-finder help");
        std::process::exit(1);
    }
}

fn gui() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 750.0])
            .with_min_inner_size([760.0, 480.0])
            .with_title("Proxy Finder"),
        ..Default::default()
    };
    eframe::run_native("proxy-finder", options, Box::new(|cc| Ok(Box::new(ui::App::new(cc)))))
}
