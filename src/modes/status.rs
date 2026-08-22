use crate::config::Config;
use crate::win;
use eframe::egui;

/// Draws the system-status indicators on the right side of the pill
/// (Wi-Fi signal, Bluetooth, battery), each an SVG tinted with the accent.
/// When `hover` is true, the icons are gathered inside a single round button.
/// Returns `true` if the button was clicked (opens the control center).
#[allow(clippy::too_many_arguments)]
pub fn draw_status_icons(
    painter: &egui::Painter,
    pill_rect: egui::Rect,
    cfg: &Config,
    accent: egui::Color32,
    status: &win::SystemStatus,
    wifi: &egui::TextureHandle,
    bt: &egui::TextureHandle,
    battery: &egui::TextureHandle,
    alpha: f32,
    hover: bool,
) -> bool {
    if alpha <= 0.01 {
        return false;
    }
    let icon = 15.0;
    let gap = 8.0;
    let right = pill_rect.right() - 16.0;
    let cy = pill_rect.center().y;

    let mut items: Vec<(f32, &egui::TextureHandle)> = Vec::new();

    if cfg.show_wifi && status.wifi_connected {
        items.push((status.wifi_signal as f32 / 100.0, wifi));
    }
    if cfg.show_bluetooth && status.bluetooth_connected {
        items.push((1.0, bt));
    }
    if cfg.show_battery && status.battery_present {
        let level = if status.battery_charging {
            1.0
        } else {
            status.battery_percent as f32 / 100.0
        };
        items.push((level, battery));
    }

    if items.is_empty() {
        return false;
    }

    let pad_x = 12.0;
    let pad_y = 7.0;
    let total = items.len() as f32 * icon + (items.len() - 1) as f32 * gap;
    let btn_size = egui::vec2(total + pad_x * 2.0, icon + pad_y * 2.0);
    let btn_center = egui::pos2(right - btn_size.x / 2.0, cy);
    let btn_rect = egui::Rect::from_center_size(btn_center, btn_size);

    if hover {
        let button_bg = Config::parse_color(&cfg.status_button_background);
        let button_alpha = (button_bg.a() as f32 * alpha).round() as u8;
        let bg = egui::Color32::from_rgba_premultiplied(
            button_bg.r(),
            button_bg.g(),
            button_bg.b(),
            button_alpha,
        );
        painter.rect_filled(
            btn_rect,
            egui::CornerRadius::same((btn_size.y / 2.0) as u8),
            bg,
        );
    }

    let mut x = btn_center.x - total / 2.0;

    for (level, tex) in items {
        let tint = egui::Color32::from_rgba_premultiplied(
            accent.r(),
            accent.g(),
            accent.b(),
            (255.0 * alpha).round() as u8,
        );
        let rect = egui::Rect::from_min_size(
            egui::pos2(x, cy - icon / 2.0),
            egui::vec2(icon, icon),
        );
        painter.image(
            tex.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            tint,
        );

        if std::ptr::eq(tex, battery) {
            let level = level.clamp(0.0, 1.0);
            let inner = rect.shrink2(egui::vec2(2.0, 4.0));
            let fill_w = inner.width() * level;
            if fill_w > 1.0 {
                let fill_rect = egui::Rect::from_min_max(
                    inner.min,
                    egui::pos2(inner.left() + fill_w, inner.bottom()),
                );
                painter.rect_filled(fill_rect, egui::CornerRadius::same(1), tint);
            }
        }

        x += icon + gap;
    }

    // Clickable button region that opens the control center.
    painter.ctx().input(|i| {
        i.pointer.any_pressed()
            && i.pointer.interact_pos().map(|p| btn_rect.contains(p)).unwrap_or(false)
    })
}
