// No console window in release builds (debug keeps one for the [notify] logs).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod launcher;
mod modes;
mod power;
mod switcher;
mod themes;
mod wallpaper;
mod win;

use chrono::Local;
use eframe::egui;
use launcher::LauncherState;
use modes::IslandMode;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::time::{Duration, Instant, SystemTime};

use crate::config::Config;

fn smooth_step(current: f32, target: f32, dt: f32, speed: f32) -> (f32, bool) {
    let diff = target - current;
    if diff.abs() < 0.01 {
        return (target, true);
    }
    let factor = 1.0 - (-speed * dt).exp();
    (current + diff * factor, false)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    egui::Color32::from_rgba_premultiplied(
        lerp(a.r() as f32, b.r() as f32, t).round() as u8,
        lerp(a.g() as f32, b.g() as f32, t).round() as u8,
        lerp(a.b() as f32, b.b() as f32, t).round() as u8,
        lerp(a.a() as f32, b.a() as f32, t).round() as u8,
    )
}

fn lerp_rect(a: egui::Rect, b: egui::Rect, t: f32) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(lerp(a.min.x, b.min.x, t), lerp(a.min.y, b.min.y, t)),
        egui::pos2(lerp(a.max.x, b.max.x, t), lerp(a.max.y, b.max.y, t)),
    )
}

fn pill_center_y(cfg: &Config) -> f32 {
    cfg.border_radius + 8.0 + cfg.height / 2.0
}

fn island_viewport_size(
    cfg: &Config,
    launcher_anim: f32,
    hover_anim: f32,
    power_open: bool,
    control_open: bool,
    wallpaper_open: bool,
    themes_open: bool,
    switcher_active: bool,
    power_anim: f32,
    control_anim: f32,
    wallpaper_anim: f32,
    themes_anim: f32,
) -> egui::Vec2 {
    if switcher_active {
        return egui::vec2(760.0, 520.0);
    }
    let closed_width = (cfg.width * cfg.volume_expand_factor).max(cfg.width) + 64.0;
    let open_width = closed_width.max(cfg.launcher_width + 40.0);
    let closed_height = cfg.height + cfg.border_radius * 2.0 + 16.0;
    // Open viewport spans from window top to just past the launcher panel,
    // whose top edge is anchored at the pill's top.
    let open_height = cfg.border_radius + 8.0 + cfg.launcher_height.max(160.0) + 20.0;
    // Hover expansion grows width/height around the collapsed pill.
    let hover_width = closed_width.max(cfg.hover_width + 40.0);
    let hover_height = cfg.border_radius + 8.0 + cfg.hover_height.max(48.0) + 16.0;
    // Power menu growth below the pill.
    let power_width = closed_width.max(560.0 + 40.0);
    let power_height = cfg.border_radius + 8.0 + power::POWER_MENU_HEIGHT + 24.0;
    // Control-center growth below the pill.
    let control_width = closed_width.max(560.0 + 40.0);
    let control_height = cfg.border_radius + 8.0 + 500.0 + 24.0;
    // Wallpaper panel growth below the pill.
    let wallpaper_width = closed_width.max(560.0 + 40.0);
    let wallpaper_height = cfg.border_radius + 8.0 + 420.0 + 24.0;
    // Themes panel growth below the pill.
    let themes_width = closed_width.max(560.0 + 40.0);
    let themes_height = cfg.border_radius + 8.0 + 420.0 + 24.0;
    let tl = ease_out_cubic(launcher_anim);
    let th = ease_out_cubic(hover_anim);
    // Snap the window to full size immediately so the native window can't lag
    // behind the drawn panel (prevents clipping); only the content eases in.
    let tp = if power_open { 1.0 } else { ease_out_cubic(power_anim) };
    let tc = if control_open { 1.0 } else { ease_out_cubic(control_anim) };
    let tw = if wallpaper_open { 1.0 } else { ease_out_cubic(wallpaper_anim) };
    let tt = if themes_open { 1.0 } else { ease_out_cubic(themes_anim) };
    let w = lerp(lerp(closed_width, open_width, tl), hover_width, th);
    let h = lerp(lerp(closed_height, open_height, tl), hover_height, th);
    let w = lerp(w, power_width, tp);
    let h = lerp(h, power_height, tp);
    let w = lerp(w, control_width, tc);
    let h = lerp(h, control_height, tc);
    let w = lerp(w, wallpaper_width, tw);
    let h = lerp(h, wallpaper_height, tw);
    let w = lerp(w, themes_width, tt);
    let h = lerp(h, themes_height, tt);
    egui::vec2(w, h)
}

fn point_in_round_rect(point: egui::Pos2, rect: egui::Rect, radius: f32) -> bool {
    if !rect.contains(point) {
        return false;
    }
    let radius = radius.min(rect.width() * 0.5).min(rect.height() * 0.5);
    let inner_rect = rect.shrink(radius);
    if inner_rect.contains(point) {
        return true;
    }
    let corners = [
        egui::pos2(rect.left() + radius, rect.top() + radius),
        egui::pos2(rect.right() - radius, rect.top() + radius),
        egui::pos2(rect.left() + radius, rect.bottom() - radius),
        egui::pos2(rect.right() - radius, rect.bottom() - radius),
    ];
    corners.iter().any(|center| point.distance(*center) <= radius)
}

struct PollState {
    next_window: Instant,
    next_volume: Instant,
    next_config: Instant,
    next_clock: Instant,
}

impl PollState {
    fn new(now: Instant) -> Self {
        Self {
            next_window: now,
            next_volume: now,
            next_config: now,
            next_clock: now,
        }
    }

    fn earliest(&self) -> Instant {
        let mut earliest = self.next_window;
        earliest = earliest.min(self.next_volume);
        earliest = earliest.min(self.next_config);
        earliest.min(self.next_clock)
    }
}

struct DynamicIslandApp {
    cfg: Config,
    hwnd: Option<isize>,
    last_modified: SystemTime,
    viewport_size: egui::Vec2,

    clock: String,
    day: String,
    mode: IslandMode,
    hidden: bool,

    current_y: f32,
    target_y: f32,
    y_animating: bool,

    current_width: f32,
    target_width: f32,
    width_animating: bool,

    current_height: f32,
    hover_anim: f32,

    last_volume: f32,
    display_volume: f32,
    volume_initialized: bool,
    volume_mode_deadline: Option<Instant>,
    hold_shown_until: Option<Instant>,
    notification_mode_deadline: Option<Instant>,
    notif: Option<win::NotificationInfo>,
    notif_deadline: Option<Instant>,
    notif_shown_at: Option<Instant>,

    media_playing: bool,
    visualizer_visibility: f32,
    display_bands: [f32; 4],

    launcher: LauncherState,

    vol_icon_on: Option<egui::TextureHandle>,
    vol_icon_off: Option<egui::TextureHandle>,
    status_icon_wifi: Option<egui::TextureHandle>,
    status_icon_eth: Option<egui::TextureHandle>,
    status_icon_bt: Option<egui::TextureHandle>,
    status_icon_battery: Option<egui::TextureHandle>,
    search_icon: Option<egui::TextureHandle>,
    marquee_start: Option<f64>,
    media_tex: Option<egui::TextureHandle>,
    media_art_gen: u64,

    power: power::PowerMenuState,
    power_icons: Option<[egui::TextureHandle; 5]>,

    switcher: switcher::SwitcherState,
    last_switcher_tab: u64,

    wallpaper: wallpaper::WallpaperState,
    themes: themes::ThemesState,

    control_open: bool,
    control_state: modes::ControlCenterState,
    control_icons: Option<modes::ControlCenterIcons>,
    control_anim: f32,
    had_panel_focus: bool,

    last_frame: Instant,
    polls: PollState,
    clickthrough: bool,
    accepts_focus: bool,
}

