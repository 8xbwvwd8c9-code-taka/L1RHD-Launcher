//! 背景與元件

use eframe::egui;

pub fn draw_background(ui: &mut egui::Ui) {
    ui.painter().rect_filled(
        ui.max_rect(),
        0.0,
        egui::Color32::from_rgb(214, 196, 170),
    );
}