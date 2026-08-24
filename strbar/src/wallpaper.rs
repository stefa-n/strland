use crate::config::Config;
use crate::win;
use eframe::egui;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;

const VIDEO_EXTS: &[&str] = &["mp4", "webm", "mov", "avi", "mkv"];

fn is_video(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    VIDEO_EXTS.contains(&ext.to_ascii_lowercase().as_str())
}

fn load_thumbnail_data(path: &PathBuf) -> Option<(Vec<u8>, u32, u32)> {
    if is_video(&path.to_string_lossy()) {
        load_video_first_frame(path)
    } else {
        load_image_thumbnail(path)
    }
}

fn load_image_thumbnail(path: &PathBuf) -> Option<(Vec<u8>, u32, u32)> {
    let img = image::open(path).ok()?;
    let img = img.thumbnail(256, 256);
    let rgba = img.to_rgba8().into_raw();
    Some((rgba, img.width(), img.height()))
}

fn load_video_first_frame(path: &PathBuf) -> Option<(Vec<u8>, u32, u32)> {
    let path_str = path.to_str()?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let tmp = std::env::temp_dir().join(format!("strland_thumb_{stem}.png"));
    let tmp_str = tmp.to_str()?;
    let output = std::process::Command::new("ffmpeg")
        .args(["-y", "-i", path_str, "-vframes", "1", "-q:v", "2", tmp_str])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    let img = image::open(&tmp).ok()?;
    let _ = std::fs::remove_file(&tmp);
    let img = img.thumbnail(256, 256);
    let rgba = img.to_rgba8().into_raw();
    Some((rgba, img.width(), img.height()))
}

enum Thumbnail {
    Loading,
    Ready(Option<egui::TextureHandle>),
}

pub struct WallpaperState {
    pub open: bool,
    pub anim: f32,
    wallpapers: Vec<String>,
    current: Option<String>,
    loaded: bool,
    thumbnails: HashMap<String, Thumbnail>,
    thumb_rx: mpsc::Receiver<(String, Option<(Vec<u8>, u32, u32)>)>,
    thumb_tx: mpsc::Sender<(String, Option<(Vec<u8>, u32, u32)>)>,
}

