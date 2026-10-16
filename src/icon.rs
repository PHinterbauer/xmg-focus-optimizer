//! The application icon, drawn in code so the window and the tray icon can
//! never drift apart.
//!
//! Flat by design: one solid plate, one solid glyph, no gradient and no shadow.
//! Depth effects that look good at 256 px turn to mud at 16 px, which is the
//! size that matters most in the notification area.
//!
//! Two variants exist because two different backgrounds need them. The tray
//! icon follows the theme: a white plate on a light taskbar, the green plate
//! everywhere else. Inside the window and in the title bar there is only ever
//! the green brand plate, so the app looks the same everywhere it owns the
//! pixels.
//!
//! This must stay in step with `assets/New-Icon.ps1`, which draws the same
//! geometry at 1024 px and writes `assets/app.ico`. The angles below are in the
//! same convention GDI+ uses - degrees clockwise from 3 o'clock, y pointing
//! down - so the two agree by construction. The previous version used a
//! different convention and swept the gap onto the *left* while drawing a
//! vertical stem at the *top*, so the runtime icon was a broken power symbol
//! even though the .ico was fine.

// ------------------------------------------------------------- geometry ---

/// The two looks the icon comes in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Variant {
    /// Green plate, dark glyph. For a dark taskbar, and the one baked into
    /// `assets/app.ico`. The glyph is dark rather than white because white on
    /// this green washes out at tray size.
    Dark,
    /// White plate, deep-green glyph and outline. For a light taskbar.
    ///
    /// The outline is not decoration. A plain white plate would disappear
    /// completely against a white taskbar or a light window title bar, which is
    /// exactly where this variant is used.
    Light,
}

impl Variant {
    /// The plate fill.
    fn plate(self) -> [u8; 3] {
        match self {
            Variant::Dark => GREEN,
            Variant::Light => [255, 255, 255],
        }
    }

    /// The power symbol, and the light variant's outline.
    fn glyph(self) -> [u8; 3] {
        match self {
            Variant::Dark => INK,
            Variant::Light => GREEN_DEEP,
        }
    }
}

struct Rgba {
    w: u32,
    h: u32,
    px: Vec<u8>, // straight alpha, RGBA
}

/// The brand green, and the plate colour in `assets/New-Icon.ps1`.
const GREEN: [u8; 3] = [0, 230, 118];

/// Deep green for the light variant's glyph and outline, where the vivid
/// green would not read against white.
const GREEN_DEEP: [u8; 3] = [0, 122, 61];

/// Near-black for the dark variant's glyph, for the same reason in reverse:
/// white on the vivid plate washes out at 16 px.
const INK: [u8; 3] = [17, 17, 20];

/// Ring radius and stroke width, both as a fraction of the plate's side.
const RING: f32 = 0.205;
const STROKE: f32 = 0.090;

/// Outline weight for [`Variant::Light`], as a fraction of the plate's radius.
/// The absolute floor keeps it from thinning below a pixel at tray size.
const OUTLINE: f32 = 0.075;
const OUTLINE_MIN_PX: f32 = 1.0;

/// The ring sweeps 270 degrees from -45, leaving a 90 degree gap centred on
/// 270 degrees - straight up, which is where the stem is.
const ARC_START: f32 = -45.0;
const ARC_SWEEP: f32 = 270.0;