impl DynamicIslandApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let cfg = Config::load();
        load_custom_font(&cc.egui_ctx, &cfg.font);
        cc.egui_ctx.set_theme(egui::Theme::Dark);

        #[cfg(target_os = "windows")]
        let hwnd = {
            let handle = cc.window_handle().ok().and_then(|handle| match handle.as_raw() {
                RawWindowHandle::Win32(win32) => Some(win32.hwnd.get() as isize),
                _ => None,
            });
            if let Some(hwnd) = handle {
                win::configure_window_styles(hwnd, true, false);
            }
            handle
        };

        #[cfg(not(target_os = "windows"))]
        let hwnd = None;

        let now = Instant::now();
        let viewport_size = island_viewport_size(&cfg, 0.0, 0.0, false, false, false, false, false, 0.0, 0.0, 0.0, 0.0);
        let start_y = -(viewport_size.y + cfg.y_padding);
        let clock = formatted_clock(&cfg);
        let day = formatted_day();
        let start_width = cfg.width.min(viewport_size.x);
        let start_height = cfg.height;

        Self {
            cfg,
            hwnd,
            last_modified: config_file_mtime(),
            viewport_size,
            clock,
            day,
            mode: IslandMode::Clock,
            hidden: false,
            current_y: start_y,
            target_y: 0.0,
            y_animating: true,
            current_width: start_width,
            target_width: start_width,
            width_animating: false,
            current_height: start_height,
            hover_anim: 0.0,
            last_volume: 0.0,
            display_volume: 0.0,
            volume_initialized: false,
            volume_mode_deadline: None,
            hold_shown_until: None,
            notification_mode_deadline: None,
            notif: None,
            notif_deadline: None,
            notif_shown_at: None,
            media_playing: false,
            visualizer_visibility: 0.0,
            display_bands: [0.0; 4],
            launcher: LauncherState::new(),
            vol_icon_on: None,
            vol_icon_off: None,
            status_icon_wifi: None,
            status_icon_eth: None,
            status_icon_bt: None,
            status_icon_battery: None,
            search_icon: None,
            marquee_start: None,
            media_tex: None,
            media_art_gen: 0,
            power: power::PowerMenuState::new(),
            power_icons: None,
            switcher: switcher::SwitcherState::new(),
            last_switcher_tab: 0,
            wallpaper: wallpaper::WallpaperState::new(),
            themes: themes::ThemesState::new(),
            control_open: false,
            control_state: modes::ControlCenterState::default(),
            control_icons: None,
            control_anim: 0.0,
            had_panel_focus: false,
            last_frame: now,
            polls: PollState::new(now),
            clickthrough: true,
            accepts_focus: false,
        }
    }

    fn sync_viewport_size(&mut self, ctx: &egui::Context) {
        let desired = island_viewport_size(&self.cfg, self.launcher.anim, self.hover_anim, self.power.open, self.control_open, self.wallpaper.open, self.themes.open, self.switcher.active || self.switcher.anim > 0.001, self.power.anim, self.control_anim, self.wallpaper.anim, self.themes.anim);
        if desired == self.viewport_size {
            return;
        }

        self.viewport_size = desired;
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(desired));

        if let Some(hwnd) = self.hwnd {
            win::reset_pos_cache();
            let scale = win::window_scale(hwnd);
            let phys_w = desired.x * scale;
            let phys_h = desired.y * scale;
            let x = ((win::get_screen_width() as f32 - phys_w) / 2.0).round() as i32;
            // The switcher overlay is vertically centered on screen; as it
            // closes it eases back up to the island's resting position instead
            // of teleporting.
            let y = if self.switcher.active || self.switcher.anim > 0.001 {
                let screen_h = win::get_screen_height();
                let centered = (screen_h as f32 - phys_h) / 2.0;
                // anim=1 → centered; anim→0 → current_y (top of screen).
                let t = ease_out_cubic(self.switcher.anim);
                lerp(centered, self.current_y, 1.0 - t).round() as i32
            } else {
                self.current_y.round() as i32
            };
            win::set_position_and_size(
                hwnd,
                x,
                y,
                phys_w.round() as i32,
                phys_h.round() as i32,
            );
        }
    }

    fn set_launcher_open(&mut self, ctx: &egui::Context, open: bool) {
        if open == self.launcher.open {
            return;
        }
        if open {
            self.launcher.open();
            // Collapse any hover expansion first so the morph starts from the
            // resting pill (and no expanded clock bleeds over the search field).
            self.hover_anim = 0.0;
            if let Some(hwnd) = self.hwnd {
                win::configure_window_styles(hwnd, false, true);
                win::focus_window(hwnd);
            }
        } else {
            self.launcher.close();
        }
        self.sync_viewport_size(ctx);
    }

    fn apply_config_reload(&mut self, ctx: &egui::Context) {
        let current = config_file_mtime();
        if current == self.last_modified {
            return;
        }
        let (new_cfg, _) = Config::load_and_heal();
        if new_cfg.font != self.cfg.font {
            load_custom_font(ctx, &new_cfg.font);
        }
        self.cfg = new_cfg;
        self.last_modified = config_file_mtime();
        self.sync_viewport_size(ctx);

        if let Some(hwnd) = self.hwnd {
            win::reset_pos_cache();
            let scale = win::window_scale(hwnd);
            let phys_w = self.viewport_size.x * scale;
            let phys_h = self.viewport_size.y * scale;
            let x = ((win::get_screen_width() as f32 - phys_w) / 2.0).round() as i32;
            win::resize_window(
                hwnd,
                phys_w.round() as i32,
                phys_h.round() as i32,
            );
            win::set_position(hwnd, x, self.current_y.round() as i32);
        }
    }

    fn update_clock(&mut self) {
        self.clock = formatted_clock(&self.cfg);
        self.day = formatted_day();
    }

    fn poll_window_state(&mut self, cursor: &win::FrameCursor) {
        let shown_y = self.cfg.y_padding;
        let hidden_y = -(self.viewport_size.y + self.cfg.y_padding);
        // Hide over maximized windows and borderless fullscreen (video/games).
        let covered = win::foreground_covers_screen();

        if covered {
            if win::cursor_at_top(cursor, 4) && self.hidden {
                self.hidden = false;
                self.target_y = shown_y;
                self.y_animating = true;
            } else if !win::cursor_at_top(
                cursor,
                (self.viewport_size.y + 20.0 + self.cfg.y_padding) as i32,
            ) && !self.hidden
            {
                self.hidden = true;
                self.target_y = hidden_y;
                self.y_animating = true;
            }
        } else if self.hidden {
            self.hidden = false;
            self.target_y = shown_y;
            self.y_animating = true;
        }
    }

    fn poll_volume(&mut self, now: Instant) {
        // Media keys (volume up/down/mute) trigger the UI even without a shell —
        // when there's no shell we apply the change ourselves (on this thread,
        // never inside the keyboard hook).
        if let Some(kind) = win::take_media_key_event(500) {
            if !win::shell_is_running() {
                win::apply_media_key(kind);
            }
            self.mode = IslandMode::Volume;
            self.volume_mode_deadline =
                Some(now + Duration::from_millis(self.cfg.volume_timeout_ms));
            self.last_volume = win::get_volume();
            self.bring_down_if_hidden();
            self.hold_shown_until =
                Some(now + Duration::from_millis(self.cfg.volume_timeout_ms + 1000));
        }

        let vol = win::get_volume();
        if !self.volume_initialized {
            self.last_volume = vol;
            self.volume_initialized = true;
            return;
        }

        let changed = (vol - self.last_volume).abs() > 0.003;
        if changed {
            self.last_volume = vol;
            self.mode = IslandMode::Volume;
            self.target_width = self.cfg.width * self.cfg.volume_expand_factor;
            self.width_animating = true;
            self.volume_mode_deadline =
                Some(now + Duration::from_millis(self.cfg.volume_timeout_ms));
            self.bring_down_if_hidden();
            self.hold_shown_until =
                Some(now + Duration::from_millis(self.cfg.volume_timeout_ms + 1000));
        } else if self.mode == IslandMode::Volume {
            if let Some(deadline) = self.volume_mode_deadline {
                if now >= deadline {
                    // Swap content right away — the pill shrinks while already showing the clock.
                    self.mode = IslandMode::Clock;
                    self.target_width = self.cfg.width;
                    self.width_animating = true;
                    self.volume_mode_deadline = None;
                }
            }
        }
    }

    fn update_animations(&mut self, dt: f32) {
        // Real spectrum bars: fast attack, slower release for a lively feel.
        let target_bands = win::audio_bands();
        for i in 0..self.display_bands.len() {
            let speed = if target_bands[i] > self.display_bands[i] { 30.0 } else { 10.0 };
            let (next, _) = smooth_step(self.display_bands[i], target_bands[i], dt, speed);
            self.display_bands[i] = next.clamp(0.0, 1.0);
        }

        let visualizer_target = if self.media_playing { 1.0 } else { 0.0 };
        let (vis, _) = smooth_step(
            self.visualizer_visibility,
            visualizer_target,
            dt,
            self.cfg.visualizer_anim_speed,
        );
        self.visualizer_visibility = vis.clamp(0.0, 1.0);

        // Smooth volume bar interpolation (speed 18 = snappy but not jarring)
        let (dv, _) = smooth_step(self.display_volume, self.last_volume, dt, 18.0);
        self.display_volume = dv;

        if self.y_animating {
            let (new_y, done) =
                smooth_step(self.current_y, self.target_y, dt, self.cfg.slide_anim_speed);
            self.current_y = new_y;
            self.y_animating = !done;

            if let Some(hwnd) = self.hwnd {
                let scale = win::window_scale(hwnd);
                let phys_w = self.viewport_size.x * scale;
                let x = ((win::get_screen_width() as f32 - phys_w) / 2.0).round() as i32;
                win::set_position(hwnd, x, self.current_y.round() as i32);
            }
        }

        if self.width_animating {
            let (new_w, done) = smooth_step(
                self.current_width,
                self.target_width,
                dt,
                self.cfg.mode_anim_speed,
            );
            self.current_width = new_w;
            self.width_animating = !done;
            if done && (self.current_width - self.cfg.width).abs() < 0.5 {
                self.current_width = self.cfg.width;
            }
        }
    }

    fn focus_window(&mut self) {
        if let Some(hwnd) = self.hwnd {
            win::focus_window(hwnd);
        }
    }

    /// Slides the island back down (un-hides) if it's currently hidden. Skipped
    /// when the cursor is hidden (games) so we don't pop over gameplay.
    fn bring_down_if_hidden(&mut self) {
        if self.hidden && win::cursor_visible() {
            self.hidden = false;
            self.target_y = self.cfg.y_padding;
            self.y_animating = true;
        }
    }

    fn update_window_styles(&mut self, clickthrough: bool, accepts_focus: bool) {
        if self.clickthrough == clickthrough && self.accepts_focus == accepts_focus {
            return;
        }
        self.clickthrough = clickthrough;
        self.accepts_focus = accepts_focus;
        if let Some(hwnd) = self.hwnd {
            win::configure_window_styles(hwnd, clickthrough, accepts_focus);
        }
    }

    fn ensure_ui_assets(&mut self, ctx: &egui::Context) {
        // Volume SVG icons (accent-tinted at draw time).
        let icon_px = (24.0 * win::window_scale(self.hwnd.unwrap_or(0))).round();
        match &self.vol_icon_on {
            Some(t) if t.size()[0] as f32 == icon_px => {}
            _ => self.vol_icon_on = Some(render_svg_texture(ctx, "vol-on", VOLUME_SVG_ON, icon_px)),
        }
        match &self.vol_icon_off {
            Some(t) if t.size()[0] as f32 == icon_px => {}
            _ => {
                self.vol_icon_off = Some(render_svg_texture(ctx, "vol-off", VOLUME_SVG_OFF, icon_px))
            }
        }

        // Status icons (accent-tinted at draw time).
        self.status_icon_wifi.get_or_insert_with(|| {
            render_svg_texture(ctx, "status-wifi", WIFI_SVG, icon_px)
        });
        self.status_icon_eth.get_or_insert_with(|| {
            render_svg_texture(ctx, "status-eth", ETHERNET_SVG, icon_px)
        });
        self.status_icon_bt.get_or_insert_with(|| {
            render_svg_texture(ctx, "status-bt", BLUETOOTH_SVG, icon_px)
        });
        self.status_icon_battery.get_or_insert_with(|| {
            render_svg_texture(ctx, "status-battery", BATTERY_SVG, icon_px)
        });
        self.search_icon.get_or_insert_with(|| {
            render_svg_texture(ctx, "search", SEARCH_SVG, icon_px)
        });

        // Session-menu icons (lazy; built once for the power menu).
        if self.power_icons.is_none() {
            let arr = [
                render_svg_texture(ctx, "power-lock", POWER_LOCK_SVG, icon_px),
                render_svg_texture(ctx, "power-suspend", POWER_SUSPEND_SVG, icon_px),
                render_svg_texture(ctx, "power-logout", POWER_LOGOUT_SVG, icon_px),
                render_svg_texture(ctx, "power-reboot", POWER_REBOOT_SVG, icon_px),
                render_svg_texture(ctx, "power-off", POWER_OFF_SVG, icon_px),
            ];
            self.power_icons = Some(arr);
        }

        // Control-center icons (lazy).
        if self.control_icons.is_none() {
            self.control_icons = Some(modes::ControlCenterIcons {
                wifi: render_svg_texture(ctx, "cc-wifi", WIFI_SVG, icon_px),
                audio: render_svg_texture(ctx, "cc-audio", CONTROL_AUDIO_SVG, icon_px),
                audio_muted: render_svg_texture(ctx, "cc-audio-muted", VOLUME_SVG_OFF, icon_px),
                bt: render_svg_texture(ctx, "cc-bt", BLUETOOTH_SVG, icon_px),
                moon: render_svg_texture(ctx, "cc-moon", CONTROL_MOON_SVG, icon_px),
                night: render_svg_texture(ctx, "cc-night", CONTROL_NIGHT_SVG, icon_px),
                power: render_svg_texture(ctx, "cc-power", POWER_OFF_SVG, icon_px),
                wallpaper: render_svg_texture(ctx, "cc-wallpaper", WALLPAPER_SVG, icon_px),
                theme: render_svg_texture(ctx, "cc-theme", THEME_SVG, icon_px),
            });
        }

        // Album art upload when the background fetcher produced new pixels.
        if let Some(art) = win::media_art_if_new(&mut self.media_art_gen) {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [art.width.max(1), art.height.max(1)],
                &art.rgba,
            );
            self.media_tex =
                Some(ctx.load_texture("media-art", image, egui::TextureOptions::LINEAR));
        }
    }
}

