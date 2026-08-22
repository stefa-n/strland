use crate::config::Config;
use eframe::egui;

#[allow(clippy::too_many_arguments)]
pub fn draw_notification_mode(
    painter: &egui::Painter,
    pill_rect: egui::Rect,
    cfg: &Config,
    accent: egui::Color32,
    title: &str,
    alpha: f32,
) {
    if alpha <= 0.01 {
        return;
    }
    let center = pill_rect.center() + egui::vec2(0.0, cfg.text_offset_y);
    let label_alpha = ((255.0 * 0.55) * alpha).round() as u8;
    let title_alpha = (255.0 * alpha).round() as u8;

    painter.text(
        egui::pos2(center.x, center.y - 8.5),
        egui::Align2::CENTER_CENTER,
        "Now Playing",
        egui::FontId::new(cfg.font_size - 5.0, egui::FontFamily::Proportional),
        egui::Color32::from_rgba_premultiplied(accent.r(), accent.g(), accent.b(), label_alpha),
    );

    let font = egui::FontId::new(cfg.font_size + 1.0, egui::FontFamily::Proportional);
    let galley = painter.layout_no_wrap(
        title.to_owned(),
        font,
        egui::Color32::from_rgba_premultiplied(255, 255, 255, title_alpha),
    );
    let inner = pill_rect.shrink2(egui::vec2(22.0, 0.0));
    let clipped = painter.with_clip_rect(inner);
    clipped.galley(
        egui::pos2(center.x - galley.size().x * 0.5, center.y + 8.0 - galley.size().y * 0.5),
        galley,
        egui::Color32::TRANSPARENT,
    );
}