/// Renders the icon at `size` pixels.
fn draw(size: u32, variant: Variant) -> Rgba {
    let s = size as f32;
    let pad = s * 0.078;
    let w = s - pad * 2.0;
    let rad = (w / 2.0).max(0.001);
    let cx = pad + w / 2.0;
    let cy = pad + w / 2.0;
    let plate = variant.plate();
    let glyph = variant.glyph();
    let outline_w = (rad * OUTLINE).max(OUTLINE_MIN_PX);

    let mut px = vec![0u8; (size * size * 4) as usize];

    // --- plate: one flat fill ---------------------------------------------
    //
    // The superellipse has to be tested about the *centre*, so dx and dy are
    // offsets from (cx, cy). Measuring to the nearer edge instead - the
    // obvious-looking `(pad + w - fx).min(fx - pad)` - makes both terms at
    // least the radius, so q is never below 2, every pixel is rejected and the
    // icon comes out completely transparent. That is exactly what it did: the
    // tray and window icons have been blank since this code was written, which
    // never showed up because the binary has not been run.
    for y in 0..size {
        for x in 0..size {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            // superellipse (n = 4) gives a fuller, more modern corner
            let q = ((fx - cx).abs() / rad).powi(4) + ((fy - cy).abs() / rad).powi(4);
            if q > 1.06 {
                continue;
            }
            // 1.06 rather than 1.0 so the outline gets a thin feather and does
            // not look chewed at 16 px.
            let a = ((1.06 - q) / 0.06).clamp(0.0, 1.0);
            let i = ((y * size + x) * 4) as usize;
            let mut c = plate;
            if variant == Variant::Light {
                // q^(1/n) is 1 at the boundary and 0 at the centre, so
                // rad * (1 - q^0.25) approximates distance inwards from the
                // edge - good enough to lay an even border on a squircle.
                let d = rad * (1.0 - q.clamp(0.0, 1.0).powf(0.25));
                let o = (outline_w + 0.5 - d).clamp(0.0, 1.0);
                if o > 0.0 {
                    c = [
                        lerp(c[0], glyph[0], o),
                        lerp(c[1], glyph[1], o),
                        lerp(c[2], glyph[2], o),
                    ];
                }
            }
            px[i] = c[0];
            px[i + 1] = c[1];
            px[i + 2] = c[2];
            px[i + 3] = (a * 255.0) as u8;
        }
    }

    // --- power glyph ------------------------------------------------------
    //
    // Coverage is accumulated as a maximum, not blended in place. Stamping
    // semi-transparent discs straight onto the buffer compounds wherever they
    // overlap - the stroke comes out visibly fatter and brighter than the same
    // path stroked by GDI+ into assets/app.ico. Keeping coverage separate and
    // compositing once gives the correct width.
    let rr = w * RING;
    let sw = (w * STROKE).max(1.0);
    let mut cover = vec![0.0f32; (size * size) as usize];

    let steps = (size as usize * 8).clamp(180, 1200);
    for i in 0..=steps {
        let deg = (ARC_START + ARC_SWEEP * (i as f32 / steps as f32)).to_radians();
        disc(&mut cover, size, cx + deg.cos() * rr, cy + deg.sin() * rr, sw / 2.0);
    }

    // The stem rises through the gap to just past the ring's outer edge and
    // stops a little below the centre.
    let top = cy - (rr + sw * 0.62);
    let bot = cy + rr * 0.10;
    let mut y = top;
    while y <= bot {
        disc(&mut cover, size, cx, y, sw / 2.0);
        y += 0.4;
    }

    for y in 0..size {
        for x in 0..size {
            let g = cover[(y * size + x) as usize];
            if g <= 0.0 {
                continue;
            }
            let i = ((y * size + x) * 4) as usize;
            let inv = 1.0 - g;
            px[i] = (px[i] as f32 * inv + glyph[0] as f32 * g) as u8;
            px[i + 1] = (px[i + 1] as f32 * inv + glyph[1] as f32 * g) as u8;
            px[i + 2] = (px[i + 2] as f32 * inv + glyph[2] as f32 * g) as u8;
            // Alpha is deliberately left alone: the glyph only ever lands on the
            // already-opaque plate, and painting the edge would give it a
            // visible dark fringe where it crosses the squircle outline.
        }
    }

    Rgba { w: size, h: size, px }
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8
}

/// Widens `cover` with a round-capped disc of radius `r` centred at (cx, cy).
fn disc(cover: &mut [f32], size: u32, cx: f32, cy: f32, r: f32) {
    let ri = (r + 1.0).ceil() as i32;
    let cxi = cx.round() as i32;
    let cyi = cy.round() as i32;
    for y in (cyi - ri)..=(cyi + ri) {
        if y < 0 || y >= size as i32 {
            continue;
        }
        for x in (cxi - ri)..=(cxi + ri) {
            if x < 0 || x >= size as i32 {
                continue;
            }
            let d = (((x as f32 - cx) * (x as f32 - cx) + (y as f32 - cy) * (y as f32 - cy))
                .sqrt())
                - r;
            let a = (0.5 - d).clamp(0.0, 1.0);
            if a <= 0.0 {
                continue;
            }
            let i = (y as u32 * size + x as u32) as usize;
            if a > cover[i] {
                cover[i] = a;
            }
        }
    }
}

// ---------------------------------------------------------------- caching --
//
// Only the plain pixel data is cached globally. `tray_icon::Icon` wraps a raw
// pointer and is deliberately neither Send nor Sync, so it can never live in a
// `static`; it is rebuilt on demand instead, which costs nothing at this size.
//
// Two sets, one per variant. Each is drawn once and then reused, so switching
// theme costs a single 64x64 render the first time and nothing after that.

