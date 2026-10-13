//! The application icon, drawn in code so the window, the buttons and the tray
//! icon can never drift apart.
//!
//! A squircle plate with a power glyph. Quiet is calm blue, Performance is warm
//! amber - so the mode is readable at a glance even from a 16 px tray icon.

use std::sync::OnceLock;

use crate::profile::Mode;

// ------------------------------------------------------------- geometry ---

struct Rgba {
    w: u32,
    h: u32,
    px: Vec<u8>, // straight alpha, RGBA
}

fn mode_colors(mode: Mode) -> ([u8; 3], [u8; 3]) {
    match mode {
        // deep indigo blue -> sky
        Mode::Quiet => ([30, 64, 175], [14, 182, 225]),
        // deep amber -> warm orange
        Mode::Performance => ([176, 58, 12], [249, 168, 37]),
    }
}

/// Renders the icon at `size` pixels.
fn draw(size: u32, mode: Mode) -> Rgba {
    let (c0, c1) = mode_colors(mode);
    let s = size as f32;
    let pad = s * 0.075;
    let w = s - pad * 2.0;
    let rad = (w * 0.30).max(0.001);
    let cx = pad + w / 2.0;
    let cy = pad + w / 2.0;

    let mut px = vec![0u8; (size * size * 4) as usize];

    // --- plate ------------------------------------------------------------
    for y in 0..size {
        for x in 0..size {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let dx = ((pad + w) - fx).max(fx - pad);
            let dy = ((pad + w) - fy).max(fy - pad);
            if dx < 0.0 || dy < 0.0 {
                continue;
            }
            // superellipse (n = 4) gives a fuller, more modern corner
            let q = (dx / rad).powi(4) + (dy / rad).powi(4);
            if q > 1.06 {
                continue;
            }
            // antialias across the last little band
            let a = ((1.06 - q) / 0.06).clamp(0.0, 1.0);
            let t = ((fx - pad) + (fy - pad)) / (2.0 * w);
            let i = ((y * size + x) * 4) as usize;
            px[i] = lerp(c0[0], c1[0], t);
            px[i + 1] = lerp(c0[1], c1[1], t);
            px[i + 2] = lerp(c0[2], c1[2], t);
            px[i + 3] = (a * 255.0) as u8;
        }
    }

    // --- power glyph ------------------------------------------------------
    let rr = w * 0.205;
    let sw = (w * 0.075).max(1.0);

    // ring: 284 degrees starting at -52, leaving a gap at the top
    let steps = (size as usize * 8).clamp(180, 1200);
    for i in 0..=steps {
        let deg = -52.0 + 284.0 * (i as f32 / steps as f32);
        let rad_a = deg.to_radians();
        let (sx, sy) = (rad_a.sin(), -rad_a.cos());
        disc(&mut px, size, cx + sx * rr, cy + sy * rr, sw / 2.0);
    }

    // stem, from just above the centre up through the gap
    let top = cy - rr * 1.12;
    let bot = cy + rr * 0.10;
    let mut y = top;
    while y <= bot {
        disc(&mut px, size, cx, y, sw / 2.0);
        y += 0.4;
    }

    // The glyph was composited opaquely white on top of whatever was there.
    Rgba { w: size, h: size, px }
}

/// Alpha-blends an opaque white disc onto the buffer.
fn disc(px: &mut [u8], size: u32, cx: f32, cy: f32, r: f32) {
    let ri = (r + 1.0).ceil() as i32;
    let cxi = cx.round() as i32;
    let cyi = cy.round() as i32;
    for y in (cyi - ri)..=(cyi + ri) {
        for x in (cxi - ri)..=(cxi + ri) {
            if x < 0 || y < 0 || x >= size as i32 || y >= size as i32 {
                continue;
            }
            let d = (((x as f32 - cx) * (x as f32 - cx) + (y as f32 - cy) * (y as f32 - cy)).sqrt())
                - r;
            let a = (0.5 - d).clamp(0.0, 1.0);
            if a <= 0.0 {
                continue;
            }
            let i = ((y as u32 * size + x as u32) * 4) as usize;
            let inv = 1.0 - a;
            px[i] = (px[i] as f32 * inv + 255.0 * a) as u8;
            px[i + 1] = (px[i + 1] as f32 * inv + 255.0 * a) as u8;
            px[i + 2] = (px[i + 2] as f32 * inv + 255.0 * a) as u8;
            px[i + 3] = (px[i + 3] as f32 * inv + 255.0 * a).max(px[i + 3] as f32) as u8;
        }
    }
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8
}

// ---------------------------------------------------------------- caching --

static QUIET_RGBA: OnceLock<Vec<u8>> = OnceLock::new();
static PERF_RGBA: OnceLock<Vec<u8>> = OnceLock::new();
static QUIET_TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
static PERF_TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
static QUIET_ICON: OnceLock<tray_icon::Icon> = OnceLock::new();
static PERF_ICON: OnceLock<tray_icon::Icon> = OnceLock::new();

fn rgba_for(mode: Mode) -> &'static Vec<u8> {
    match mode {
        Mode::Quiet => QUIET_RGBA.get_or_init(|| draw(64, mode).px),
        Mode::Performance => PERF_RGBA.get_or_init(|| draw(64, mode).px),
    }
}

/// egui texture for use inside the window.
pub fn texture(ctx: &egui::Context, mode: Mode) -> egui::TextureHandle {
    match mode {
        Mode::Quiet => QUIET_TEX
            .get_or_init(|| {
                let px = rgba_for(mode);
                let img = egui::ColorImage::from_rgba_unmultiplied([64, 64], px);
                ctx.load_texture("icon-quiet", img, egui::TextureOptions::LINEAR)
            })
            .clone(),
        Mode::Performance => PERF_TEX
            .get_or_init(|| {
                let px = rgba_for(mode);
                let img = egui::ColorImage::from_rgba_unmultiplied([64, 64], px);
                ctx.load_texture("icon-perf", img, egui::TextureOptions::LINEAR)
            })
            .clone(),
    }
}

/// Windows icon handle for the tray.
pub fn load(mode: Mode) -> tray_icon::Icon {
    match mode {
        Mode::Quiet => QUIET_ICON
            .get_or_init(|| {
                let px = rgba_for(mode);
                tray_icon::Icon::from_rgba(64, 64, px.clone())
            })
            .clone(),
        Mode::Performance => PERF_ICON
            .get_or_init(|| {
                let px = rgba_for(mode);
                tray_icon::Icon::from_rgba(64, 64, px.clone())
            })
            .clone(),
    }
}

/// Human-readable description of the icon colours, used in the About area.
pub fn describe(mode: Mode) -> &'static str {
    match mode {
        Mode::Quiet => "calm blue",
        Mode::Performance => "warm amber",
    }
}