impl eframe::App for DynamicIslandApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.05);
        self.last_frame = now;

        // While hidden the pill isn't rendered — poll slowly and skip cosmetic
        // work (clock text) so idle redraws are rare. Volume keys / notifications
        // still wake it because those polls keep running.
        let hidden_idle = self.hidden && !self.switcher.active;

        // --- Polls (only when due) ---
        if now >= self.polls.next_config {
            self.apply_config_reload(ctx);
            self.polls.next_config = now + Duration::from_millis(self.cfg.config_poll_ms);
        }
        if !hidden_idle && now >= self.polls.next_clock {
            self.update_clock();
            self.polls.next_clock = now + Duration::from_millis(self.cfg.clock_poll_ms);
        }

        // Get cursor once per frame — shared by all polls and drawing
        let cursor = win::frame_cursor();

        if now >= self.polls.next_window {
            // Don't auto-hide while a notification card is up, or while the
            // island is pinned down after a volume change — it should stay
            // visible for its timeout instead of jittering up and down.
            let holding = self.hold_shown_until.map(|t| now < t).unwrap_or(false);
            if self.notif.is_none() && !holding {
                self.poll_window_state(&cursor);
            }
            // When hidden there's nothing to watch on screen — poll slowly.
            let window_poll = if hidden_idle { 150 } else { self.cfg.window_poll_ms };
            self.polls.next_window = now + Duration::from_millis(window_poll);
        }
        if now >= self.polls.next_volume {
            self.poll_volume(now);
            let volume_poll = if hidden_idle { 150 } else { self.cfg.volume_poll_ms };
            self.polls.next_volume = now + Duration::from_millis(volume_poll);
        }

        // Media playing is polled on a background thread — just read the atomic
        self.media_playing = win::media_is_playing();

        // New Windows toast notification → show it as a card and bring the
        // island back down (suppressed in Peace / Do-Not-Disturb mode). Only
        // apps with a custom implementation are shown (currently Discord).
        if !self.control_state.dnd && win::take_new_notification(1200) {
            let found = win::take_latest_notification()
                .filter(|n| is_supported_notification_app(&n.app));
            if let Some(n) = found {
                self.notif = Some(n);
                self.notif_shown_at = Some(now);
                self.notif_deadline =
                    Some(now + Duration::from_millis(self.cfg.notification_timeout_ms));
                self.mode = IslandMode::Notification;
                // Slide the island back down if it's hidden.
                if self.hidden {
                    self.hidden = false;
                    self.target_y = self.cfg.y_padding;
                    self.y_animating = true;
                }
            }
        }

        // Song change → brief "Now Playing" notification (only if nothing else
        // is showing; suppressed in Peace).
        if self.notif.is_none()
            && self.mode == IslandMode::Clock
            && self.media_playing
            && !self.control_state.dnd
            && win::take_track_change(400)
        {
            self.mode = IslandMode::Notification;
            self.notification_mode_deadline =
                Some(now + Duration::from_millis(self.cfg.notification_timeout_ms));
        }

        // Auto-dismiss timeout.
        if let Some(deadline) = self.notification_mode_deadline {
            if now >= deadline {
                self.notification_mode_deadline = None;
                if self.mode == IslandMode::Notification {
                    self.mode = IslandMode::Clock;
                }
            }
        }
        if let Some(deadline) = self.notif_deadline {
            if now >= deadline {
                self.notif_deadline = None;
                self.notif = None;
                self.notif_shown_at = None;
                if self.mode == IslandMode::Notification {
                    self.mode = IslandMode::Clock;
                }
            }
        }
        // Click or global ESC dismisses the notification card. A short grace
        // after appearing prevents an in-flight click from dismissing it the
        // instant it pops up.
        if self.notif.is_some() {
            let clicking = ctx.input(|i| i.pointer.any_pressed())
                && self.notif_shown_at.map(|t| now.duration_since(t) > Duration::from_millis(150)).unwrap_or(false);
            let esc = win::take_escape(100);
            if clicking || esc {
                self.notif = None;
                self.notif_deadline = None;
                self.notif_shown_at = None;
                if self.mode == IslandMode::Notification {
                    self.mode = IslandMode::Clock;
                }
            }
        }

        // Close the launcher if it genuinely loses focus to another window —
        // but only after it has actually held focus (otherwise opening via Win
        // key while foreground-locked would instantly bounce back to clock).
        if self.launcher.open {
            if let Some(hwnd) = self.hwnd {
                if win::foreground_hwnd() == hwnd {
                    self.had_panel_focus = true;
                } else if self.had_panel_focus {
                    self.set_launcher_open(ctx, false);
                }
            }
        }

        // Super/Windows key → open the app launcher (closing any other overlay).
        if win::take_super_key(300) {
            if self.power.open {
                self.power.close();
            }
            if self.control_open {
                self.control_open = false;
                self.control_state.submenu = modes::ControlSubmenu::None;
            }
            if self.wallpaper.open {
                self.wallpaper.close();
            }
            self.set_launcher_open(ctx, true);
        }

        // Win+A → toggle the control center.
        if win::take_control_open(300) {
            self.control_open = !self.control_open;
            if self.control_open {
                if self.power.open {
                    self.power.close();
                }
                if self.launcher.open {
                    self.set_launcher_open(ctx, false);
                }
                if self.wallpaper.open {
                    self.wallpaper.close();
                }
                // Freeze hover so the panel doesn't ride the expanding pill.
                self.hover_anim = 0.0;
                self.focus_window();
            }
        }

        // Win+W → toggle the wallpaper panel (closing any other overlay).
        if win::take_wallpaper_open(300) {
            win::debug_log("[wallpaper] Win+W trigger fired — opening panel");
            self.wallpaper.open = !self.wallpaper.open;
            if self.wallpaper.open {
                if self.power.open {
                    self.power.close();
                }
                if self.launcher.open {
                    self.set_launcher_open(ctx, false);
                }
                if self.control_open {
                    self.control_open = false;
                    self.control_state.submenu = modes::ControlSubmenu::None;
                }
                self.hover_anim = 0.0;
                self.focus_window();
            }
        }
        // Win+Shift+S → fullscreen screenshot to clipboard (background thread).
        if win::take_screenshot_trigger(500) {
            win::debug_log("[screenshot] Win+Shift+S trigger fired — spawning capture");
            self.focus_window();
            std::thread::spawn(|| {
                let ok = win::capture_screen_to_clipboard();
                if ok {
                    win::notify_screenshot_done();
                } else {
                    win::debug_log("[screenshot] capture failed");
                }
            });
        }
        // Screenshot done → show a brief "Screenshot taken" notification card.
        if win::take_screenshot_done(2000) {
            win::debug_log("[screenshot] capture done — showing notification");
            let now = Instant::now();
            self.notif = Some(win::NotificationInfo {
                id: u32::MAX,
                app: "Screenshot".to_string(),
                title: "Screenshot taken".to_string(),
                body: String::new(),
            });
            self.notif_shown_at = Some(now);
            self.notif_deadline =
                Some(now + Duration::from_millis(self.cfg.notification_timeout_ms));
            self.mode = IslandMode::Notification;
            if self.hidden {
                self.hidden = false;
                self.target_y = self.cfg.y_padding;
                self.y_animating = true;
            }
            ctx.request_repaint();
        }
        if self.control_open && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.control_state.submenu != modes::ControlSubmenu::None {
                self.control_state.submenu = modes::ControlSubmenu::None;
            } else {
                self.control_open = false;
            }
        }
        if self.wallpaper.open && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.wallpaper.close();
        }
        if self.themes.open && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.themes.close();
        }
        // Animate the menu's list of open/close and refresh the geometry.
        let power_target = if self.power.open { 1.0 } else { 0.0 };
        let (pa, _) = smooth_step(self.power.anim, power_target, dt, 9.0);
        self.power.anim = pa;
        if !self.power.open && self.power.anim <= 0.001 {
            self.power.anim = 0.0;
        }
        if self.power.open && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.power.close();
        }
        // Animate the control-center open/close.
        let control_target = if self.control_open { 1.0 } else { 0.0 };
        let (ca, _) = smooth_step(self.control_anim, control_target, dt, 9.0);
        self.control_anim = ca;
        if !self.control_open && self.control_anim <= 0.001 {
            self.control_anim = 0.0;
        }
        // Animate the wallpaper panel open/close.
        let wp_target = if self.wallpaper.open { 1.0 } else { 0.0 };
        let (wa, _) = smooth_step(self.wallpaper.anim, wp_target, dt, 9.0);
        self.wallpaper.anim = wa;
        if !self.wallpaper.open && self.wallpaper.anim <= 0.001 {
            self.wallpaper.anim = 0.0;
        }

        // Animate the themes panel open/close.
        let th_target = if self.themes.open { 1.0 } else { 0.0 };
        let (tha, _) = smooth_step(self.themes.anim, th_target, dt, 9.0);
        self.themes.anim = tha;
        if !self.themes.open && self.themes.anim <= 0.001 {
            self.themes.anim = 0.0;
        }

        // Clicking outside the app (window loses focus entirely) closes any
        // open overlay panel. We only close on focus-loss if the window had
        // actually gained focus first — otherwise opening via a hotkey (when
        // SetForegroundWindow hasn't been granted yet) would instantly close it.
        if self.power.open || self.control_open {
            if let Some(hwnd) = self.hwnd {
                if win::foreground_hwnd() == hwnd {
                    self.had_panel_focus = true;
                } else if self.had_panel_focus {
                    self.had_panel_focus = false;
                    self.power.close();
                    self.control_open = false;
                    self.control_state.submenu = modes::ControlSubmenu::None;
                }
            }
        } else if !self.launcher.open && !self.switcher.active {
            self.had_panel_focus = false;
        }

        // Global Escape closes any overlay, even when another app has focus.
        if win::take_escape(300) {
            if self.switcher.active {
                self.switcher.dismiss();
            }
            if self.power.open {
                self.power.close();
            }
            if self.control_open {
                if self.control_state.submenu != modes::ControlSubmenu::None {
                    self.control_state.submenu = modes::ControlSubmenu::None;
                } else {
                    self.control_open = false;
                }
            }
            if self.launcher.open {
                self.set_launcher_open(ctx, false);
            }
        }

        // --- Alt+Tab app switcher lifecycle ---
        if win::switcher_alt_down() {
            // Each Tab press (while Alt is held) opens the switcher on the first
            // tap and advances the highlight on subsequent taps.
            let tab_ms = win::switcher_tab_ms();
            if tab_ms != 0 && tab_ms != self.last_switcher_tab {
                self.last_switcher_tab = tab_ms;
                if !self.switcher.active {
                    self.switcher.begin(ctx);
                    // Close any other overlay.
                    if self.power.open {
                        self.power.close();
                    }
                    self.control_open = false;
                    self.control_state.submenu = modes::ControlSubmenu::None;
                } else {
                    self.switcher.advance(1);
                }
            }
        }
        // Alt released while switching → focus the selected app. Fires when the
        // Alt-up is reported, OR simply when Alt is no longer down while the
        // switcher is showing (covers a missed timestamp).
        if self.switcher.active {
            let released = win::take_alt_released(500) || !win::switcher_alt_down();
            if released {
                self.switcher.activate_selected();
            }
        }
        if !win::switcher_alt_down() {
            self.last_switcher_tab = 0;
        }
        // Grow the switcher in from the pill over a short ease.
        let sw_target = if self.switcher.active { 1.0 } else { 0.0 };
        let (sw_anim, _) = smooth_step(self.switcher.anim, sw_target, dt, 10.0);
        self.switcher.anim = sw_anim;
        if !self.switcher.active && self.switcher.anim <= 0.001 {
            self.switcher.anim = 0.0;
        }

        // --- Animations ---
        let launcher_target = if self.launcher.open { 1.0 } else { 0.0 };
        let (anim, _) = smooth_step(
            self.launcher.anim,
            launcher_target,
            dt,
            self.cfg.launcher_anim_speed,
        );
        self.launcher.anim = anim;

        self.update_animations(dt);

        // Cursor in window-local logical coordinates (shared by hover + clickthrough).
        let scale = win::window_scale(self.hwnd.unwrap_or(0));
        let phys_w = self.viewport_size.x * scale;
        let window_x = (win::get_screen_width() as f32 - phys_w) / 2.0;
        let local_cursor = egui::pos2(
            (cursor.screen_x as f32 - window_x) / scale,
            cursor.screen_y as f32 - self.current_y,
        );

        // Hover expansion — probed against a stable rect so resize frames can't
        // make it flicker: collapsed size when idle, animated size (+margin,
        // hysteresis) once already expanded.
        let mut probe_pill = egui::Rect::from_center_size(
            egui::pos2(self.viewport_size.x / 2.0, pill_center_y(&self.cfg)),
            egui::vec2(
                if self.hover_anim > 0.01 { self.current_width } else { self.cfg.width },
                if self.hover_anim > 0.01 { self.current_height } else { self.cfg.height },
            ),
        );
        if self.hover_anim > 0.01 {
            probe_pill = probe_pill.expand(8.0);
        }
        let hovering = !self.launcher.open
            && !self.hidden
            && !self.power.open
            && !self.control_open
            && self.mode == IslandMode::Clock
            && point_in_round_rect(local_cursor, probe_pill, self.cfg.border_radius);
        let hover_target = if hovering { 1.0 } else { 0.0 };
        // Slower than the slide animation so the clock visibly grows and the
        // media content rolls in/out instead of snapping.
        let hover_speed = (self.cfg.slide_anim_speed * 0.5).min(8.0).max(3.0);
        let (hover_anim, _) = smooth_step(
            self.hover_anim,
            hover_target,
            dt,
            hover_speed,
        );
        self.hover_anim = hover_anim;

        // Anchor the marquee scroll to the moment the media content appears,
        // so the title starts scrolling from its natural position instead of
        // catching mid-cycle and whipping around for a second.
        if hover_anim > 0.05 && self.media_playing {
            if self.marquee_start.is_none() {
                self.marquee_start = Some(ctx.input(|i| i.time));
            }
        } else {
            self.marquee_start = None;
        }

        // Derive this frame's animated pill geometry from the hover progress.
        let ht = ease_out_cubic(hover_anim);
        self.current_height = lerp(
            self.cfg.height,
            self.cfg.hover_height.max(self.cfg.height),
            ht,
        );
        let base_w = match self.mode {
            IslandMode::Volume | IslandMode::Notification => {
                (self.cfg.width * self.cfg.volume_expand_factor).max(self.cfg.width)
            }
            _ => self.cfg.width,
        };
        self.target_width = lerp(base_w, base_w.max(self.cfg.hover_width), ht);
        self.width_animating = true;

        self.ensure_ui_assets(ctx);
        self.sync_viewport_size(ctx);

        // --- Drawing ---
        let viewport_rect = ctx.screen_rect();
        let pill_rect = egui::Rect::from_center_size(
            egui::pos2(viewport_rect.center().x, pill_center_y(&self.cfg)),
            egui::vec2(self.current_width, self.current_height),
        );
        let launcher_rect = self.launcher.panel_rect(viewport_rect, pill_rect, &self.cfg);
        let t = ease_out_cubic(self.launcher.anim);
        let morphing = self.launcher.anim > 0.001;
        let morph_rect = lerp_rect(pill_rect, launcher_rect, t);
        let morph_corner = lerp(self.cfg.border_radius, self.cfg.border_radius + 4.0, t);

        // Pill click input
        egui::Area::new(egui::Id::new("pill_input"))
            .order(egui::Order::Foreground)
            .fixed_pos(pill_rect.min)
            .show(ctx, |ui| {
                let (rect, response) =
                    ui.allocate_exact_size(pill_rect.size(), egui::Sense::click());
                if (response.secondary_clicked() || response.double_clicked()) && !self.power.open {
                    self.set_launcher_open(ctx, !self.launcher.open);
                }
                let _ = rect;
            });

        if self.launcher.open && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.set_launcher_open(ctx, false);
        }

        // Clickthrough logic — reuse the local cursor computed earlier
        let overlay_open = self.launcher.open || self.power.open || self.control_open || self.wallpaper.open;
        let interactive = overlay_open
            || point_in_round_rect(local_cursor, pill_rect, self.cfg.border_radius)
            || (morphing
                && self.launcher.open
                && point_in_round_rect(local_cursor, morph_rect, morph_corner));
        self.update_window_styles(!interactive, overlay_open);

        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::new("island_pill"),
        ));
        let accent = Config::parse_color(&self.cfg.accent_color);
        let background = Config::parse_color(&self.cfg.background);
        let font_id = egui::FontId::new(self.cfg.font_size, egui::FontFamily::Proportional);

        if !morphing {
            // Hide the clock pill's content while an overlay panel (control
            // center / power menu) is expanded onto it. `pill_alpha` ramps the
            // pill content back in smoothly as the panel closes away.
            let panel_fade = (1.0 - self.control_anim).min(1.0 - self.power.anim);
            let pill_alpha = ((panel_fade - 0.35) / 0.45).clamp(0.0, 1.0);
            let corner = egui::CornerRadius::same(self.cfg.border_radius as u8);

            // Only draw the pill itself when no overlay panel has grown over it.
            if pill_alpha > 0.01 {
                // Shadow
                painter.rect_filled(
                    pill_rect.translate(egui::vec2(0.0, 1.0)),
                    corner,
                    egui::Color32::from_black_alpha(40),
                );
                // Background
                painter.rect_filled(pill_rect, corner, background);
            }

            if pill_alpha > 0.01 {
                match self.mode {
                    IslandMode::Clock => {
                        // Continuous hover transition — everything is drawn here
                        // with the eased hover progress so it stays smooth.
                        let media_owned = win::media_text().and_then(|m| {
                            self.media_tex.clone().map(|tex| (tex, m.title, m.artist))
                        });
                        let media_ref = media_owned
                            .as_ref()
                            .map(|(tex, title, artist)| (tex, title.as_str(), artist.as_str()));
                        let marquee_t = self
                            .marquee_start
                            .map(|s| (ctx.input(|i| i.time) - s) as f32)
                            .unwrap_or(0.0)
                            .max(0.0);
                        modes::draw_clock_island(
                            &painter,
                            pill_rect,
                            &self.cfg,
                            &self.clock,
                            &self.day,
                            accent,
                            &font_id,
                            self.visualizer_visibility,
                            &self.display_bands,
                            media_ref,
                            self.hover_anim,
                            marquee_t,
                            pill_alpha,
                        );
                        // System status indicators (right side).
                        let status = win::system_status();
                        let status_clicked = modes::draw_status_icons(
                            &painter,
                            pill_rect,
                            &self.cfg,
                            accent,
                            &status,
                            self.status_icon_wifi.as_ref().unwrap(),
                            self.status_icon_eth.as_ref().unwrap(),
                            self.status_icon_bt.as_ref().unwrap(),
                            self.status_icon_battery.as_ref().unwrap(),
                            pill_alpha,
                            self.hover_anim > 0.05,
                        );
                        if status_clicked && !self.control_open {
                            self.control_open = true;
                            self.hover_anim = 0.0;
                            self.focus_window();
                            if self.power.open {
                                self.power.close();
                            }
                        }
                    }
                    IslandMode::Notification => {
                        let (small, big) = if let Some(n) = &self.notif {
                            if n.app == "Screenshot" {
                                // Screenshot notification: app name as small, title as big.
                                (n.app.clone(), n.title.clone())
                            } else {
                                // Discord-style: "<username> on Discord" + message body.
                                let img = format!("{} on {}", n.title, n.app);
                                (img, n.body.clone())
                            }
                        } else {
                            let m = win::media_text().map(|m| (m.title, m.artist)).unwrap_or_default();
                            ("Now Playing".to_string(), m.0)
                        };
                        modes::draw_notification_mode(
                            &painter,
                            pill_rect,
                            &self.cfg,
                            accent,
                            &small,
                            &big,
                            1.0,
                        );
                    }
                IslandMode::Volume => modes::draw_volume_mode(
                    &painter,
                    pill_rect,
                    accent,
                    self.display_volume,
                    1.0,
                    win::get_mute(),
                    self.vol_icon_on.as_ref().unwrap(),
                    self.vol_icon_off.as_ref().unwrap(),
                ),
            }
            }
        } else {
            // The pill morphs into the launcher panel; the clock's slot becomes the search field.
            let panel_fill = Config::parse_color(&self.cfg.launcher_background);
            let corner = egui::CornerRadius::same(morph_corner.round() as u8);

            let shadow_color = lerp_color(
                egui::Color32::from_black_alpha(40),
                egui::Color32::from_black_alpha(110),
                t,
            );
            painter.rect_filled(
                morph_rect.translate(egui::vec2(0.0, lerp(1.0, 12.0, t))),
                corner,
                shadow_color,
            );

            let fill = lerp_color(background, panel_fill, t);
            painter.rect_filled(morph_rect, corner, fill);

            let stroke_alpha = (32.0 * t).round() as u8;
            if stroke_alpha > 1 {
                let stroke = egui::Color32::from_rgba_premultiplied(
                    accent.r(),
                    accent.g(),
                    accent.b(),
                    stroke_alpha,
                );
                painter.rect_stroke(
                    morph_rect,
                    corner,
                    egui::Stroke::new(1.0, stroke),
                    egui::StrokeKind::Middle,
                );
            }

            // Clock fades out during the first third of the morph.
            if t < 0.9 {
                let island_alpha = (1.0 - self.launcher.anim * 3.0).clamp(0.0, 1.0);
                match self.mode {
                    IslandMode::Clock => modes::draw_clock_mode(
                        &painter,
                        pill_rect,
                        &self.cfg,
                        &self.clock,
                        accent,
                        &font_id,
                        self.visualizer_visibility,
                        &self.display_bands,
                        island_alpha,
                    ),
                    IslandMode::Notification => {
                        let (small, big) = if let Some(n) = &self.notif {
                            if n.app == "Screenshot" {
                                (n.app.clone(), n.title.clone())
                            } else {
                                let img = format!("{} on {}", n.title, n.app);
                                (img, n.body.clone())
                            }
                        } else {
                            let m = win::media_text().map(|m| (m.title, m.artist)).unwrap_or_default();
                            ("Now Playing".to_string(), m.0)
                        };
                        modes::draw_notification_mode(
                            &painter,
                            pill_rect,
                            &self.cfg,
                            accent,
                            &small,
                            &big,
                            island_alpha,
                        );
                    }
                    IslandMode::Volume => modes::draw_volume_mode(
                        &painter,
                        pill_rect,
                        accent,
                        self.display_volume,
                        island_alpha,
                        win::get_mute(),
                        self.vol_icon_on.as_ref().unwrap(),
                        self.vol_icon_off.as_ref().unwrap(),
                    ),
                }
            }

            // Launcher content fades in over the last two thirds.
            let content_alpha = ((t - 0.3) / 0.7).clamp(0.0, 1.0);
            if content_alpha > 0.01 {
                let highlight = Config::parse_color(&self.cfg.launcher_highlight);
                let launcher_bg = Config::parse_color(&self.cfg.launcher_background);
                self.launcher.show(
                    ctx,
                    morph_rect.min,
                    morph_rect.width(),
                    &self.cfg,
                    accent,
                    highlight,
                    launcher_bg,
                    self.search_icon.as_ref().unwrap(),
                    dt,
                    content_alpha,
                );
            }

            // Clicking outside the launcher panel (and not on the pill) closes it.
            if self.launcher.open {
                let clicked_outside = ctx.input(|i| {
                    i.pointer.any_pressed()
                        && i.pointer.interact_pos().map(|p| {
                            !morph_rect.contains(p) && !pill_rect.contains(p)
                        }).unwrap_or(false)
                });
                if clicked_outside {
                    self.set_launcher_open(ctx, false);
                }
            }
        }

        // --- Power menu (session actions) ---
        if self.power.anim > 0.001 {
            let power_alpha = self.power.anim.clamp(0.0, 1.0);
            let pe = ease_out_cubic(self.power.anim);
            let menu_width = 560.0f32.min(viewport_rect.width() - 40.0);
            let target = egui::Rect::from_min_size(
                egui::pos2(viewport_rect.center().x - menu_width / 2.0, pill_rect.top()),
                egui::vec2(menu_width, power::POWER_MENU_HEIGHT),
            );
            // Grow out of the clock pill for a smooth transition.
            let panel = lerp_rect(pill_rect, target, pe);
            let highlight = Config::parse_color(&self.cfg.launcher_highlight);
            if let Some(icons) = &self.power_icons {
                egui::Area::new(egui::Id::new("power_menu"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(panel.min)
                    .movable(false)
                    .show(ctx, |ui| {
                        if let Some(action) = power::draw_power_menu(
                            ui,
                            panel,
                            &self.cfg,
                            highlight,
                            icons,
                            power_alpha,
                            &mut self.power.selected,
                        ) {
                            win::perform_power_action(action);
                            self.power.close();
                        }
                    });
            }
        }

        // --- Control center ---
        if self.control_anim > 0.001 {
            let ce = ease_out_cubic(self.control_anim);
            let cc_width = 560.0f32.min(viewport_rect.width() - 40.0);
            let cc_height = 500.0f32;
            let target = egui::Rect::from_min_size(
                egui::pos2(viewport_rect.center().x - cc_width / 2.0, pill_rect.top()),
                egui::vec2(cc_width, cc_height),
            );
            // Grow out of the clock pill for a smooth transition.
            let panel = lerp_rect(pill_rect, target, ce);
            let accent = Config::parse_color(&self.cfg.accent_color);
            let media_owned = win::media_text().and_then(|m| {
                self.media_tex.clone().map(|tex| (tex, m.title, m.artist))
            });
            let media_ref = media_owned
                .as_ref()
                .map(|(tex, title, artist)| (tex, title.as_str(), artist.as_str()));
            if let Some(icons) = &self.control_icons {
                let mut open_power = false;
                egui::Area::new(egui::Id::new("control_center"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(panel.min)
                    .movable(false)
                    .show(ctx, |ui| {
                        let actions = modes::draw_control_center(
                            ui,
                            panel,
                            &self.cfg,
                            accent,
                            &mut self.control_state,
                            icons,
                            media_ref,
                            win::get_volume(),
                        );
                        for action in actions {
                            match action {
                                modes::ControlAction::OpenSubmenu(modes::ControlSubmenu::Power) => {
                                    open_power = true;
                                }
                                modes::ControlAction::OpenSubmenu(sub) => {
                                    self.control_state.submenu = sub;
                                }
                                modes::ControlAction::CloseSubmenu => {
                                    self.control_state.submenu = modes::ControlSubmenu::None;
                                }
                                modes::ControlAction::ToggleMute => {
                                    win::toggle_mute();
                                }
                                modes::ControlAction::ToggleWifi => {
                                    // Toggle the actual radio state.
                                    let on = !win::wifi_radio_is_on();
                                    win::set_wifi_radio(on);
                                }
                                modes::ControlAction::ToggleBluetooth => {
                                    let on = !win::bluetooth_radio_is_on();
                                    win::set_bluetooth_radio(on);
                                }
                                modes::ControlAction::ToggleDnd => {
                                    self.control_state.dnd = !self.control_state.dnd;
                                    win::set_quiet_hours(self.control_state.dnd);
                                }
                                modes::ControlAction::ToggleNightLight => {
                                    self.control_state.night_light = !self.control_state.night_light;
                                }
                                modes::ControlAction::SetDefaultAudio(id) => {
                                    win::set_default_audio_output(&id);
                                    self.control_state.submenu = modes::ControlSubmenu::None;
                                }
                                modes::ControlAction::ConnectWifi(ssid) => {
                                    let _ = ssid;
                                    // No in-place connect available; close submenu.
                                    self.control_state.submenu = modes::ControlSubmenu::None;
                                }
                                modes::ControlAction::OpenWallpapers => {
                                    self.control_open = false;
                                    self.control_anim = 0.0;
                                    self.control_state.submenu = modes::ControlSubmenu::None;
                                    self.wallpaper.open = true;
                                    if self.wallpaper.open {
                                        self.wallpaper.refresh();
                                    }
                                }
                                modes::ControlAction::OpenThemes => {
                                    self.control_open = false;
                                    self.control_anim = 0.0;
                                    self.control_state.submenu = modes::ControlSubmenu::None;
                                    self.themes.open = true;
                                    if self.themes.open {
                                        self.themes.refresh();
                                    }
                                }
                            }
                        }
                    });
                // Click outside → close.
                let click_outside = ctx.input(|i| {
                    i.pointer.any_pressed()
                        && i.pointer.interact_pos().map(|p| !target.contains(p)).unwrap_or(false)
                });
                if click_outside {
                    self.control_open = false;
                    self.control_state.submenu = modes::ControlSubmenu::None;
                } else if open_power {
                    self.control_open = false;
                    self.control_anim = 0.0;
                    self.control_state.submenu = modes::ControlSubmenu::None;
                    self.power.open_menu();
                    self.focus_window();
                }
            }
        }

        // --- Wallpaper panel ---
        if self.wallpaper.anim > 0.001 {
            let we = ease_out_cubic(self.wallpaper.anim);
            let wp_width = 560.0f32.min(viewport_rect.width() - 40.0);
            let wp_height = 420.0f32;
            let target = egui::Rect::from_min_size(
                egui::pos2(viewport_rect.center().x - wp_width / 2.0, pill_rect.top()),
                egui::vec2(wp_width, wp_height),
            );
            // Grow out of the clock pill for a smooth transition.
            let panel = lerp_rect(pill_rect, target, we);
            let accent = Config::parse_color(&self.cfg.accent_color);
            let mut picked: Option<String> = None;
            egui::Area::new(egui::Id::new("wallpaper_panel"))
                .order(egui::Order::Foreground)
                .fixed_pos(panel.min)
                .movable(false)
                .show(ctx, |ui| {
                    if let Some(name) =
                        self.wallpaper.draw(ui, panel, &self.cfg, accent, we)
                    {
                        picked = Some(name);
                    }
                });
            // Click outside → close.
            let click_outside = ctx.input(|i| {
                i.pointer.any_pressed()
                    && i.pointer.interact_pos().map(|p| !target.contains(p)).unwrap_or(false)
            });
            if click_outside {
                self.wallpaper.close();
            } else if let Some(name) = picked {
                win::set_wallpaper(&name);
                self.wallpaper.refresh_after_selection(&name);
            }
        }

        // --- Themes panel ---
        if self.themes.anim > 0.001 {
            let te = ease_out_cubic(self.themes.anim);
            let th_width = 560.0f32.min(viewport_rect.width() - 40.0);
            let th_height = 420.0f32;
            let target = egui::Rect::from_min_size(
                egui::pos2(viewport_rect.center().x - th_width / 2.0, pill_rect.top()),
                egui::vec2(th_width, th_height),
            );
            let panel = lerp_rect(pill_rect, target, te);
            let accent = Config::parse_color(&self.cfg.accent_color);
            let mut picked_theme: Option<String> = None;
            egui::Area::new(egui::Id::new("themes_panel"))
                .order(egui::Order::Foreground)
                .fixed_pos(panel.min)
                .movable(false)
                .show(ctx, |ui| {
                    if let Some(name) = self.themes.draw(ui, panel, &self.cfg, accent, te) {
                        picked_theme = Some(name);
                    }
                });
            let click_outside = ctx.input(|i| {
                i.pointer.any_pressed()
                    && i.pointer.interact_pos().map(|p| !target.contains(p)).unwrap_or(false)
            });
            if click_outside {
                self.themes.close();
            } else if let Some(name) = picked_theme {
                win::apply_theme(&name);
                self.cfg = Config::load();
                self.themes.close();
            }
        }

        // --- Alt+Tab app switcher overlay ---
        // Render while active OR animating out, growing from the pill to a
        // centered panel.
        if self.switcher.active || self.switcher.anim > 0.001 {
            let se = ease_out_cubic(self.switcher.anim);
            let sw_w = viewport_rect.width().min(760.0);
            let sw_h = viewport_rect.height().min(520.0);
            let target = egui::Rect::from_center_size(viewport_rect.center(), egui::vec2(sw_w, sw_h));
            // Grow out of the clock pill (top-center of the viewport).
            let start = egui::Rect::from_center_size(
                egui::pos2(viewport_rect.center().x, pill_rect.center().y),
                egui::vec2(pill_rect.width(), pill_rect.height()),
            );
            let sw_rect = lerp_rect(start, target, se);
            let accent = Config::parse_color(&self.cfg.accent_color);
            let mut double_clicked: Option<usize> = None;
            egui::Area::new(egui::Id::new("app_switcher"))
                .order(egui::Order::Foreground)
                .fixed_pos(sw_rect.min)
                .movable(false)
                .show(ctx, |ui| {
                    if let Some(sel) = self.switcher.draw(ui, sw_rect, &self.cfg, accent, se) {
                        double_clicked = Some(sel);
                    }
                });
            if let Some(sel) = double_clicked {
                self.switcher.index = sel;
                self.switcher.activate_selected();
            }
        }

        // --- Repaint scheduling ---
        let launcher_animating = self.launcher.anim > 0.001 && self.launcher.anim < 0.999;
        let hover_animating = self.hover_anim > 0.001 && self.hover_anim < 0.999;
        let power_animating = self.power.anim > 0.001 && self.power.anim < 0.999;
        let control_animating = self.control_anim > 0.001 && self.control_anim < 0.999;
        let wallpaper_animating = self.wallpaper.anim > 0.001 && self.wallpaper.anim < 0.999;
        let themes_animating = self.themes.anim > 0.001 && self.themes.anim < 0.999;
        let switcher_animating = self.switcher.active || (self.switcher.anim > 0.001 && self.switcher.anim < 0.999);
        let animating = self.y_animating || self.width_animating || launcher_animating || hover_animating || power_animating || control_animating || switcher_animating || wallpaper_animating || themes_animating;
        let viz_active = self.visualizer_visibility > 0.01;
        let notification_active = self.mode == IslandMode::Notification;
        // A marquee-scrolling media title needs continuous repaints even after
        // the hover animation settles.
        let hover_expanded = self.hover_anim > 0.05 && self.mode == IslandMode::Clock;
        let scrolling_title = hover_expanded && win::media_text().map(|m| m.title).is_some();

        if self.switcher.active {
            // Alt+Tab switcher needs immediate repaints so Tab-taps / highlight
            // update as fast as the key repeats.
            ctx.request_repaint();
        } else if self.switcher.anim > 0.001 {
            // Switcher still animating out.
            ctx.request_repaint();
        } else if animating {
            // During animation: request repaint immediately, no sleep
            ctx.request_repaint();
        } else if viz_active {
            ctx.request_repaint_after(Duration::from_millis(16));
        } else if scrolling_title {
            ctx.request_repaint_after(Duration::from_millis(16));
        } else if notification_active {
            ctx.request_repaint_after(Duration::from_millis(50));
        } else if self.launcher.open {
            ctx.request_repaint_after(Duration::from_millis(50));
        } else if self.power.open {
            ctx.request_repaint_after(Duration::from_millis(50));
        } else if self.control_open {
            ctx.request_repaint_after(Duration::from_millis(50));
        } else if self.wallpaper.open {
            ctx.request_repaint_after(Duration::from_millis(50));
        } else if self.hidden {
            // Hidden: nothing is rendered, so idle at a low cadence. Volume
            // keys / notifications are still picked up by the (slowed) polls.
            ctx.request_repaint_after(Duration::from_millis(150));
        } else {
            // Idle: sleep until next poll
            let next_due = self.polls.earliest();
            if next_due <= now {
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(next_due.saturating_duration_since(now));
            }
        }
    }
}

fn formatted_clock(cfg: &Config) -> String {
    match cfg.clock_format.as_str() {
        "short" => Local::now().format("%H:%M").to_string(),
        _ => Local::now().format("%H:%M:%S").to_string(),
    }
}

/// Whether we have a custom notification implementation for this app.
/// Only these apps' notifications are surfaced; others are ignored.
fn is_supported_notification_app(app: &str) -> bool {
    app.eq_ignore_ascii_case("Discord")
}

/// e.g. "Sat, 22 Aug"
fn formatted_day() -> String {
    Local::now().format("%a, %d %b").to_string()
}

const VOLUME_SVG_ON: &str = include_str!("assets/volume.svg");
const VOLUME_SVG_OFF: &str = include_str!("assets/volume_muted.svg");
const WIFI_SVG: &str = include_str!("assets/wifi.svg");
const ETHERNET_SVG: &str = include_str!("assets/ethernet.svg");
const BLUETOOTH_SVG: &str = include_str!("assets/bluetooth.svg");
const BATTERY_SVG: &str = include_str!("assets/battery.svg");
const SEARCH_SVG: &str = include_str!("assets/search.svg");
const POWER_LOCK_SVG: &str = include_str!("assets/power_lock.svg");
const POWER_SUSPEND_SVG: &str = include_str!("assets/power_suspend.svg");
const POWER_LOGOUT_SVG: &str = include_str!("assets/power_logout.svg");
const POWER_REBOOT_SVG: &str = include_str!("assets/power_reboot.svg");
const POWER_OFF_SVG: &str = include_str!("assets/power_off.svg");
const CONTROL_AUDIO_SVG: &str = include_str!("assets/control_audio.svg");
const CONTROL_MOON_SVG: &str = include_str!("assets/control_moon.svg");
const CONTROL_NIGHT_SVG: &str = include_str!("assets/control_night.svg");
const WALLPAPER_SVG: &str = include_str!("assets/wallpaper.svg");
const THEME_SVG: &str = include_str!("assets/theme.svg");

/// Rasterizes an SVG asset into an egui texture at the given pixel size.
/// The SVG is drawn white so it can be tinted with any color when painted.
fn render_svg_texture(ctx: &egui::Context, name: &str, svg: &str, px: f32) -> egui::TextureHandle {
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_str(svg, &opt).expect("invalid built-in SVG asset");
    let size = tree.size();
    let w = px.round().max(1.0) as u32;
    let h = px.round().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(w, h).expect("pixmap alloc");
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(
            w as f32 / size.width(),
            h as f32 / size.height(),
        ),
        &mut pixmap.as_mut(),
    );
    // resvg emits premultiplied RGBA.
    let image = egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], pixmap.data());
    ctx.load_texture(name.to_owned(), image, egui::TextureOptions::LINEAR)
}