impl WallpaperState {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            open: false,
            anim: 0.0,
            wallpapers: Vec::new(),
            current: None,
            loaded: false,
            thumbnails: HashMap::new(),
            thumb_rx: rx,
            thumb_tx: tx,
        }
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn refresh_after_selection(&mut self, name: &str) {
        self.current = Some(name.to_string());
    }

    pub fn refresh(&mut self) {
        self.wallpapers = win::list_wallpapers();
        self.current = win::current_wallpaper();
        self.loaded = true;
        self.thumbnails.clear();
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
        let gap = 12.0;

        // Title fades out earlier (at ~40% panel alpha) for a cleaner close.
        let title_a = (alpha * 2.5).min(1.0);
        let title_alpha = (title_a * 255.0).round() as u8;
        ui.painter().text(
            egui::pos2(rect.left() + pad, rect.top() + pad + 8.0),
            egui::Align2::LEFT_CENTER,
            "Wallpaper",
            egui::FontId::new(15.0, egui::FontFamily::Proportional),
            egui::Color32::from_rgba_premultiplied(accent.r(), accent.g(), accent.b(), title_alpha),
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

        // Drain completed thumbnail loads and create textures.
        while let Ok((name, data)) = self.thumb_rx.try_recv() {
            win::debug_log(&match &data {
                Some((_, w, h)) => format!("[wp] loaded thumb '{}' {}x{}", name, w, h),
                None => format!("[wp] FAILED thumb '{}'", name),
            });
            let tex = data.and_then(|(rgba, w, h)| {
                let color_image = egui::ColorImage::from_rgba_unmultiplied(
                    [w as usize, h as usize],
                    &rgba,
                );
                let handle = ui.ctx().load_texture(
                    format!("wp_thumb_{name}"),
                    color_image,
                    egui::TextureOptions::LINEAR,
                );
                Some(handle)
            });
            self.thumbnails.insert(name, Thumbnail::Ready(tex));
        }

        // Start loading missing thumbnails in background threads.
        let dir = win::wallpaper_dir();
        for name in &self.wallpapers {
            match self.thumbnails.get(name) {
                Some(Thumbnail::Loading) | Some(Thumbnail::Ready(_)) => continue,
                _ => {}
            }
            self.thumbnails.insert(name.clone(), Thumbnail::Loading);
            let path = dir.join(name);
            let tx = self.thumb_tx.clone();
            let name_clone = name.clone();
            std::thread::spawn(move || {
                let data = load_thumbnail_data(&path);
                let _ = tx.send((name_clone, data));
            });
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
                break;
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

            // Draw thumbnail if available.
            let has_thumb = matches!(
                self.thumbnails.get(name),
                Some(Thumbnail::Ready(Some(_)))
            );
            if has_thumb {
                if let Some(Thumbnail::Ready(Some(tex))) = self.thumbnails.get(name) {
                    let tex_size = tex.size_vec2();
                    let inner = card.shrink(3.0);
                    let scale =
                        (inner.width() / tex_size.x).min(inner.height() / tex_size.y);
                    let scaled = tex_size * scale;
                    let img_rect =
                        egui::Rect::from_center_size(inner.center(), scaled);
                    ui.painter().image(
                        tex.id(),
                        img_rect,
                        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );

                    // Rounded corner masks: draw4 quarter-circle sectors in
                    // the card background colour so the image corners don't
                    // poke out of the card's 16 px rounded corners.
                    let cr = 16.0_f32;
                    let steps = 8;
                    corner_mask(ui.painter(), card.min, 1.0, 1.0, cr, bgc, steps);
                    corner_mask(ui.painter(), egui::pos2(card.right(), card.top()), -1.0, 1.0, cr, bgc, steps);
                    corner_mask(ui.painter(), egui::pos2(card.left(), card.bottom()), 1.0, -1.0, cr, bgc, steps);
                    corner_mask(ui.painter(), card.max, -1.0, -1.0, cr, bgc, steps);
                }
            }

            let fg = if is_current {
                egui::Color32::from_rgb(18, 18, 18)
            } else {
                egui::Color32::from_rgb(225, 225, 225)
            };

            // File name (stem) — always on top.
            let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
            let font = egui::FontId::new(13.0, egui::FontFamily::Proportional);
            let galley = ui.painter().layout_no_wrap(stem.to_owned(), font, fg);

            let name_y = if has_thumb {
                card.bottom() - 4.0 - galley.size().y
            } else {
                card.center().y - galley.size().y / 2.0
            };
            let name_x = card.left() + 8.0;

            if has_thumb {
                let strip = egui::Rect::from_min_size(
                    egui::pos2(card.left(), name_y - 2.0),
                    egui::vec2(card.width(), galley.size().y + 6.0),
                );
                ui.painter().rect_filled(
                    strip,
                    egui::CornerRadius::same(0),
                    egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180),
                );
            }

            let clip = egui::Rect::from_min_size(
                egui::pos2(name_x, name_y - 2.0),
                egui::vec2(card.width() - 16.0, galley.size().y + 4.0),
            );
            let clipped = ui.painter().with_clip_rect(clip);
            clipped.galley(
                egui::pos2(name_x, name_y),
                galley,
                egui::Color32::TRANSPARENT,
            );

            if is_video(name) {
                let badge = egui::Rect::from_min_size(
                    egui::pos2(card.left() + 6.0, card.top() + 6.0),
                    egui::vec2(24.0, 16.0),
                );
                ui.painter().rect_filled(
                    badge,
                    egui::CornerRadius::same(4),
                    egui::Color32::from_rgba_unmultiplied(0, 0, 0, 160),
                );
                ui.painter().text(
                    badge.center(),
                    egui::Align2::CENTER_CENTER,
                    "\u{25B6}",
                    egui::FontId::new(10.0, egui::FontFamily::Proportional),
                    egui::Color32::WHITE,
                );
            }

            if is_current {
                ui.painter().circle_filled(
                    egui::pos2(card.right() - 14.0, card.top() + 14.0),
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

/// Draw a filled quarter-circle at `corner` that masks the sharp image corner
/// behind the card's rounded corner.  `dx`/`dy` select which quadrant.
fn corner_mask(
    painter: &egui::Painter,
    corner: egui::Pos2,
    dx: f32,
    dy: f32,
    r: f32,
    bg: egui::Color32,
    steps: usize,
) {
    let mut pts = Vec::with_capacity(steps + 2);
    pts.push(corner);
    for i in 0..=steps {
        let t = std::f32::consts::FRAC_PI_2 * (i as f32 / steps as f32);
        pts.push(corner + egui::vec2(dx * r * t.cos(), dy * r * t.sin()));
    }
    pts.push(corner);
    painter.add(egui::epaint::PathShape::convex_polygon(
        pts, bg, egui::Stroke::NONE,
    ));
}
