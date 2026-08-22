use eframe::egui::Color32;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use toml::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_border_radius")]
    pub border_radius: f32,
    #[serde(default = "default_background")]
    pub background: String,
    #[serde(default = "default_accent_color")]
    pub accent_color: String,
    #[serde(default = "default_font")]
    pub font: String,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    #[serde(default = "default_clock_format")]
    pub clock_format: String,
    #[serde(default = "default_width")]
    pub width: f32,
    #[serde(default = "default_height")]
    pub height: f32,
    #[serde(default)]
    pub text_offset_y: f32,
    #[serde(default)]
    pub y_padding: f32,
    #[serde(default = "default_volume_expand_factor")]
    pub volume_expand_factor: f32,
    #[serde(default = "default_volume_timeout_ms")]
    pub volume_timeout_ms: u64,
    #[serde(default = "default_window_poll_ms")]
    pub window_poll_ms: u64,
    #[serde(default = "default_volume_poll_ms")]
    pub volume_poll_ms: u64,
    #[serde(default = "default_media_poll_ms")]
    pub media_poll_ms: u64,
    #[serde(default = "default_config_poll_ms")]
    pub config_poll_ms: u64,
    #[serde(default = "default_clock_poll_ms")]
    pub clock_poll_ms: u64,
    #[serde(default = "default_mode_anim_speed")]
    pub mode_anim_speed: f32,
    #[serde(default = "default_slide_anim_speed")]
    pub slide_anim_speed: f32,
    #[serde(default = "default_visualizer_anim_speed")]
    pub visualizer_anim_speed: f32,
    #[serde(default = "default_visualizer_wave_speed")]
    pub visualizer_wave_speed: f32,
    #[serde(default = "default_visualizer_left_inset")]
    pub visualizer_left_inset: f32,
    #[serde(default = "default_visualizer_bar_width")]
    pub visualizer_bar_width: f32,
    #[serde(default = "default_visualizer_bar_gap")]
    pub visualizer_bar_gap: f32,
    #[serde(default = "default_visualizer_height_ratio")]
    pub visualizer_height_ratio: f32,
    #[serde(default = "default_launcher_width")]
    pub launcher_width: f32,
    #[serde(default = "default_launcher_height")]
    pub launcher_height: f32,
    #[serde(default = "default_launcher_results")]
    pub launcher_results: usize,
    #[serde(default = "default_launcher_anim_speed")]
    pub launcher_anim_speed: f32,
    #[serde(default = "default_hover_width")]
    pub hover_width: f32,
    #[serde(default = "default_hover_height")]
    pub hover_height: f32,
    #[serde(default = "default_notification_timeout_ms")]
    pub notification_timeout_ms: u64,
    #[serde(default = "default_show_wifi")]
    pub show_wifi: bool,
    #[serde(default = "default_show_bluetooth")]
    pub show_bluetooth: bool,
    #[serde(default = "default_show_battery")]
    pub show_battery: bool,
    #[serde(default = "default_launcher_highlight")]
    pub launcher_highlight: String,
    #[serde(default = "default_status_button_background")]
    pub status_button_background: String,
    #[serde(default = "default_launcher_background")]
    pub launcher_background: String,
}

