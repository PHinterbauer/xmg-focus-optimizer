//! The colour system.
//!
//! One brand colour: XMG green. Dark
//! mode is near-black; light mode is clean white and grey. The
//! two modes share a hue but nothing else - one is a room with the lights off,
//! the other is paper.
//!
//! Mode is still readable at a glance, but through the *ramp* rather than
//! through two unrelated colours: Quiet is the lighter end, Performance the
//! deeper one. That keeps the old "calm versus hot" signal
//! while making the app read as one thing.

use egui::{Color32, Stroke, Visuals};

use crate::log::{log_debug, log_info, log_warn};
use crate::profile::Mode;

// ------------------------------------------------------------- palette ---

/// The brand accent: XMG green.
///
/// Derived shades below follow the same structure as before: a bright end for
/// dark mode, a deep end for light mode, so Quiet and Performance stay
/// distinguishable without the app turning two colours.
///
/// NOTE: this deliberately differs from the icon plate. The icon keeps a deep
/// green plate with a dark glyph; the UI accent is the vivid green. Both come
/// from the same family so they read as one brand.
pub const ACCENT: Color32 = Color32::from_rgb(0x00, 0xE6, 0x76);

/// Bright accent, for text and borders that have to sit on near-black.
pub const ACCENT_LIGHT: Color32 = Color32::from_rgb(0x69, 0xF0, 0xAE);

const ACCENT_400: Color32 = Color32::from_rgb(0x69, 0xF0, 0xAE);
const ACCENT_500: Color32 = Color32::from_rgb(0x00, 0xE6, 0x76);
const ACCENT_700: Color32 = Color32::from_rgb(0x00, 0x83, 0x43);
const ACCENT_900: Color32 = Color32::from_rgb(0x00, 0x5C, 0x2F);

/// Dark mode is the primary look: true black, white text, accent used only as a
/// highlight. The accent is deliberately sparse so it reads as a highlight
/// rather than as a colour scheme.
const D_WINDOW: Color32 = Color32::from_rgb(0, 0, 0);
const D_PANEL: Color32 = Color32::from_rgb(10, 10, 12);
const D_CARD: Color32 = Color32::from_rgb(19, 19, 23);
const D_CARD_HI: Color32 = Color32::from_rgb(30, 30, 36);
const D_STROKE: Color32 = Color32::from_rgb(46, 46, 54);
const D_TEXT: Color32 = Color32::from_rgb(255, 255, 255);
const D_WEAK_TEXT: Color32 = Color32::from_rgb(148, 148, 158);

/// Light mode: clean white and grey, same accent.
const L_WINDOW: Color32 = Color32::from_rgb(255, 255, 255);
const L_PANEL: Color32 = Color32::from_rgb(250, 250, 251);
const L_CARD: Color32 = Color32::from_rgb(244, 244, 246);
const L_STROKE: Color32 = Color32::from_rgb(219, 219, 226);
const L_TEXT: Color32 = Color32::from_rgb(17, 17, 20);
const L_WEAK_TEXT: Color32 = Color32::from_rgb(105, 105, 118);

// ------------------------------------------------------------------ fonts --