fn load_custom_font(ctx: &egui::Context, font_name: &str) {
    if font_name.is_empty() || font_name == "proportional" || font_name == "monospace" {
        ctx.set_fonts(egui::FontDefinitions::default());
        return;
    }
    let windir = std::env::var("WINDIR")
        .or_else(|_| std::env::var("SystemRoot"))
        .unwrap_or_else(|_| "C:\\Windows".into());
    let fonts_dir = std::path::PathBuf::from(windir).join("Fonts");
    let mut font_data: Option<Vec<u8>> = None;
    for ext in [".ttf", ".otf", ".TTF", ".OTF"] {
        let path = fonts_dir.join(format!("{font_name}{ext}"));
        if let Ok(data) = std::fs::read(&path) {
            font_data = Some(data);
            break;
        }
    }
    if let Some(data) = font_data {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "custom".to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(data)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .and_modify(|family| family.insert(0, "custom".to_owned()));
        ctx.set_fonts(fonts);
    }
}

fn config_file_mtime() -> SystemTime {
    std::fs::metadata(Config::config_path())
        .and_then(|meta| meta.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn main() -> eframe::Result {
    let cfg = Config::load();

    // Start media poller on background thread (reads SMTC status every 200ms)
    win::start_media_poller();
    // Start WASAPI loopback spectrum analyzer (feeds the pill visualizer)
    win::start_audio_spectrum_poller();
    // Start media-key hook (volume keys work even without explorer as the shell)
    win::start_media_key_hook();
    // Start system-status poller (battery / Wi-Fi / Bluetooth)
    win::start_status_poller();
    // Start notification listener (reads Windows toast notifications)
    win::start_notification_listener();

    // Reset the work area so maximised apps fill the whole screen
    // (needed when Explorer/taskbar is not running).
    win::reset_work_area();

    let viewport = island_viewport_size(&cfg, 0.0, 0.0, false, false, false, false, false, 0.0, 0.0, 0.0, 0.0);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([viewport.x, viewport.y])
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_taskbar(false)
            .with_title("strbar"),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    eframe::run_native(
        "strbar",
        options,
        Box::new(|cc| Ok(Box::new(DynamicIslandApp::new(cc)))),
    )
}
