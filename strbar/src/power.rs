use crate::config::Config;
use eframe::egui;

/// Index of the default-selected action ("Reboot", matching the sample UI).
const ACTION_REBOOT_DEFAULT: usize = 3;

const LABELS: [&str; 5] = ["Lock", "Suspend", "Log Out", "Reboot", "Power Off"];

pub struct PowerMenuState {
    pub open: bool,
    pub anim: f32,
    pub selected: usize,
}

impl PowerMenuState {
    pub fn new() -> Self {
        Self {
            open: false,
            anim: 0.0,
            selected: ACTION_REBOOT_DEFAULT,
        }
    }

    pub fn open_menu(&mut self) {
        self.open = true;
        self.selected = ACTION_REBOOT_DEFAULT;
    }

    pub fn close(&mut self) {
        self.open = false;
    }
}

pub const POWER_MENU_HEIGHT: f32 = 132.0;

/// Draws a GNOME-style session menu (a rounded container with five buttons).
/// Returns the action the user chose, if any (Escape is handled by the caller).
pub fn draw_power_menu(
    ui: &egui::Ui,
    panel_rect: egui::Rect,
    cfg: &Config,
    highlight: egui::Color32,
    icons: &[egui::TextureHandle; 5],
    alpha: f32,
    selected: &mut usize,
) -> Option<u32> {
    if alpha <= 0.01 {
        return None;
    }
    let a = (alpha * 255.0).round() as u8;

    let btn_h = panel_rect.height() - 24.0;
    let btn_w = (panel_rect.width() - 16.0 * 6.0) / 5.0;

    let panel_bg = Config::parse_color(&cfg.background);
    let panel = egui::Color32::from_rgba_premultiplied(
        panel_bg.r(),
        panel_bg.g(),
        panel_bg.b(),
        (panel_bg.a() as f32 * alpha).round() as u8,
    );
    let shadow = egui::Color32::from_black_alpha((70.0 * alpha).round() as u8);
    ui.painter().rect_filled(
        panel_rect.translate(egui::vec2(0.0, 6.0 * alpha)),
        egui::CornerRadius::same((panel_rect.height() / 2.0).round() as u8),
        shadow,
    );
    ui.painter().rect_filled(
        panel_rect,
        egui::CornerRadius::same((panel_rect.height() / 2.0).round() as u8),
        panel,
    );

    // Keyboard navigation.
    if ui.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
        *selected = (*selected + 1) % 5;
    }
    if ui.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
        *selected = (*selected + 4) % 5;
    }

    let mut triggered: Option<u32> = None;

    for i in 0..5 {
        let bx = panel_rect.left() + 16.0 + i as f32 * (btn_w + 16.0);
        let btn_rect = egui::Rect::from_min_size(
            egui::pos2(bx, panel_rect.top() + 12.0),
            egui::vec2(btn_w, btn_h),
        );
        let response =
            ui.interact(btn_rect, ui.make_persistent_id(("powerbtn", i)), egui::Sense::click());

        let hovered = response.hovered() || *selected == i;
        let (fg, bg) = if hovered {
            (
                egui::Color32::from_rgb(18, 18, 18),
                egui::Color32::from_rgba_premultiplied(
                    highlight.r(),
                    highlight.g(),
                    highlight.b(),
                    255,
                ),
            )
        } else {
            (egui::Color32::from_rgb(225, 225, 225), egui::Color32::from_rgb(28, 28, 30))
        };

        ui.painter().rect_filled(btn_rect, egui::CornerRadius::same(22), bg);

        let icon = &icons[i];
        let icon_size = 26.0;
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(btn_rect.center().x, btn_rect.top() + 30.0),
            egui::vec2(icon_size, icon_size),
        );
        ui.painter().image(
            icon.id(),
            icon_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::from_rgba_premultiplied(fg.r(), fg.g(), fg.b(), a),
        );

        ui.painter().text(
            egui::pos2(btn_rect.center().x, btn_rect.bottom() - 22.0),
            egui::Align2::CENTER_CENTER,
            LABELS[i],
            egui::FontId::new(cfg.font_size - 2.0, egui::FontFamily::Proportional),
            egui::Color32::from_rgba_premultiplied(fg.r(), fg.g(), fg.b(), a),
        );

        if response.hovered() {
            *selected = i;
        }
        let clicked = response.clicked()
            || (ui.input(|i| i.key_pressed(egui::Key::Enter)) && *selected == i);
        if clicked {
            triggered = Some(i as u32);
        }
    }

    triggered
}