/// Points egui at a Windows UI font.
///
/// egui ships Ubuntu-Light as its default. It is a perfectly good font, but it
/// is a Linux font, and on Windows it is the single biggest reason an egui
/// window looks like a prototype: the letterforms do not match anything else
/// on the screen. Loading Segoe UI costs one file read and makes the whole app
/// look like it belongs on this machine.
pub fn apply_fonts(ctx: &egui::Context) {
    const CANDIDATES: &[&str] = &[
        "C:\\Windows\\Fonts\\segoeui.ttf",
        "C:\\Windows\\Fonts\\segoeuib.ttf", // bold
        "C:\\Windows\\Fonts\\seguisb.ttf",  // semibold
    ];

    let mut fonts = egui::FontDefinitions::default();
    let mut loaded = false;

    for (i, path) in CANDIDATES.iter().enumerate() {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let name = match i {
            0 => "segoe",
            1 => "segoe-bold",
            _ => "segoe-semibold",
        };
        fonts
            .font_data
            // FontDefinitions::font_data holds Arc<FontData>, not FontData.
            .insert(name.to_owned(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
        loaded = true;
    }

    if !loaded {
        log_warn!("no system UI font found; falling back to egui's built-in default");
        return;
    }

    for family in [
        egui::FontFamily::Proportional,
        egui::FontFamily::Monospace,
    ] {
        let list = fonts.families.entry(family).or_default();
        list.insert(0, "segoe".to_owned());
        list.insert(1, "segoe-semibold".to_owned());
        list.insert(2, "segoe-bold".to_owned());
    }

    ctx.set_fonts(fonts);
    log_info!("using the system UI font for all text");
}

/// A slightly tighter, more deliberate type scale than the egui defaults,
/// which are tuned for dense debug overlays rather than a settings window.
///
/// Applied to both themes at once. egui 0.36 removed `Context::style()`, so the
/// only way to reach every style is `all_styles_mut`.
pub fn apply_spacing(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(10.0, 9.0);
        s.button_padding = egui::vec2(14.0, 8.0);
        s.window_margin = egui::Margin::same(18);
        s.interact_size = egui::vec2(44.0, 34.0);
        s.menu_margin = egui::Margin::same(10);
        s.slider_width = 180.0;
        s.scroll.bar_width = 9.0;
        s.scroll.floating_allocated_width = 11.0;
    });
    log_debug!("spacing applied to both themes");
}

// ---------------------------------------------------------------- roles ---

/// Relative luminance, used by the contrast assertions in the tests.
#[cfg(test)]
fn lum(c: Color32) -> f32 {
    0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32
}

pub fn window_bg(dark: bool) -> Color32 {
    if dark { D_WINDOW } else { L_WINDOW }
}

pub fn panel_bg(dark: bool) -> Color32 {
    if dark { D_PANEL } else { L_PANEL }
}

pub fn card_bg(dark: bool) -> Color32 {
    if dark { D_CARD } else { L_CARD }
}

pub fn stroke(dark: bool) -> Color32 {
    if dark { D_STROKE } else { L_STROKE }
}

pub fn text(dark: bool) -> Color32 {
    if dark { D_TEXT } else { L_TEXT }
}

pub fn weak_text(dark: bool) -> Color32 {
    if dark { D_WEAK_TEXT } else { L_WEAK_TEXT }
}

/// The accent for the selected mode.
///
/// Quiet sits at the light end of the ramp and Performance at the dark end, so
/// the two stay distinguishable without the app turning two colours.
pub fn accent(mode: Mode, dark: bool) -> Color32 {
    match (mode, dark) {
        (Mode::Quiet, true) => ACCENT_400,
        (Mode::Quiet, false) => ACCENT_700,
        (Mode::Performance, true) => ACCENT_500,
        (Mode::Performance, false) => ACCENT_900,
        // Default is neutral gray - not a performance mode
        (Mode::Default, true) => egui::Color32::from_gray(120),
        (Mode::Default, false) => egui::Color32::from_gray(140),
    }
}

/// Text colour that reads on top of a filled accent.
///
/// White on a light fill is the classic failure (bright button, invisible
/// label), so the choice is computed from luminance rather than picked by
/// hand. Dark fills get white, light fills get near-black.
pub fn contrast_text(bg: Color32) -> Color32 {
    let lum = 0.2126 * bg.r() as f32 + 0.7152 * bg.g() as f32 + 0.0722 * bg.b() as f32;
    if lum > 140.0 {
        Color32::from_rgb(17, 17, 20)
    } else {
        Color32::WHITE
    }
}

/// A faint wash of the brand colour, for the informational panels (Safety,
/// Backups) that used to be green.
///
/// Those panels are decoration, not status: green there read as "all clear" and
/// fought the theme. Status colour now only appears on actual status - the
/// notes and the pills.
pub fn surface(dark: bool) -> Color32 {
    ACCENT.gamma_multiply(if dark { 0.22 } else { 0.08 })
}

