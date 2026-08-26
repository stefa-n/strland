use crate::config::Config;
use crate::win;
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[derive(Clone, Debug)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    pub target: PathBuf,
}

pub struct LauncherState {
    pub open: bool,
    pub anim: f32,
    query: String,
    apps: Vec<AppEntry>,
    loaded: bool,
    focus_search: bool,
    row_anim: HashMap<String, f32>,
    row_rank: HashMap<String, usize>,
    icon_cache: HashMap<String, Option<egui::TextureHandle>>,
    selected: usize,
    selection_query: String,
    cached_filtered: Vec<usize>,
    cached_query: String,
}

impl LauncherState {
    pub fn new() -> Self {
        Self {
            open: false,
            anim: 0.0,
            query: String::new(),
            apps: Vec::new(),
            loaded: false,
            focus_search: false,
            row_anim: HashMap::new(),
            row_rank: HashMap::new(),
            icon_cache: HashMap::new(),
            selected: 0,
            selection_query: String::new(),
            cached_filtered: Vec::new(),
            cached_query: String::new(),
        }
    }

    pub fn open(&mut self) {
        self.open = true;
        self.focus_search = true;
        self.query.clear();
        self.row_anim.clear();
        self.row_rank.clear();
        self.selected = 0;
        self.selection_query.clear();
        self.ensure_loaded();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.focus_search = false;
    }

    pub fn ensure_loaded(&mut self) {
        if self.loaded {
            return;
        }
        self.apps = load_apps();
        self.loaded = true;
    }

