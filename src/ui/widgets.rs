//! 背景圖與元件

use eframe::egui;

static BG: std::sync::OnceLock<Option<egui::TextureHandle>> = std::sync::OnceLock::new();

pub fn install_bg(ctx: &egui::Context) {
    BG.get_or_init(|| {
        let bytes: &[u8] = include_bytes!("../../assets/bg.jpg");
        match image::load_from_memory(bytes) {
            Ok(d) => {
                let rgba = d.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let img = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                Some(ctx.load_texture("bg", img, egui::TextureOptions::LINEAR))
            }
            Err(_) => None,
        }
    });
}

/// 全屏繪製背景圖：等比放大到覆蓋整個視窗，不留黑邊。
pub fn draw_background(ui: &mut egui::Ui) {
    let rect = ui.max_rect();
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(214, 196, 170));

    if let Some(Some(tex)) = BG.get() {
        let ts = tex.size_vec2();
        if ts.x > 0.0 && ts.y > 0.0 {
            let scale = (rect.width() / ts.x).max(rect.height() / ts.y);
            let draw_size = ts * scale;
            let center = rect.center();
            let min = center - draw_size * 0.5;
            let draw_rect = egui::Rect::from_min_size(min, draw_size);

            let u0 = (rect.left() - draw_rect.left()) / draw_size.x;
            let u1 = (rect.right() - draw_rect.left()) / draw_size.x;
            let v0 = (rect.top() - draw_rect.top()) / draw_size.y;
            let v1 = (rect.bottom() - draw_rect.top()) / draw_size.y;

            let mut mesh = egui::Mesh::with_texture(tex.id());
            mesh.add_rect_with_uv(
                rect,
                egui::Rect::from_min_max(egui::pos2(u0, v0), egui::pos2(u1, v1)),
                egui::Color32::WHITE,
            );
            painter.add(mesh);
        }

        let w = rect.width();
        let shade_w = (w * 0.42).min(380.0);
        let shade = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.min.x + shade_w, rect.max.y),
        );
        let mut m = egui::Mesh::default();
        let c0 = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 120);
        let c1 = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 0);
        let i = m.vertices.len() as u32;
        m.vertices.push(egui::epaint::Vertex { pos: shade.left_top(), uv: egui::Pos2::ZERO, color: c0 });
        m.vertices.push(egui::epaint::Vertex { pos: shade.right_top(), uv: egui::Pos2::ZERO, color: c1 });
        m.vertices.push(egui::epaint::Vertex { pos: shade.right_bottom(), uv: egui::Pos2::ZERO, color: c1 });
        m.vertices.push(egui::epaint::Vertex { pos: shade.left_bottom(), uv: egui::Pos2::ZERO, color: c0 });
        m.indices.extend_from_slice(&[i, i + 1, i + 2, i, i + 2, i + 3]);
        painter.add(m);
    }
}
