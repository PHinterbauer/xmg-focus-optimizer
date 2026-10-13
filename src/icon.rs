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

/// Writes the icon into the window's title bar via `WM_SETICON`.
#[cfg(windows)]
pub fn set_window_icon(hwnd: std::os::windows::raw::HWND, mode: Mode) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_SETICON, ICON_SMALL, ICON_BIG};
    let _ = hwnd;

    let h = HWND(hwnd as *mut std::ffi::c_void);
    // Tray icons are 32-bit ARGB DIBs; build a small and a large one.
    for (size, which) in [(32usize, ICON_BIG), (16usize, ICON_SMALL)] {
        let img = draw(size as u32, mode);
        // BGRA + unused mask, bottom-up.
        let mut dib = vec![0u8; 40 + (size as u32 * size as u32 * 4)];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes()); // biSize
        dib[4..8].copy_from_slice(&(size as i32).to_le_bytes()); // biWidth
        dib[8..12].copy_from_slice(&((size as i32) * 2).to_le_bytes()); // biHeight (XOR+AND)
        dib[12..14].copy_from_slice(&1u16.to_le_bytes()); // biPlanes
        dib[14..16].copy_from_slice(&32u16.to_le_bytes()); // biBitCount
        dib[16..20].copy_from_slice(&0u32.to_le_bytes()); // BI_RGB

        let stride = size as u32 * 4;
        for row in 0..size as u32 {
            let src_row = (size as u32 - 1 - row) as usize;
            let src_off = src_row * stride as usize;
            let dst_off = (40 + row as usize * stride as usize) as usize;
            for i in 0..size as u32 {
                let r = img.px[src_off + i as usize * 4] as u32;
                let g = img.px[src_off + i as usize * 4 + 1] as u32;
                let b = img.px[src_off + i as usize * 4 + 2] as u32;
                let a = img.px[src_off + i as usize * 4 + 3] as u32;
                let o = dst_off + i as usize * 4;
                dib[o] = (b * a / 255) as u8;
                dib[o + 1] = (g * a / 255) as u8;
                dib[o + 2] = (r * a / 255) as u8;
                dib[o + 3] = a as u8;
            }
        }

        unsafe {
            let hicon = windows::Win32::UI::WindowsAndMessaging::CreateIconFromResourceEx(
                Some(dib.as_ptr()),
                None,
                true,
                0,
                0,
                size as i32,
                size as i32,
                None,
            );
            if let Ok(hicon) = hicon {
                let _ = SendMessageW(h, WM_SETICON, Some(which.0 as usize), Some(windows::Win32::Foundation::LPARAM(hicon.0 as isize)));
                let _ = windows::Win32::Graphics::Gdi::DeleteObject(hicon.into());
            }
        }
    }
}