    pub fn panel_rect(&self, viewport_rect: egui::Rect, pill_rect: egui::Rect, cfg: &Config) -> egui::Rect {
        let width = cfg.launcher_width;
        let x = viewport_rect.center().x - width * 0.5;
        let y = pill_rect.top();
        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, cfg.launcher_height.max(160.0)))
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        origin: egui::Pos2,
        content_width: f32,
        cfg: &Config,
        accent: egui::Color32,
        highlight: egui::Color32,
        background: egui::Color32,
        search_icon: &egui::TextureHandle,
        dt: f32,
        alpha: f32,
    ) {
        if alpha <= 0.003 {
            return;
        }

        self.ensure_loaded();
        self.update_filtered_matches();

        // Keyboard selection resets whenever the query changes.
        if self.query != self.selection_query {
            self.selected = 0;
            self.selection_query = self.query.clone();
        }
        if self.selected >= self.cached_filtered.len() {
            self.selected = self.cached_filtered.len().saturating_sub(1);
        }

        // Row visibility is driven by the CURRENT filtered matches, so a search
        // immediately replaces the list — old rows fade out instead of lingering.
        let visible_ids: HashSet<String> =
            self.cached_filtered.iter().map(|&i| self.apps[i].id.clone()).take(cfg.launcher_results).collect();

        if self.open {
            for (rank, &idx) in self.cached_filtered.iter().enumerate().take(cfg.launcher_results) {
                let entry = &self.apps[idx];
                self.row_rank.insert(entry.id.clone(), rank);
                let current = *self.row_anim.get(&entry.id).unwrap_or(&0.0);
                self.row_anim.insert(entry.id.clone(), approach(current, 1.0, dt, cfg.launcher_anim_speed));
            }
        }

        // Fade out any previously-shown rows that are no longer in the results.
        let stale_ids: Vec<String> = self
            .row_anim
            .keys()
            .filter(|id| !visible_ids.contains(*id))
            .cloned()
            .collect();
        for id in stale_ids {
            let current = *self.row_anim.get(&id).unwrap_or(&0.0);
            let next = approach(current, 0.0, dt, cfg.launcher_anim_speed);
            if next <= 0.01 {
                self.row_anim.remove(&id);
                self.row_rank.remove(&id);
            } else {
                self.row_anim.insert(id, next);
            }
        }

        // Extract at most one real app icon per frame to keep things smooth.
        if self.open {
            for &idx in self.cached_filtered.iter().take(cfg.launcher_results) {
                let entry = &self.apps[idx];
                if !self.icon_cache.contains_key(&entry.id) {
                    let tex = load_icon_texture(ctx, &entry.id, &entry.target);
                    self.icon_cache.insert(entry.id.clone(), tex);
                    break;
                }
            }
        }

        let inset = 16.0;

        let area = egui::Area::new(egui::Id::new("launcher_panel"))
            .order(egui::Order::Foreground)
            .fixed_pos(origin)
            .movable(false);

        area.show(ctx, |ui| {
            ui.set_max_width(content_width);
            ui.set_clip_rect(egui::Rect::from_min_size(origin, egui::vec2(content_width, 4000.0)));

            // Search field row — no background; just an icon + bare text input.
            let field_rect = egui::Rect::from_min_size(
                origin + egui::vec2(inset, inset),
                egui::vec2(content_width - inset * 2.0, 40.0),
            );

            ui.add_space(inset);

            let text_alpha = (255.0 * alpha).round() as u8;
            let text_color = egui::Color32::from_rgba_premultiplied(255, 255, 255, text_alpha);

            // Search icon left, vertically centered on the field row.
            let icon_size = 17.0;
            let icon_center = egui::pos2(origin.x + inset + icon_size / 2.0, field_rect.center().y);
            let icon_rect = egui::Rect::from_center_size(icon_center, egui::vec2(icon_size, icon_size));
            ui.painter().image(
                search_icon.id(),
                icon_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::from_rgba_premultiplied(accent.r(), accent.g(), accent.b(), text_alpha),
            );

            let style = ui.style_mut();
            let vis = &mut style.visuals;
            vis.override_text_color = Some(text_color);
            vis.extreme_bg_color = egui::Color32::TRANSPARENT;
            vis.widgets.noninteractive.bg_stroke = egui::Stroke::NONE;
            vis.widgets.inactive.bg_stroke = egui::Stroke::NONE;
            vis.widgets.hovered.bg_stroke = egui::Stroke::NONE;
            vis.widgets.active.bg_stroke = egui::Stroke::NONE;
            vis.widgets.open.bg_stroke = egui::Stroke::NONE;
            vis.selection.bg_fill = egui::Color32::from_rgba_premultiplied(
                accent.r(),
                accent.g(),
                accent.b(),
                (80.0 * alpha).round() as u8,
            );
            vis.selection.stroke = egui::Stroke::new(
                1.5,
                egui::Color32::from_rgba_premultiplied(accent.r(), accent.g(), accent.b(), text_alpha),
            );

            // Text input, same vertical center as the icon.
            let input_rect = egui::Rect::from_min_size(
                egui::pos2(origin.x + inset + icon_size + 10.0, field_rect.center().y - 14.0),
                egui::vec2(content_width - inset * 2.0 - icon_size - 10.0, 28.0),
            );
            let search_id = ui.make_persistent_id("launcher_search");
            let search = egui::TextEdit::singleline(&mut self.query)
                .hint_text("Search")
                .frame(false)
                .desired_width(f32::INFINITY)
                .font(egui::FontId::new(cfg.font_size + 2.0, egui::FontFamily::Proportional));

            let response = ui.put(input_rect, search.id(search_id));
            if self.focus_search {
                response.request_focus();
                self.focus_search = false;
            }

            // Divider between the search bar and the results.
            let divider_y = field_rect.bottom() + 6.0;
            ui.painter().line_segment(
                [
                    egui::pos2(origin.x + inset, divider_y),
                    egui::pos2(origin.x + content_width - inset, divider_y),
                ],
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_premultiplied(accent.r(), accent.g(), accent.b(), (32.0 * alpha).round() as u8),
                ),
            );

            // Result rows
            let row_height = 50.0;
            let rows_top = divider_y + 10.0;
            let rows_left = origin.x + inset;
            let rows_width = content_width - inset * 2.0;
            let mut launch_target: Option<PathBuf> = None;

            // Keyboard navigation.
            if self.open {
                if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) && !self.cached_filtered.is_empty() {
                    self.selected = (self.selected + 1).min(self.cached_filtered.len() - 1);
                }
                if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) && !self.cached_filtered.is_empty() {
                    self.selected = self.selected.saturating_sub(1);
                }
            }

            let mut rows: Vec<_> = self
                .apps
                .iter()
                .filter_map(|app| {
                    if !visible_ids.contains(&app.id) {
                        return None;
                    }
                    let visibility = *self.row_anim.get(&app.id).unwrap_or(&0.0);
                    if visibility <= 0.01 {
                        return None;
                    }
                    Some((app, visibility, *self.row_rank.get(&app.id).unwrap_or(&usize::MAX)))
                })
                .collect();
            rows.sort_by_key(|(_, _, rank)| *rank);
            rows.truncate(cfg.launcher_results);

            for (app, visibility, rank) in rows {
                let fade = visibility * alpha;
                let y = rows_top + rank as f32 * row_height + (1.0 - visibility) * 18.0;
                let row_rect = egui::Rect::from_min_size(
                    egui::pos2(rows_left, y),
                    egui::vec2(rows_width, row_height - 8.0),
                );
                let response = ui.interact(row_rect, ui.make_persistent_id((&app.id, rank)), egui::Sense::click());
                let is_selected = rank == self.selected && self.open;
                // Hovering points the keyboard selection at the row it's over.
                if response.hovered() && self.open {
                    self.selected = rank;
                }
                let a = (fade * 255.0).round() as u8;

                // Selected rows use the dedicated highlight color; others sit on
                // the app background color (lifted just enough to read as a row).
                let (bg, stroke) = if is_selected {
                    (egui::Color32::from_rgba_premultiplied(
                        highlight.r(),
                        highlight.g(),
                        highlight.b(),
                        ((120.0 + 135.0 * visibility) * alpha).round() as u8,
                    ), highlight)
                } else {
                    let hover = if response.hovered() { 1.0 } else { 0.0 };
                    let lift: f32 = (0.04 + hover * 0.06) * 255.0;
                    let lift = lift.min(255.0 - background.r() as f32);
                    let lifted = egui::Color32::from_rgba_premultiplied(
                        (background.r() as f32 + lift) as u8,
                        (background.g() as f32 + lift) as u8,
                        (background.b() as f32 + lift) as u8,
                        255,
                    );
                    let border_alpha = ((45.0 + visibility * 60.0) * alpha).round() as u8;
                    (
                        lifted,
                        egui::Color32::from_rgba_premultiplied(accent.r(), accent.g(), accent.b(), border_alpha),
                    )
                };
                // Launcher text is solid white — only the entrance fade modulates it.
                let text = egui::Color32::from_rgba_premultiplied(255, 255, 255, a);

                ui.painter().rect_filled(row_rect, egui::CornerRadius::same(14), bg);
                ui.painter().rect_stroke(
                    row_rect,
                    egui::CornerRadius::same(14),
                    egui::Stroke::new(1.0, stroke),
                    egui::StrokeKind::Middle,
                );

                let icon_rect = egui::Rect::from_min_size(
                    egui::pos2(row_rect.left() + 10.0, row_rect.center().y - 15.0),
                    egui::vec2(30.0, 30.0),
                );
                let icon_tex = self.icon_cache.get(&app.id).cloned().flatten();
                match icon_tex {
                    Some(tex) => {
                        ui.painter().image(
                            tex.id(),
                            icon_rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::from_rgba_premultiplied(255, 255, 255, a),
                        );
                    }
                    None => {
                        // Fallback letter chip when no icon could be extracted.
                        ui.painter().rect_filled(
                            icon_rect,
                            egui::CornerRadius::same(8),
                            egui::Color32::from_rgba_premultiplied(
                                accent.r(),
                                accent.g(),
                                accent.b(),
                                ((60.0 + visibility * 42.0) * alpha).round() as u8,
                            ),
                        );
                        ui.painter().text(
                            egui::pos2(icon_rect.center().x, icon_rect.center().y - 0.5),
                            egui::Align2::CENTER_CENTER,
                            app.name.chars().next().unwrap_or('A'),
                            egui::FontId::new(cfg.font_size - 1.0, egui::FontFamily::Proportional),
                            egui::Color32::from_rgba_premultiplied(12, 12, 12, a),
                        );
                    }
                }
                ui.painter().text(
                    egui::pos2(row_rect.left() + 52.0, row_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &app.name,
                    egui::FontId::new(cfg.font_size, egui::FontFamily::Proportional),
                    text,
                );

                if response.clicked() && self.open {
                    launch_target = Some(app.target.clone());
                }
            }

            if self.open {
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.close();
                }
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    if let Some(&idx) = self.cached_filtered.get(self.selected) {
                        launch_target = Some(self.apps[idx].target.clone());
                    }
                }
            }

            if let Some(target) = launch_target {
                win::launch_path(&target);
                self.close();
            }
        });
    }

    fn update_filtered_matches(&mut self) {
        let query = self.query.trim().to_ascii_lowercase();
        if query == self.cached_query && !self.cached_filtered.is_empty() {
            return;
        }
        self.cached_query = query.clone();
        let mut scored: Vec<(usize, i32)> = self
            .apps
            .iter()
            .enumerate()
            .map(|(i, entry)| (i, score_entry(entry, &query)))
            .filter(|(_, score)| *score >= 0)
            .collect();
        scored.sort_by(|(a, score_a), (b, score_b)| score_b.cmp(score_a).then_with(|| self.apps[*a].name.cmp(&self.apps[*b].name)));
        self.cached_filtered = scored.into_iter().map(|(i, _)| i).collect();
    }
}

