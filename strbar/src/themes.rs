use crate::config::Config;
use crate::win;
use eframe::egui;

pub struct ThemesState {
    pub open: bool,
    pub anim: f32,
    themes: Vec<win::ThemeEntry>,
    loaded: bool,
}

impl ThemesState {
    pub fn new() -> Self {
        Self {
            open: false,
            anim: 0.0,
            themes: Vec::new(),
            loaded: false,
        }
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn refresh(&mut self) {
        self.themes = win::list_themes();
        self.loaded = true;
    }

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
        let gap = 10.0;

        let title_a = (alpha * 2.5).min(1.0);
        let title_alpha = (title_a * 255.0).round() as u8;
        ui.painter().text(
            egui::pos2(rect.left() + pad, rect.top() + pad + 8.0),
            egui::Align2::LEFT_CENTER,
            "Themes",
            egui::FontId::new(15.0, egui::FontFamily::Proportional),
            egui::Color32::from_rgba_premultiplied(accent.r(), accent.g(), accent.b(), title_alpha),
        );

        if self.themes.is_empty() {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "No themes found\nAdd .toml files to ~/.strland/strbar/themes/",
                egui::FontId::new(13.0, egui::FontFamily::Proportional),
                egui::Color32::from_rgb(160, 160, 160),
            );
            return None;
        }

        let cols = 2;
        let card_w = (rect.width() - pad * 2.0 - gap * (cols - 1) as f32) / cols as f32;
        let card_h = 72.0;
        let start_y = rect.top() + pad + 30.0;

        let mut selected: Option<String> = None;
        for (i, theme) in self.themes.iter().enumerate() {
            let row = i / cols;
            let col = i % cols;
            let x = rect.left() + pad + col as f32 * (card_w + gap);
            let y = start_y + row as f32 * (card_h + gap);
            if y + card_h > rect.bottom() - pad {
                break;
            }

            let card = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(card_w, card_h));
            let resp = ui.interact(card, ui.make_persistent_id(("theme_card", &theme.file_name)), egui::Sense::click());
            let hovered = resp.hovered();

            // Card background
            let card_bg = if hovered {
                egui::Color32::from_rgb(42, 42, 44)
            } else {
                egui::Color32::from_rgb(28, 28, 30)
            };
            ui.painter().rect_filled(card, egui::CornerRadius::same(16), card_bg);

            // Color swatch: accent_color on left, background on right
            let swatch_h = 28.0;
            let swatch_y = card.top() + 12.0;
            let swatch_w = (card_w - pad) / 2.0 - 2.0;

            // Accent color swatch
            let accent_swatch = egui::Rect::from_min_size(
                egui::pos2(card.left() + pad / 2.0, swatch_y),
                egui::vec2(swatch_w, swatch_h),
            );
            let accent_color = theme.accent_color.as_deref()
                .map(Config::parse_color)
                .unwrap_or(accent);
            ui.painter().rect_filled(accent_swatch, egui::CornerRadius::same(6), accent_color);

            // Background swatch
            let bg_swatch = egui::Rect::from_min_size(
                egui::pos2(accent_swatch.right() + 4.0, swatch_y),
                egui::vec2(swatch_w, swatch_h),
            );
            let bg_color = theme.background.as_deref()
                .map(Config::parse_color)
                .unwrap_or(egui::Color32::from_rgb(9, 9, 9));
            ui.painter().rect_filled(bg_swatch, egui::CornerRadius::same(6), bg_color);

            // Theme name
            ui.painter().text(
                egui::pos2(card.left() + pad / 2.0, card.bottom() - 14.0),
                egui::Align2::LEFT_CENTER,
                &theme.display_name,
                egui::FontId::new(12.0, egui::FontFamily::Proportional),
                egui::Color32::from_rgb(220, 220, 220),
            );

            // Wallpaper indicator
            if theme.wallpaper.is_some() {
                ui.painter().text(
                    egui::pos2(card.right() - pad / 2.0, card.bottom() - 14.0),
                    egui::Align2::RIGHT_CENTER,
                    "wp",
                    egui::FontId::new(10.0, egui::FontFamily::Proportional),
                    egui::Color32::from_rgb(120, 120, 120),
                );
            }

            if resp.clicked() {
                selected = Some(theme.file_name.clone());
            }
        }

        selected
    }
}