fn default_border_radius() -> f32 { 18.0 }
fn default_background() -> String { "#090909".into() }
fn default_accent_color() -> String { "#F2F2F2".into() }
fn default_font() -> String { "segoeui".into() }
fn default_font_size() -> f32 { 16.0 }
fn default_clock_format() -> String { "full".into() }
fn default_width() -> f32 { 260.0 }
fn default_height() -> f32 { 36.0 }
fn default_volume_expand_factor() -> f32 { 1.6 }
fn default_volume_timeout_ms() -> u64 { 1400 }
fn default_window_poll_ms() -> u64 { 40 }
fn default_volume_poll_ms() -> u64 { 32 }
fn default_media_poll_ms() -> u64 { 220 }
fn default_config_poll_ms() -> u64 { 1000 }
fn default_clock_poll_ms() -> u64 { 250 }
fn default_mode_anim_speed() -> f32 { 15.0 }
fn default_slide_anim_speed() -> f32 { 14.0 }
fn default_visualizer_anim_speed() -> f32 { 10.0 }
fn default_visualizer_wave_speed() -> f32 { 0.9 }
fn default_visualizer_left_inset() -> f32 { 14.0 }
fn default_visualizer_bar_width() -> f32 { 3.0 }
fn default_visualizer_bar_gap() -> f32 { 3.0 }
fn default_visualizer_height_ratio() -> f32 { 0.42 }
fn default_launcher_width() -> f32 { 560.0 }
fn default_launcher_height() -> f32 { 520.0 }
fn default_launcher_results() -> usize { 8 }
fn default_launcher_anim_speed() -> f32 { 13.0 }
fn default_hover_width() -> f32 { 560.0 }
fn default_hover_height() -> f32 { 96.0 }
fn default_notification_timeout_ms() -> u64 { 4000 }
fn default_show_wifi() -> bool { true }
fn default_show_bluetooth() -> bool { true }
fn default_show_battery() -> bool { true }
fn default_launcher_highlight() -> String { "#4C8BF5".into() }
fn default_status_button_background() -> String { "#2E2E32".into() }
fn default_launcher_background() -> String { "#090909".into() }

impl Default for Config {
    fn default() -> Self {
        Self {
            border_radius: default_border_radius(),
            background: default_background(),
            accent_color: default_accent_color(),
            font: default_font(),
            font_size: default_font_size(),
            clock_format: default_clock_format(),
            width: default_width(),
            height: default_height(),
            text_offset_y: 0.0,
            y_padding: 0.0,
            volume_expand_factor: default_volume_expand_factor(),
            volume_timeout_ms: default_volume_timeout_ms(),
            window_poll_ms: default_window_poll_ms(),
            volume_poll_ms: default_volume_poll_ms(),
            media_poll_ms: default_media_poll_ms(),
            config_poll_ms: default_config_poll_ms(),
            clock_poll_ms: default_clock_poll_ms(),
            mode_anim_speed: default_mode_anim_speed(),
            slide_anim_speed: default_slide_anim_speed(),
            visualizer_anim_speed: default_visualizer_anim_speed(),
            visualizer_wave_speed: default_visualizer_wave_speed(),
            visualizer_left_inset: default_visualizer_left_inset(),
            visualizer_bar_width: default_visualizer_bar_width(),
            visualizer_bar_gap: default_visualizer_bar_gap(),
            visualizer_height_ratio: default_visualizer_height_ratio(),
            launcher_width: default_launcher_width(),
            launcher_height: default_launcher_height(),
            launcher_results: default_launcher_results(),
            launcher_anim_speed: default_launcher_anim_speed(),
            hover_width: default_hover_width(),
            hover_height: default_hover_height(),
            notification_timeout_ms: default_notification_timeout_ms(),
            show_wifi: default_show_wifi(),
            show_bluetooth: default_show_bluetooth(),
            show_battery: default_show_battery(),
            launcher_highlight: default_launcher_highlight(),
            status_button_background: default_status_button_background(),
            launcher_background: default_launcher_background(),
        }
    }
}

impl Config {
    pub fn config_path() -> PathBuf {
        let home = dirs_or_home();
        home.join(".strland").join("strbar").join("config.toml")
    }

    pub fn load() -> Self {
        Self::load_and_heal().0
    }

    pub fn load_and_heal() -> (Self, bool) {
        let dir = {
            let home = dirs_or_home();
            home.join(".strland").join("strbar")
        };
        let _ = fs::create_dir_all(&dir);

        let path = Self::config_path();
        if !path.exists() {
            let default_cfg = Config::default();
            let rendered = toml::to_string_pretty(&default_cfg).unwrap_or_else(|_| DEFAULT_TEMPLATE.into());
            let _ = fs::write(&path, rendered);
            return (default_cfg, true);
        }

        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => {
                let default_cfg = Config::default();
                let rendered = toml::to_string_pretty(&default_cfg).unwrap_or_else(|_| DEFAULT_TEMPLATE.into());
                let _ = fs::write(&path, rendered);
                return (default_cfg, true);
            }
        };