fn score_entry(entry: &AppEntry, query: &str) -> i32 {
    if query.is_empty() {
        return 1;
    }
    let name = entry.name.to_ascii_lowercase();
    if name == query {
        return 1000;
    }
    if name.starts_with(query) {
        return 700 - query.len() as i32;
    }
    if name.contains(query) {
        return 400 - query.len() as i32;
    }
    let compact: String = name.chars().filter(|ch| !ch.is_whitespace()).collect();
    if compact.contains(query) {
        return 250;
    }
    -1
}

fn approach(current: f32, target: f32, dt: f32, speed: f32) -> f32 {
    let factor = 1.0 - (-speed * dt).exp();
    current + (target - current) * factor
}

fn load_icon_texture(ctx: &egui::Context, id: &str, target: &Path) -> Option<egui::TextureHandle> {
    let px = win::extract_app_icon(target)?;
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [px.width.max(1), px.height.max(1)],
        &px.rgba,
    );
    Some(ctx.load_texture(format!("appicon-{id}"), image, egui::TextureOptions::LINEAR))
}

fn load_apps() -> Vec<AppEntry> {
    let mut apps = Vec::new();
    let mut dedupe = HashSet::new();

    for folder in start_menu_folders() {
        scan_start_menu_dir(&folder, &mut apps, &mut dedupe);
    }
    scan_registry_app_paths("HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\App Paths", &mut apps, &mut dedupe);
    scan_registry_app_paths("HKLM\\Software\\Microsoft\\Windows\\CurrentVersion\\App Paths", &mut apps, &mut dedupe);

    apps.sort_by(|a, b| a.name.cmp(&b.name));
    apps
}