const ICON_PX: usize = 64;

static DARK_RGBA: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
static LIGHT_RGBA: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
static DARK_TEX: std::sync::OnceLock<egui::TextureHandle> = std::sync::OnceLock::new();

/// Straight RGBA pixels for the icon, rendered once per variant.
pub fn rgba(variant: Variant) -> &'static [u8] {
    match variant {
        Variant::Dark => DARK_RGBA.get_or_init(|| draw(ICON_PX as u32, Variant::Dark).px),
        Variant::Light => LIGHT_RGBA.get_or_init(|| draw(ICON_PX as u32, Variant::Light).px),
    }
}

/// egui texture for use inside the window.
///
/// Always the brand plate (green), whichever theme is active. The light
/// variant exists for the notification area, where a white taskbar needs it -
/// inside our own window the green plate reads better on both themes, and it
/// matches the title-bar icon and the executable's icon.
pub fn texture(ctx: &egui::Context) -> egui::TextureHandle {
    DARK_TEX
        .get_or_init(|| {
            let img = egui::ColorImage::from_rgba_unmultiplied(
                [ICON_PX, ICON_PX],
                rgba(Variant::Dark),
            );
            ctx.load_texture("icon", img, egui::TextureOptions::LINEAR)
        })
        .clone()
}

/// Title-bar icon, drawn by the same code as the tray icon.
///
/// `IconData` wants unmultiplied RGBA, which is exactly what the buffer holds.
pub fn window_icon() -> egui::IconData {
    let size = 64u32;
    let variant = Variant::Dark;
    let px = draw(size, variant).px;
    egui::IconData {
        rgba: px,
        width: size,
        height: size,
    }
}

/// Windows icon handle for the tray, following the theme.
///
/// `None` only when the pixel data is rejected, which would indicate a bug in
/// the drawing code rather than anything environmental.
pub fn tray_icon(dark: bool) -> Option<tray_icon::Icon> {
    let variant = if dark { Variant::Dark } else { Variant::Light };
    tray_icon::Icon::from_rgba(rgba(variant).to_vec(), ICON_PX as u32, ICON_PX as u32).ok()
}