        let default_value = Value::try_from(Config::default()).unwrap_or(Value::Table(Default::default()));
        let mut actual_value = match text.parse::<Value>() {
            Ok(value) => value,
            Err(_) => {
                let default_cfg = Config::default();
                let rendered = toml::to_string_pretty(&default_cfg).unwrap_or_else(|_| DEFAULT_TEMPLATE.into());
                let _ = fs::write(&path, rendered);
                return (default_cfg, true);
            }
        };

        let mut changed = migrate_legacy_keys(&mut actual_value);
        changed |= merge_missing_defaults(&mut actual_value, &default_value);

        let cfg = actual_value
            .clone()
            .try_into()
            .unwrap_or_else(|_| Config::default());

        if changed {
            if let Ok(rendered) = toml::to_string_pretty(&actual_value) {
                let _ = fs::write(&path, rendered);
            }
        }

        (cfg, changed)
    }

    pub fn parse_color(hex: &str) -> Color32 {
        let hex = hex.trim_start_matches('#');
        match hex.len() {
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
                Color32::from_rgb(r, g, b)
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
                let a = u8::from_str_radix(&hex[6..8], 16).unwrap_or(255);
                Color32::from_rgba_premultiplied(r, g, b, a)
            }
            _ => Color32::BLACK,
        }
    }
}

fn migrate_legacy_keys(value: &mut Value) -> bool {
    let mut changed = false;
    let Some(table) = value.as_table_mut() else {
        return false;
    };

    if !table.contains_key("accent_color") {
        if let Some(old_text_color) = table.get("text_color").cloned() {
            table.insert("accent_color".into(), old_text_color);
            changed = true;
        }
    }
    if table.remove("text_color").is_some() {
        changed = true;
    }
    changed
}

fn merge_missing_defaults(actual: &mut Value, defaults: &Value) -> bool {
    match (actual, defaults) {
        (Value::Table(actual_table), Value::Table(default_table)) => {
            let mut changed = false;
            for (key, default_value) in default_table {
                match actual_table.get_mut(key) {
                    Some(existing) => {
                        changed |= merge_missing_defaults(existing, default_value);
                    }
                    None => {
                        actual_table.insert(key.clone(), default_value.clone());
                        changed = true;
                    }
                }
            }
            changed
        }
        _ => false,
    }
}

fn dirs_or_home() -> PathBuf {
    if let Ok(home) = std::env::var("USERPROFILE") {
        return PathBuf::from(home);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home);
    }
    PathBuf::from(".")
}

const DEFAULT_TEMPLATE: &str = r##"border_radius = 18.0
background = "#090909"
accent_color = "#F2F2F2"
font = "segoeui"
font_size = 16.0
clock_format = "full"
width = 260.0
height = 36.0
text_offset_y = 0.0
y_padding = 0.0
volume_expand_factor = 1.6
volume_timeout_ms = 1400
window_poll_ms = 40
volume_poll_ms = 32
media_poll_ms = 220
config_poll_ms = 1000
clock_poll_ms = 250
mode_anim_speed = 15.0
slide_anim_speed = 14.0
visualizer_anim_speed = 10.0
visualizer_wave_speed = 0.9
visualizer_left_inset = 14.0
visualizer_bar_width = 3.0
visualizer_bar_gap = 3.0
visualizer_height_ratio = 0.42
launcher_width = 560.0
launcher_height = 520.0
launcher_results = 8
launcher_anim_speed = 13.0
hover_width = 560.0
hover_height = 96.0
notification_timeout_ms = 4000
show_wifi = true
show_bluetooth = true
show_battery = true
launcher_highlight = "#4C8BF5"
status_button_background = "#2E2E32"
launcher_background = "#090909"
"##;