fn start_menu_folders() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(appdata) = std::env::var("APPDATA") {
        paths.push(PathBuf::from(appdata).join("Microsoft\\Windows\\Start Menu\\Programs"));
    }
    if let Ok(program_data) = std::env::var("ProgramData") {
        paths.push(PathBuf::from(program_data).join("Microsoft\\Windows\\Start Menu\\Programs"));
    }
    paths
}

fn scan_start_menu_dir(dir: &Path, apps: &mut Vec<AppEntry>, dedupe: &mut HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_start_menu_dir(&path, apps, dedupe);
            continue;
        }

        let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        let ext = ext.to_ascii_lowercase();
        if ext != "lnk" && ext != "exe" && ext != "url" {
            continue;
        }

        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let stem = stem.to_string();
        push_app(apps, dedupe, &stem, path);
    }
}

fn scan_registry_app_paths(root: &str, apps: &mut Vec<AppEntry>, dedupe: &mut HashSet<String>) {
    let Ok(output) = Command::new("reg").args(["query", root, "/s"]).creation_flags(0x08000000).output() else {
        return;
    };
    if !output.status.success() {
        return;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut current_key = String::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("HKEY_") {
            current_key = trimmed.to_string();
            continue;
        }
        if !trimmed.starts_with("(Default)") || !trimmed.contains("REG_SZ") {
            continue;
        }
        let Some(value) = trimmed.split("REG_SZ").nth(1) else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }

        let path = PathBuf::from(value.trim_matches('"'));
        let key_name = current_key.rsplit('\\').next().unwrap_or_default();
        let display_name = Path::new(key_name)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(key_name);
        push_app(apps, dedupe, display_name, path);
    }
}

fn push_app(apps: &mut Vec<AppEntry>, dedupe: &mut HashSet<String>, raw_name: &str, target: PathBuf) {
    let name = clean_name(raw_name);
    if name.is_empty() {
        return;
    }
    let id = format!("{}|{}", name.to_ascii_lowercase(), target.display());
    if !dedupe.insert(id.clone()) {
        return;
    }
    apps.push(AppEntry { id, name, target });
}

fn clean_name(raw: &str) -> String {
    raw.replace('_', " ")
        .replace("  ", " ")
        .trim()
        .to_string()
}
