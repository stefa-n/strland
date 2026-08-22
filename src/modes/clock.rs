use super::{ease_out_cubic, lerp, smoothstep01};
use crate::config::Config;
use crate::win;
use eframe::egui;

/// Core spectrum-bar renderer. Bars with a near-silent level are skipped
/// entirely — otherwise they render as tiny glow stubs that read as a
/// "white border" beside the song title.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_bars(
    painter: &egui::Painter,
    cfg: &Config,
    accent: egui::Color32,
    visualizer_visibility: f32,
    bands: &[f32; win::AUDIO_BANDS],
    x_start: f32,
    center_y: f32,
    max_bar_h: f32,
    alpha: f32,
) {
    if visualizer_visibility <= 0.01 || alpha <= 0.01 {
        return;
    }
    let bar_w = cfg.visualizer_bar_width;
    let bar_gap = cfg.visualizer_bar_gap;
    let viz_alpha = (255.0 * visualizer_visibility * alpha).round() as u8;
    let viz_color = egui::Color32::from_rgba_premultiplied(
        accent.r(),
        accent.g(),
        accent.b(),
        viz_alpha,
    );

    for (i, level) in bands.iter().enumerate() {
        let level = level.clamp(0.0, 1.0);
        if level < 0.03 {
            continue;
        }
        let h = (level * max_bar_h).max(1.0);
        let x = x_start + i as f32 * (bar_w + bar_gap);
        let y_bottom = center_y + max_bar_h / 2.0;
        let bar_rect = egui::Rect::from_min_size(
            egui::pos2(x, y_bottom - h),
            egui::vec2(bar_w, h),
        );
        painter.rect_filled(bar_rect, 1, viz_color);
    }
}

/// Draws the collapsed clock pill (used during launcher morph fade-out).
pub fn draw_clock_mode(
    painter: &egui::Painter,
    pill_rect: egui::Rect,
    cfg: &Config,
    clock: &str,
    accent: egui::Color32,
    font_id: &egui::FontId,
    visualizer_visibility: f32,
    bands: &[f32; win::AUDIO_BANDS],
    alpha: f32,
) {
    if alpha <= 0.01 {
        return;
    }
    let default_x = pill_rect.left() + cfg.visualizer_left_inset;
    draw_bars(
        painter,
        cfg,
        accent,
        visualizer_visibility,
        bands,
        default_x,
        pill_rect.center().y,
        cfg.height * cfg.visualizer_height_ratio,
        alpha,
    );

    let center = pill_rect.center() + egui::vec2(0.0, cfg.text_offset_y);
    let text_alpha = (255.0 * alpha).round() as u8;
    let text_color = egui::Color32::from_rgba_premultiplied(accent.r(), accent.g(), accent.b(), text_alpha);
    painter.text(center, egui::Align2::CENTER_CENTER, clock, font_id.clone(), text_color);
}