/// The border colour that goes with [`surface`].
pub fn surface_stroke(dark: bool) -> Color32 {
    ACCENT.gamma_multiply(if dark { 0.55 } else { 0.32 })
}

/// Semantic status colours. Deliberately not the accent: these mean "good",
/// "careful" and "wrong", and borrowing the brand hue for them would make them
/// unreadable.
pub fn ok_color() -> Color32 {
    Color32::from_rgb(52, 199, 123)
}

pub fn warn_color() -> Color32 {
    Color32::from_rgb(240, 180, 41)
}

pub fn err_color() -> Color32 {
    Color32::from_rgb(248, 105, 105)
}

// -------------------------------------------------------------- visuals ---

/// Builds the whole egui look for one mode.
///
/// Field names are egui 0.36's; they moved around a lot before that release
/// (`weak_bg_color` and friends), so this is worth re-checking when egui is
/// bumped rather than trusting it to compile.
pub fn visuals(dark: bool) -> Visuals {
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };

    v.dark_mode = dark;
    v.override_text_color = Some(text(dark));
    v.weak_text_color = Some(weak_text(dark));
    v.weak_text_alpha = 1.0;

    v.panel_fill = panel_bg(dark);
    v.window_fill = window_bg(dark);
    v.extreme_bg_color = window_bg(dark);
    v.faint_bg_color = card_bg(dark);
    v.code_bg_color = card_bg(dark);
    v.text_edit_bg_color = Some(card_bg(dark));

    let line = Stroke::new(1.0, stroke(dark));
    // On black a white label needs weight to survive being white-on-near-black.
    let fg = Stroke::new(1.0, text(dark));
    let accent_col = accent(Mode::Quiet, dark);
    let w = &mut v.widgets;

    // Buttons. The earlier version painted inactive and noninteractive almost
    // the same grey, so a button looked like the panel it sat on and read as
    // "not clickable". A button now has its own surface and a visible edge.
    w.noninteractive.bg_fill = panel_bg(dark);
    w.noninteractive.weak_bg_fill = panel_bg(dark);
    w.noninteractive.bg_stroke = line;
    w.noninteractive.fg_stroke = fg;

    w.inactive.bg_fill = if dark { D_CARD_HI } else { Color32::from_rgb(255, 255, 255) };
    w.inactive.weak_bg_fill = w.inactive.bg_fill;
    w.inactive.bg_stroke = Stroke::new(1.0, stroke(dark));
    w.inactive.fg_stroke = fg;
    w.inactive.corner_radius = egui::CornerRadius::same(6);

    // Hover lifts the surface and tints the border with the accent, so the pointer
    // position is unmistakable.
    w.hovered.bg_fill = if dark { D_CARD_HI.gamma_multiply(1.35) } else { Color32::from_rgb(232, 250, 240) };
    w.hovered.weak_bg_fill = w.hovered.bg_fill;
    w.hovered.bg_stroke = Stroke::new(1.0, ACCENT_LIGHT);
    w.hovered.fg_stroke = fg;
    w.hovered.corner_radius = egui::CornerRadius::same(6);

    // Pressed is solid accent. The label colour is computed from luminance, so
    // it stays readable whatever the accent is.
    w.active.bg_fill = ACCENT;
    w.active.weak_bg_fill = ACCENT;
    w.active.bg_stroke = Stroke::new(1.0, ACCENT_LIGHT);
    w.active.fg_stroke = Stroke::new(1.0, contrast_text(ACCENT));
    w.active.corner_radius = egui::CornerRadius::same(6);

    w.open.bg_fill = ACCENT.gamma_multiply(if dark { 0.45 } else { 0.18 });
    w.open.weak_bg_fill = w.open.bg_fill;
    w.open.bg_stroke = Stroke::new(1.0, accent_col);
    w.open.fg_stroke = fg;
    w.open.corner_radius = egui::CornerRadius::same(6);

    v.selection.bg_fill = ACCENT.gamma_multiply(if dark { 0.5 } else { 0.28 });
    v.selection.stroke = Stroke::new(1.0, accent_col);

    v.hyperlink_color = if dark { ACCENT_LIGHT } else { ACCENT_700 };
    v.warn_fg_color = warn_color();
    v.error_fg_color = err_color();
    // text_cursor is left at whatever Visuals::dark()/light() chose: it is a
    // struct with per-theme stroke colours, not an enum, and the defaults are
    // already right for each mode.

    // Squarer corners overall, and a little more breathing room than the
    // egui defaults, which are tuned for dense debug UIs.
    v.window_corner_radius = egui::CornerRadius::same(6);
    v.menu_corner_radius = egui::CornerRadius::same(6);

    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_modes_are_built_and_differ() {
        let d = visuals(true);
        let l = visuals(false);
        assert!(d.dark_mode && !l.dark_mode);
        assert_ne!(d.panel_fill, l.panel_fill);
        assert_eq!(d.override_text_color, Some(text(true)));
        assert_eq!(l.override_text_color, Some(text(false)));
    }

    #[test]
    fn dark_mode_text_is_white_and_surfaces_are_black() {
        // The dark look is black with white text and accent highlights. These
        // three assertions are the whole brief; if someone re-tints the panel
        // by hand later this is what catches it.
        assert_eq!(D_WINDOW, Color32::from_rgb(0, 0, 0), "window must be pure black");
        assert_eq!(D_TEXT, Color32::WHITE, "dark-mode text must be white");
        assert_eq!(text(true), Color32::WHITE);
        // Panels sit just off black so a card is still distinguishable.
        assert!(lum(D_CARD) > lum(D_WINDOW), "cards must be lighter than the window");
        assert!(lum(D_CARD) < lum(D_TEXT), "cards must stay well below the text");
    }

    #[test]
    fn buttons_are_visibly_distinct_from_the_panel() {
        // A button painted the same colour as the panel it sits on reads as
        // broken. That is what the first version did.
        let v = visuals(true);
        assert_ne!(
            v.widgets.inactive.bg_fill, v.panel_fill,
            "a resting button is invisible against the panel"
        );
        assert_ne!(
            v.widgets.inactive.bg_fill, v.widgets.hovered.bg_fill,
            "hover gives no feedback"
        );
        assert_ne!(
            v.widgets.inactive.bg_fill, v.widgets.active.bg_fill,
            "press gives no feedback"
        );
    }

    #[test]
    fn accent_is_used_sparingly_not_as_a_background() {
        // The accent has to stand out against black without being the colour
        // of everything.
        let v = visuals(true);
        assert_eq!(v.hyperlink_color, ACCENT_LIGHT);
        assert_ne!(v.panel_fill, ACCENT);
    }

    #[test]
    fn light_mode_text_is_dark_enough_to_read() {
        // The light theme is meant to be "clean white and grey"; an accent wash
        // that dark would make small text unreadable.
        let l = L_TEXT;
        let bg = L_PANEL;
        assert!(
            lum(bg) - lum(l) > 150.0,
            "light theme text is too close to its background"
        );
    }

    #[test]
    fn button_labels_stay_readable_on_accent_fills() {
        // White on a light fill is the classic failure (bright button,
        // invisible label). The choice is computed, so lock both sides.
        assert_eq!(contrast_text(ACCENT_400), Color32::from_rgb(17, 17, 20));
        assert_eq!(contrast_text(ACCENT_500), Color32::from_rgb(17, 17, 20));
        assert_eq!(contrast_text(ACCENT_700), Color32::WHITE);
        assert_eq!(contrast_text(ACCENT_900), Color32::WHITE);
        assert_eq!(contrast_text(ACCENT), Color32::from_rgb(17, 17, 20));
    }

    #[test]
    fn accents_stay_distinguishable_within_a_mode() {
        for dark in [true, false] {
            let q = accent(Mode::Quiet, dark);
            let p = accent(Mode::Performance, dark);
            let d = (q.r() as i32 - p.r() as i32)
                .abs()
                .max((q.g() as i32 - p.g() as i32).abs())
                .max((q.b() as i32 - p.b() as i32).abs());
            assert!(d >= 20, "Quiet and Performance look the same (delta {d})");
        }
    }
}