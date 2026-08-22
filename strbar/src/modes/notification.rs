use crate::config::Config;
use eframe::egui;

/// Draws a notification in the pill, matching the original styling: a small
/// accent "source" label on top and a centered headline below, with the
/// headline truncated to fit the pill width.
#[allow(clippy::too_many_arguments)]
pub fn draw_notification_mode(
    painter: &egui::Painter,
    pill_rect: egui::Rect,
    cfg: &Config,
    accent: egui::Color32,
    small: &str,
    big: &str,
    alpha: f32,
) {
    if alpha <= 0.01 {
        return;
    }
    let center = pill_rect.center() + egui::vec2(0.0, cfg.text_offset_y);
    let label_alpha = ((255.0 * 0.55) * alpha).round() as u8;
    let title_alpha = (255.0 * alpha).round() as u8;

    // Small source label (e.g. "<username> on Discord").
    painter.text(
        egui::pos2(center.x, center.y - 8.5),
        egui::Align2::CENTER_CENTER,
        small,
        egui::FontId::new(cfg.font_size - 5.0, egui::FontFamily::Proportional),
        egui::Color32::from_rgba_premultiplied(accent.r(), accent.g(), accent.b(), label_alpha),
    );

    // Big headline (message body), truncated to fit the pill and vertically
    // centered just below the label.
    let font = egui::FontId::new(cfg.font_size + 1.0, egui::FontFamily::Proportional);
    let inner = pill_rect.shrink2(egui::vec2(22.0, 0.0));
    let max_w = inner.width();
    let truncated = truncate_to_width(painter, big, &font, max_w);
    let galley = painter.layout_no_wrap(truncated, font, egui::Color32::from_rgba_premultiplied(255, 255, 255, title_alpha));
    let clipped = painter.with_clip_rect(inner);
    clipped.galley(
        egui::pos2(center.x - galley.size().x * 0.5, center.y + 8.0 - galley.size().y * 0.5),
        galley,
        egui::Color32::TRANSPARENT,
    );
}

/// Truncates `text` (adding an ellipsis) so its rendered width fits `max_w`.
fn truncate_to_width(painter: &egui::Painter, text: &str, font: &egui::FontId, max_w: f32) -> String {
    if max_w <= 0.0 {
        return String::new();
    }
    let measure = |s: &str| painter.layout_no_wrap(s.to_owned(), font.clone(), egui::Color32::WHITE).size().x;

    if measure(text) <= max_w {
        return text.to_string();
    }

    // Binary-search the longest prefix that fits, minus room for the ellipsis.
    let ellipsis = "…";
    let ellipsis_w = measure(ellipsis);
    let mut lo = 0usize;
    let mut hi = text.chars().count();
    let mut best = 0usize;
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let prefix: String = text.chars().take(mid).collect();
        if measure(&prefix) + ellipsis_w <= max_w {
            best = mid;
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let mut out: String = text.chars().take(best).collect();
    out.push('…');
    out
}
