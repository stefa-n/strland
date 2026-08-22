use eframe::egui;

#[allow(clippy::too_many_arguments)]
pub fn draw_volume_mode(
    painter: &egui::Painter,
    pill_rect: egui::Rect,
    accent: egui::Color32,
    level: f32,
    alpha: f32,
    muted: bool,
    icon_on: &egui::TextureHandle,
    icon_off: &egui::TextureHandle,
) {
    if alpha <= 0.01 {
        return;
    }
    let center = pill_rect.center();

    let icon_size = (pill_rect.height() - 12.0).min(24.0);
    let icon_rect = egui::Rect::from_min_size(
        egui::pos2(pill_rect.left() + 16.0, center.y - icon_size / 2.0),
        egui::vec2(icon_size, icon_size),
    );
    let tint = egui::Color32::from_rgba_premultiplied(
        accent.r(),
        accent.g(),
        accent.b(),
        (255.0 * alpha).round() as u8,
    );
    let tex = if muted { icon_off } else { icon_on };
    painter.image(
        tex.id(),
        icon_rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        tint,
    );

    let bar_left = icon_rect.right() + 10.0;
    let bar_right = pill_rect.right() - 16.0;
    let bar_w = (bar_right - bar_left).max(20.0);
    let bar_h = 4.0;
    let bar_rect = egui::Rect::from_min_size(
        egui::pos2(bar_left, center.y - bar_h / 2.0),
        egui::vec2(bar_w, bar_h),
    );
    let bar_corner = egui::CornerRadius::same(2);
    let track_alpha = (60.0 * alpha).round() as u8;
    painter.rect_filled(bar_rect, bar_corner, egui::Color32::from_rgba_premultiplied(60, 60, 60, track_alpha));

    let fill_w = bar_rect.width() * level.clamp(0.0, 1.0);
    if fill_w > 0.0 {
        let fill_rect = egui::Rect::from_min_size(bar_rect.min, egui::vec2(fill_w, bar_h));
        let fill_alpha = (255.0 * alpha).round() as u8;
        let fill_color = egui::Color32::from_rgba_premultiplied(accent.r(), accent.g(), accent.b(), fill_alpha);
        painter.rect_filled(fill_rect, bar_corner, fill_color);
    }
}
