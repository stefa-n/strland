use crate::config::Config;
use crate::win;
use eframe::egui;

/// State for the Alt+Tab app switcher overlay.
pub struct SwitcherState {
    pub active: bool,
    pub anim: f32,
    pub index: usize,
    windows: Vec<win::SwitcherWindow>,
    icons: Vec<egui::TextureHandle>,
}

impl SwitcherState {
    pub fn new() -> Self {
        Self { active: false, anim: 0.0, index: 0, windows: Vec::new(), icons: Vec::new() }
    }

    pub fn begin(&mut self, ctx: &egui::Context) {
        self.active = true;
        self.anim = 0.0;
        self.index = 0;
        self.windows = win::list_switch_windows();
        self.load_icons(ctx);
    }

    pub fn advance(&mut self, delta: isize) {
        if self.windows.is_empty() {
            self.index = 0;
            return;
        }
        let n = self.windows.len() as isize;
        self.index = ((self.index as isize + delta).rem_euclid(n)) as usize;
    }

    pub fn activate_selected(&mut self) {
        if let Some(w) = self.windows.get(self.index) {
            win::activate_switch_window(w.hwnd);
        }
        self.active = false;
    }

    pub fn dismiss(&mut self) {
        self.active = false;
    }

    fn load_icons(&mut self, ctx: &egui::Context) {
        self.icons.clear();
        for w in &self.windows {
            let pix = win::extract_app_icon_for_window(w.hwnd);
            let tex = pix.and_then(|px| {
                let img = egui::ColorImage::from_rgba_unmultiplied([px.width.max(1), px.height.max(1)], &px.rgba);
                Some(ctx.load_texture("switch-icon", img, egui::TextureOptions::LINEAR))
            });
            self.icons.push(tex.unwrap_or_else(|| placeholder_icon(ctx)));
        }
    }

    /// Draws the switcher grid inside `rect`, scaling card sizes to fit so the
    /// grid never overflows the panel (no bottom clipping).
    pub fn draw(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        cfg: &Config,
        accent: egui::Color32,
        alpha: f32,
    ) -> Option<usize> {
        let n = self.windows.len();
        if n == 0 || alpha <= 0.01 {
            return None;
        }
        let a = (alpha * 255.0).round() as u8;
        let bg = Config::parse_color(&cfg.background);
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(30),
            egui::Color32::from_rgba_premultiplied(bg.r(), bg.g(), bg.b(), a),
        );

        let pad = 16.0;
        let gap = 12.0;
        let inner_w = rect.width() - pad * 2.0;
        let inner_h = rect.height() - pad * 2.0;

        // Column count driven by width; card width fills the row.
        let cols = ((inner_w + gap) / 150.0).floor().max(1.0) as usize;
        let rows = (n + cols - 1) / cols;
        let card_w = (inner_w - (cols - 1) as f32 * gap) / cols as f32;
        // Card height fills the panel height so nothing clips off the bottom.
        let card_h = ((inner_h - (rows - 1) as f32 * gap) / rows as f32).clamp(70.0, 150.0);
        let real_w = cols as f32 * card_w + (cols - 1) as f32 * gap;
        let real_h = rows as f32 * card_h + (rows - 1) as f32 * gap;
        let start_x = rect.center().x - real_w / 2.0;
        let start_y = rect.center().y - real_h / 2.0;

        for i in 0..n {
            let row = i / cols;
            let col = i % cols;
            let x = start_x + col as f32 * (card_w + gap);
            let y = start_y + row as f32 * (card_h + gap);
            let card = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(card_w, card_h));

            let selected = i == self.index;
            let resp = ui.interact(card, ui.make_persistent_id(("sw", i)), egui::Sense::click());
            let hovered = resp.hovered();

            let bgc = if selected {
                egui::Color32::from_rgba_premultiplied(accent.r(), accent.g(), accent.b(), a)
            } else if hovered {
                egui::Color32::from_rgb(44, 44, 46)
            } else {
                egui::Color32::from_rgb(30, 30, 32)
            };
            ui.painter().rect_filled(card, egui::CornerRadius::same(18), bgc);

            // Icon (scaled to fit card).
            let icon_side = (card_h * 0.5).min(52.0).max(30.0);
            let icon_rect = egui::Rect::from_center_size(
                card.center(),
                egui::vec2(icon_side, icon_side),
            );
            if let Some(icon) = self.icons.get(i) {
                ui.painter().image(
                    icon.id(),
                    icon_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }

            let mut act_on_click = None;
            if resp.clicked() {
                self.index = i;
                // Double-click switches to the app immediately.
                if resp.double_clicked() {
                    act_on_click = Some(i);
                }
            }
            if let Some(sel) = act_on_click {
                return Some(sel);
            }
        }
        None
    }
}

fn placeholder_icon(ctx: &egui::Context) -> egui::TextureHandle {
    let px: Vec<u8> = vec![255; 40 * 40 * 4];
    let img = egui::ColorImage::from_rgba_unmultiplied([40, 40], &px);
    ctx.load_texture("switch-placeholder", img, egui::TextureOptions::LINEAR)
}
