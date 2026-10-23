//! proxy-finder: scarica liste di proxy pubblici, le prova e tiene i buoni.
//! La GUI (eframe) sta in `src/main.rs` e `src/ui/`; qui c'è solo la logica.

pub mod countries;
pub mod job;
pub mod model;
pub mod parse;
pub mod rating;
pub mod relay;
pub mod sources;
pub mod state;
pub mod store;
pub mod tester;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
