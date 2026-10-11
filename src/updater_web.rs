//! The web version's stand-in for updater.rs: the page always loads the newest version, so
//! there's nothing to check or install.

use eframe::egui;

use crate::tr;

pub struct Updates;

impl Updates {
    pub fn new() -> Self {
        Self
    }

    pub fn check(&mut self, _ctx: &egui::Context) {}

    pub fn poll(&mut self, _ctx: &egui::Context, _auto_install: bool) {}

    pub fn dialog_open(&self) -> bool {
        false
    }

    pub fn close_dialog(&mut self) {}

    pub fn status_bar(&mut self, _ui: &mut egui::Ui) {}

    pub fn dialog(&mut self, _ctx: &egui::Context) {}

    pub fn settings(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new(tr!("Version {version}", version = env!("CARGO_PKG_VERSION"))).strong());
    }
}

pub fn clean_up() {}
