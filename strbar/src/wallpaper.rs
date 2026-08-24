use crate::config::Config;
use crate::win;
use eframe::egui;

/// State for the wallpaper picker panel.
pub struct WallpaperState {
    pub open: bool,
    pub anim: f32,
    wallpapers: Vec<String>,
    current: Option<String>,
    loaded: bool,
}

impl WallpaperState {
    pub fn new() -> Self {
        Self { open: false, anim: 0.0, wallpapers: Vec::new(), current: None, loaded: false }
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    /// Marks `name` as the current selection without re-scanning the folder.
    pub fn refresh_after_selection(&mut self, name: &str) {
        self.current = Some(name.to_string());
    }

    fn refresh(&mut self) {
        self.wallpapers = win::list_wallpapers();
        self.current = win::current_wallpaper();
        self.loaded = true;
    }

    /// Draws the wallpaper picker inside `rect`. Returns the selected name, if
    /// the user clicked one.
    pub fn draw(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        cfg: &Config,
        accent: egui::Color32,
        alpha: f32,
    ) -> Option<String> {
        if !self.loaded {
            self.refresh();
        }
        if alpha <= 0.01 {
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

        // Title row.
        ui.painter().text(
            egui::pos2(rect.left() + pad, rect.top() + pad + 8.0),
            egui::Align2::LEFT_CENTER,
            "Wallpaper",
            egui::FontId::new(15.0, egui::FontFamily::Proportional),
            egui::Color32::from_rgba_premultiplied(accent.r(), accent.g(), accent.b(), a),
        );

        if self.wallpapers.is_empty() {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "No wallpapers found in the strpaper folder",
                egui::FontId::new(13.0, egui::FontFamily::Proportional),
                egui::Color32::from_rgb(160, 160, 160),
            );
            return None;
        }

        // Grid of wallpaper cards.
        let cols = 3;
        let card_w = (rect.width() - pad * 2.0 - gap * (cols - 1) as f32) / cols as f32;
        let card_h = 96.0;
        let start_y = rect.top() + pad + 30.0;

        let mut selected: Option<String> = None;
        for (i, name) in self.wallpapers.iter().enumerate() {
            let row = i / cols;
            let col = i % cols;
            let x = rect.left() + pad + col as f32 * (card_w + gap);
            let y = start_y + row as f32 * (card_h + gap);
            if y + card_h > rect.bottom() - pad {
                break; // no overflow
            }
            let card = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(card_w, card_h));
            let resp = ui.interact(card, ui.make_persistent_id(("wp", i)), egui::Sense::click());

            let is_current = self.current.as_deref() == Some(name.as_str());
            let bgc = if is_current {
                egui::Color32::from_rgba_premultiplied(accent.r(), accent.g(), accent.b(), a)
            } else if resp.hovered() {
                egui::Color32::from_rgb(44, 44, 46)
            } else {
                egui::Color32::from_rgb(30, 30, 32)
            };
            ui.painter().rect_filled(card, egui::CornerRadius::same(16), bgc);

            let fg = if is_current {
                egui::Color32::from_rgb(18, 18, 18)
            } else {
                egui::Color32::from_rgb(225, 225, 225)
            };

            // File name (stem), clipped.
            let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
            let font = egui::FontId::new(13.0, egui::FontFamily::Proportional);
            let galley = ui.painter().layout_no_wrap(stem.to_owned(), font, fg);
            let clip = egui::Rect::from_min_size(
                egui::pos2(card.left() + 8.0, card.center().y - galley.size().y / 2.0),
                egui::vec2(card.width() - 16.0, galley.size().y + 4.0),
            );
            let clipped = ui.painter().with_clip_rect(clip);
            clipped.galley(
                egui::pos2(card.left() + 8.0, clip.center().y - galley.size().y / 2.0),
                galley,
                egui::Color32::TRANSPARENT,
            );

            if is_current {
                // Small "current" dot.
                ui.painter().circle_filled(
                    egui::pos2(card.right() - 14.0, card.center().y),
                    4.0,
                    fg,
                );
            }

            if resp.clicked() {
                selected = Some(name.clone());
            }
        }

        selected
    }
}