// -------------------------------------------------------------- self-test --
//
// The drawing code here and `assets/New-Icon.ps1` are maintained by hand in
// parallel, and nothing at build time forces them to agree. The tests below
// exist so that a change to one side which is not mirrored on the other fails
// loudly instead of quietly producing a wrong icon.

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    /// Centre pixel `(x, y)` as `[r, g, b, a]`.
    fn at(buf: &Rgba, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * buf.w + x) * 4) as usize;
        [buf.px[i], buf.px[i + 1], buf.px[i + 2], buf.px[i + 3]]
    }

    /// Is this pixel the glyph colour for `variant`, rather than the plate's?
    fn is_glyph(px: [u8; 4], variant: Variant) -> bool {
        let g = variant.glyph();
        px[0].abs_diff(g[0]) < 24 && px[1].abs_diff(g[1]) < 24 && px[2].abs_diff(g[2]) < 24
    }

    /// True where the ring's radius shows the plate, i.e. the ring is broken.
    fn gap_at(deg: u32) -> bool {
        let buf = draw(256, Variant::Dark);
        let s = 256.0f32;
        let pad = s * 0.078;
        let w = s - pad * 2.0;
        let cx = pad + w / 2.0;
        let cy = pad + w / 2.0;
        let r = (deg as f32).to_radians();
        let x = (cx + r.cos() * (w * RING)).round() as u32;
        let y = (cy + r.sin() * (w * RING)).round() as u32;
        let px = at(&buf, x, y);
        !is_glyph(px, Variant::Dark) && px[3] > 200
    }

    #[test]
    fn dark_plate_is_the_agreed_green() {
        let buf = draw(64, Variant::Dark);
        // Four points on the plate but clear of the glyph. Not the centre, and
        // not straight above or below it: the stem runs up the middle with a
        // round cap that reaches past the ring, so every pixel on that line is
        // glyph-dark. These four sit outside the ring's outer edge instead.
        for (x, y) in [(32u32, 52u32), (16, 32), (48, 32), (20, 48)] {
            assert_eq!(
                at(&buf, x, y),
                [GREEN[0], GREEN[1], GREEN[2], 255],
                "plate is the wrong colour at ({x}, {y})"
            );
        }
    }

    #[test]
    fn light_variant_is_inverted_and_outlined() {
        let buf = draw(64, Variant::Light);
        // The plate inverts: white where the dark variant is green.
        for (x, y) in [(32u32, 52u32), (16, 32), (48, 32), (20, 48)] {
            assert_eq!(
                at(&buf, x, y),
                [255, 255, 255, 255],
                "light plate is not white at ({x}, {y})"
            );
        }

        // And the glyph inverts with it: deep green where the dark variant is dark.
        let cy = 64.0f32 * (0.078 + 0.844 / 2.0);
        let cx = 64.0f32 * (0.078 + 0.844 / 2.0);
        let centre = at(&buf, cx.round() as u32, cy.round() as u32);
        assert!(
            is_glyph(centre, Variant::Light),
            "the stem should be green in the light variant, got {centre:?}"
        );

        // The outline is the whole point of this variant: without it a white plate
        // vanishes against a white taskbar. Rather than try to count green
        // around the perimeter - which needs pixel-centre arithmetic to land on
        // the right ring and is brittle either way - walk in from each edge to
        // the first opaque pixel. That pixel is the outline, by construction,
        // and if the outline ever goes away it is simply white instead.
        let size = 64u32;
        let mid = (size / 2) as usize;
        for name in ["up", "down", "left", "right"] {
            let mut found = None;
            for k in 0..size as usize {
                let (x, y) = if name == "left" || name == "right" {
                    (k, mid)
                } else {
                    (mid, k)
                };
                let px = at(&buf, x as u32, y as u32);
                if px[3] > 128 {
                    found = Some(px);
                    break;
                }
            }
            let px = found.unwrap_or_else(|| panic!("nothing opaque scanning {name}"));
            assert!(
                is_glyph(px, Variant::Light),
                "the first opaque pixel from the {name} edge is {px:?}, not the \
                 green outline; the light variant would be invisible on white"
            );
        }
    }

    #[test]
    fn ring_gap_sits_at_the_top_where_the_stem_is() {
        // The gap must be around 270 degrees - straight up. The old drawing put
        // it at 180 (the left) while drawing a vertical stem at the top, which
        // is the bug these assertions guard against.
        //
        // The gap cannot be sampled at exactly 270: the stem occupies that
        // point, so it reads as glyph. It is sampled either side of the stem
        // instead. The stem is about asin(sw/2 / ring) ~= 13 degrees wide
        // there, so 20 to 30 degrees off centre is clear of it and still
        // comfortably inside the 90 degree gap, which runs 225 to 315.
        for deg in [240u32, 247, 254, 286, 293, 300] {
            assert!(gap_at(deg), "ring should be open near {deg} degrees (straight up)");
        }
        // ...and the ring must be closed everywhere else.
        for deg in [0u32, 45, 90, 135, 180, 225, 315, 350] {
            assert!(!gap_at(deg), "ring should be solid at {deg} degrees");
        }
    }

    #[test]
    fn stem_runs_vertically_up_the_centre() {
        let buf = draw(256, Variant::Dark);
        let s = 256.0f32;
        let pad = s * 0.078;
        let w = s - pad * 2.0;
        let cx = (pad + w / 2.0).round() as u32;
        let cy = pad + w / 2.0;
        let rr = w * RING;
        let sw = w * STROKE;

        // A short stretch of the centre line above the middle must be glyph.
        let from = cy - rr - sw * 0.62;
        for step in 0..8 {
            let y = (from + step as f32 * 0.5).round() as u32;
            assert!(
                is_glyph(at(&buf, cx, y), Variant::Dark),
                "no stem at y={y}; the symbol is malformed"
            );
        }

        // And the same must hold for the light variant, where the stem is
        // deep green rather than dark.
        let light = draw(256, Variant::Light);
        for step in 0..8 {
            let y = (from + step as f32 * 0.5).round() as u32;
            assert!(
                is_glyph(at(&light, cx, y), Variant::Light),
                "no stem in the light variant at y={y}"
            );
        }
    }

    fn ico_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("app.ico")
    }

    /// Extracts one uncompressed DIB frame from a multi-size `.ico`.
    ///
    /// Returns straight RGBA, top row first. The 256 px frame is stored as
    /// PNG inside the container, so only the DIB frames are handled here - the
    /// 32 px one is plenty to compare against.
    fn ico_frame(bytes: &[u8], want: u32) -> Option<Vec<u8>> {
        let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        for i in 0..count {
            let o = 6 + 16 * i;
            let w = if bytes[o] == 0 { 256 } else { bytes[o] as u32 };
            let len = u32::from_le_bytes(bytes[o + 8..o + 12].try_into().ok()?) as usize;
            let off = u32::from_le_bytes(bytes[o + 12..o + 16].try_into().ok()?) as usize;
            if w != want {
                continue;
            }
            let body = &bytes[off..off + len];
            // A PNG frame starts with the 8-byte signature; skip those.
            if body.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
                return None;
            }
            // BITMAPINFOHEADER is 40 bytes; XOR pixels follow, bottom-up, BGRA.
            let mut out = vec![0u8; (want * want * 4) as usize];
            for y in 0..want {
                let src = (want - 1 - y) as usize * (want as usize * 4);
                for x in 0..want as usize {
                    let s = src + x * 4;
                    let d = (y as usize * want as usize + x) * 4;
                    out[d] = body[40 + s + 2];
                    out[d + 1] = body[40 + s + 1];
                    out[d + 2] = body[40 + s];
                    out[d + 3] = body[40 + s + 3];
                }
            }
            return Some(out);
        }
        None
    }

    /// Averages `px` down to an 8x8 grid of mean, alpha-premultiplied RGBA blocks.
    ///
    /// Comparing GDI+ against this rasteriser pixel by pixel is hopeless -
    /// the two antialias differently and disagree along every edge, which is
    /// most of the image at 32 px. Averaging into blocks suppresses that and
    /// leaves the question actually worth asking: is it the same design?
    ///
    /// Colour is premultiplied by alpha first. The `.ico` stores fully
    /// transparent pixels with RGB 0, while this buffer leaves the plate colour
    /// there, so a straight channel compare reads the squircle's outline as a
    /// large difference even when the two drawings agree.
    fn blocks(px: &[u8], size: u32) -> Vec<[f32; 4]> {
        const N: u32 = 8;
        let cell = size / N;
        let mut out = vec![[0.0f32; 4]; (N * N) as usize];
        let mut hits = vec![0u32; (N * N) as usize];
        for y in 0..size {
            for x in 0..size {
                let b = ((y / cell) * N + (x / cell)) as usize;
                let i = ((y * size + x) * 4) as usize;
                let a = px[i + 3] as f32 / 255.0;
                out[b][0] += px[i] as f32 * a;
                out[b][1] += px[i + 1] as f32 * a;
                out[b][2] += px[i + 2] as f32 * a;
                out[b][3] += px[i + 3] as f32;
                hits[b] += 1;
            }
        }
        for (b, h) in hits.iter().enumerate() {
            for c in 0..4 {
                out[b][c] /= *h as f32;
            }
        }
        out
    }

    /// Frame size used for the cross-check.
    ///
    /// 64 px, not 16 or 32: at 16 px an 8x8 block is two pixels across, so the
    /// comparison is mostly noise and would be flaky in CI. 64 px is the
    /// smallest frame where the two rasterisers' antialiasing settles down
    /// enough for the tolerance below to mean something.
    const COMPARE_AT: u32 = 64;

    #[test]
    fn matches_the_committed_ico() {
        let path = ico_path();
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));

        let embedded = ico_frame(&bytes, COMPARE_AT).unwrap_or_else(|| {
            panic!(
                "app.ico has no uncompressed {COMPARE_AT} px frame - re-run assets/New-Icon.ps1"
            )
        });
        // assets/app.ico is the dark variant; the light one has no .ico on disk,
        // because Explorer and the Start menu always draw it on a light
        // background where a white plate would be invisible.
        let ours = draw(COMPARE_AT, Variant::Dark).px;

        assert_eq!(embedded.len(), ours.len(), "frame size mismatch");

        for (i, (x, y)) in blocks(&embedded, COMPARE_AT)
            .iter()
            .zip(blocks(&ours, COMPARE_AT).iter())
            .enumerate()
        {
            for c in 0..4 {
                let d = (x[c] - y[c]).abs();
                assert!(
                    d <= 26.0,
                    "src/icon.rs has drifted from assets/app.ico: block {} (col {}, row {}) \
                     channel {c} differs by {d:.1} (ico {:.1}, rust {:.1}). Mirror the change \
                     in assets/New-Icon.ps1 (or the other way round) and re-run it.",
                    i,
                    (i as u32 % 8) * (COMPARE_AT / 8),
                    (i as u32 / 8) * (COMPARE_AT / 8),
                    x[c],
                    y[c]
                );
            }
        }
    }
}