/// Continuously drawn island content for Clock mode.
///
/// `t` is the RAW hover progress 0..1 (not pre-eased); this function applies
/// easing once. Everything interpolates smoothly: the clock grows from the
/// resting size, the day fades in beneath it, and when media is present the
/// artwork zooms-in/fades-in and the title/artist (with the spectrum bars,
/// which shrink to sit inline with the title) slide up beside it. On collapse
/// all of that reverses — the picture zooms out and disappears.
#[allow(clippy::too_many_arguments)]
pub fn draw_clock_island(
    painter: &egui::Painter,
    pill_rect: egui::Rect,
    cfg: &Config,
    clock: &str,
    day: &str,
    accent: egui::Color32,
    font_id: &egui::FontId,
    visualizer_visibility: f32,
    bands: &[f32; win::AUDIO_BANDS],
    media: Option<(&egui::TextureHandle, &str, &str)>, // art, title, artist
    t: f32,
    marquee_time: f32,
    alpha: f32,
) {
    let t = t.clamp(0.0, 1.0);
    let e = ease_out_cubic(t);
    let alpha = alpha.clamp(0.0, 1.0);
    let cx = pill_rect.center().x;
    let cy = pill_rect.center().y;

    let default_x = pill_rect.left() + cfg.visualizer_left_inset;
    let normal_max_h = cfg.height * cfg.visualizer_height_ratio;

    let has_media = media.is_some() && t > 0.05;

    let clock_size = lerp(font_id.size, font_id.size + 6.0, e);
    let clock_y = lerp(cy + cfg.text_offset_y, cy - 11.0 + cfg.text_offset_y, e);
    painter.text(
        egui::pos2(cx, clock_y),
        egui::Align2::CENTER_CENTER,
        clock,
        egui::FontId::new(clock_size, font_id.family.clone()),
        egui::Color32::from_rgba_premultiplied(accent.r(), accent.g(), accent.b(), (255.0 * alpha) as u8),
    );

    let day_alpha = smoothstep01((e - 0.5) / 0.5) * alpha;
    if day_alpha > 0.01 {
        let day_font = egui::FontId::new(font_id.size - 5.0, font_id.family.clone());
        painter.text(
            egui::pos2(cx, cy + 13.0 + cfg.text_offset_y),
            egui::Align2::CENTER_CENTER,
            day,
            day_font,
            egui::Color32::from_rgba_premultiplied(
                accent.r(),
                accent.g(),
                accent.b(),
                ((255.0 * 0.62) * day_alpha).round() as u8,
            ),
        );
    }

    if let (Some((art_tex, title_text, artist_text)), true) = (media, has_media) {
        let art_side_full = (pill_rect.height() - 34.0).clamp(40.0, 58.0);
        let art_scale = lerp(0.4, 1.0, smoothstep01((e - 0.25) / 0.6));
        let art_side = art_side_full * art_scale;
        let art_center_x = pill_rect.left() + 18.0 + art_side_full / 2.0;
        let art_rect = egui::Rect::from_center_size(
            egui::pos2(art_center_x, cy),
            egui::vec2(art_side, art_side),
        );
        let art_alpha = (smoothstep01((e - 0.25) / 0.6) * 255.0).round() as u8;
        if art_alpha > 1 {
            painter.image(
                art_tex.id(),
                art_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::from_rgba_premultiplied(255, 255, 255, art_alpha),
            );
        }

        let title_font = egui::FontId::new(font_id.size - 1.0, font_id.family.clone());
        let artist_font = egui::FontId::new(font_id.size - 4.0, font_id.family.clone());
        let content_alpha = smoothstep01((e - 0.45) / 0.5);
        if content_alpha > 0.01 {
            let title_galley = painter.layout_no_wrap(
                title_text.to_owned(),
                title_font.clone(),
                egui::Color32::from_rgba_premultiplied(
                    255,
                    255,
                    255,
                    (content_alpha * 255.0).round() as u8,
                ),
            );
            let artist_galley = painter.layout_no_wrap(
                artist_text.to_owned(),
                artist_font,
                egui::Color32::from_rgba_premultiplied(
                    accent.r(),
                    accent.g(),
                    accent.b(),
                    ((255.0 * 0.62) * content_alpha).round() as u8,
                ),
            );

            let bars_total = win::AUDIO_BANDS as f32 * cfg.visualizer_bar_width
                + (win::AUDIO_BANDS - 1) as f32 * cfg.visualizer_bar_gap;
            let bars_x = art_rect.right() + 12.0;
            let text_left = bars_x + bars_total + 14.0;

            let th = title_galley.size().y;
            let ah = artist_galley.size().y;
            let gap = 3.0;
            let row1_h = th;
            let y0 = cy - (row1_h + gap + ah) / 2.0;

            let clip_right = (cx - 90.0).max(text_left + 40.0);
            let avail_width = clip_right - text_left;

            let title_w = title_galley.size().x;
            let title_left = if title_w > avail_width {
                let margin = 18.0;
                let overshoot = (title_w - avail_width) + margin;
                let speed = 42.0;
                let pause = 1.0;
                let travel = overshoot / speed;
                let cycle = (travel + pause) * 2.0;
                let phase = marquee_time % cycle;
                let offset = if phase < travel {
                    phase * speed
                } else if phase < travel + pause {
                    overshoot
                } else if phase < travel * 2.0 + pause {
                    overshoot - (phase - travel - pause) * speed
                } else {
                    0.0
                };
                text_left - offset
            } else {
                text_left
            };

            let clip = egui::Rect::from_min_size(
                egui::pos2(bars_x, y0 - 6.0),
                egui::vec2(clip_right - bars_x, row1_h + gap + ah + 12.0),
            );
            let clipped = painter.with_clip_rect(clip);

            let title_clip = egui::Rect::from_min_size(
                egui::pos2(text_left, y0 - 6.0),
                egui::vec2(clip_right - text_left, row1_h + gap + ah + 12.0),
            );
            let title_clipped = painter.with_clip_rect(title_clip);

            let slide = (1.0 - e) * 8.0;
            title_clipped.galley(
                egui::pos2(title_left, y0 + slide),
                title_galley,
                egui::Color32::TRANSPARENT,
            );
            clipped.galley(
                egui::pos2(bars_x, y0 + row1_h + gap + slide),
                artist_galley,
                egui::Color32::TRANSPARENT,
            );

            let title_center_y = y0 + row1_h / 2.0;
            let small_max_h = (th * 0.5).min(normal_max_h);
            draw_bars(
                painter,
                cfg,
                accent,
                visualizer_visibility,
                bands,
                lerp(default_x, bars_x, e),
                lerp(cy, title_center_y, e),
                lerp(normal_max_h, small_max_h, e),
                1.0,
            );
        } else {
            draw_bars(painter, cfg, accent, visualizer_visibility, bands, default_x, cy, normal_max_h, 1.0);
        }
        return;
    }

    draw_bars(painter, cfg, accent, visualizer_visibility, bands, default_x, cy, normal_max_h, 1.0);
}
