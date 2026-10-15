//! The colour system.
//!
//! One brand colour: a deep violet, the same one the icon is drawn in. Dark
//! mode is violet-tinted near-black; light mode is clean white and grey. The
//! two modes share a hue but nothing else - one is a room with the lights off,
//! the other is paper.
//!
//! Mode is still readable at a glance, but through the *ramp* rather than
//! through two unrelated colours: Quiet is the lighter violet, Performance the
//! deeper one. That keeps the old "blue means calm, amber means hot" signal
//! while making the app read as one thing.

use egui::{Color32, Stroke, Visuals};

use crate::profile::Mode;

// ------------------------------------------------------------- palette ---

/// The brand violet, and the plate colour in `src/icon.rs`.
pub const VIOLET: Color32 = Color32::from_rgb(109, 40, 217);

const VIOLET_400: Color32 = Color32::from_rgb(167, 139, 250);
const VIOLET_500: Color32 = Color32::from_rgb(139, 92, 246);
const VIOLET_700: Color32 = Color32::from_rgb(109, 40, 217);
const VIOLET_900: Color32 = Color32::from_rgb(76, 29, 149);

/// Dark-mode surfaces, darkest first.
const D_WINDOW: Color32 = Color32::from_rgb(16, 13, 25);
const D_PANEL: Color32 = Color32::from_rgb(24, 20, 36);
const D_CARD: Color32 = Color32::from_rgb(32, 26, 48);
const D_STROKE: Color32 = Color32::from_rgb(56, 47, 79);
const D_TEXT: Color32 = Color32::from_rgb(236, 233, 245);
const D_WEAK_TEXT: Color32 = Color32::from_rgb(150, 142, 175);

/// Light-mode surfaces.
const L_WINDOW: Color32 = Color32::from_rgb(255, 255, 255);
const L_PANEL: Color32 = Color32::from_rgb(255, 255, 255);
const L_CARD: Color32 = Color32::from_rgb(244, 244, 246);
const L_STROKE: Color32 = Color32::from_rgb(226, 226, 231);
const L_TEXT: Color32 = Color32::from_rgb(27, 27, 31);
const L_WEAK_TEXT: Color32 = Color32::from_rgb(107, 107, 118);

// ---------------------------------------------------------------- roles ---

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
        (Mode::Quiet, true) => VIOLET_400,
        (Mode::Quiet, false) => VIOLET_700,
        (Mode::Performance, true) => VIOLET_500,
        (Mode::Performance, false) => VIOLET_900,
    }
}

/// A faint wash of the brand colour, for the informational panels (Safety,
/// Backups) that used to be green.
///
/// Those panels are decoration, not status: green there read as "all clear" and
/// fought the theme. Status colour now only appears on actual status - the
/// notes and the pills.
pub fn surface(dark: bool) -> Color32 {
    VIOLET.gamma_multiply(if dark { 0.16 } else { 0.07 })
}

/// The border colour that goes with [`surface`].
pub fn surface_stroke(dark: bool) -> Color32 {
    VIOLET.gamma_multiply(if dark { 0.42 } else { 0.30 })
}

/// Semantic status colours. Deliberately not violet: these mean "good",
/// "careful" and "wrong", and borrowing the brand hue for them would make them
/// unreadable.
pub fn ok_color() -> Color32 {
    Color32::from_rgb(34, 160, 90)
}

pub fn warn_color() -> Color32 {
    Color32::from_rgb(190, 140, 20)
}

pub fn err_color() -> Color32 {
    Color32::from_rgb(226, 80, 80)
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
    let w = &mut v.widgets;

    // Unfilled things: just a background and a hairline.
    w.noninteractive.bg_fill = panel_bg(dark);
    w.noninteractive.weak_bg_fill = panel_bg(dark);
    w.noninteractive.bg_stroke = line;
    w.noninteractive.fg_stroke = Stroke::new(1.0, text(dark));

    // Filled things: a quiet card, warming to violet under the cursor.
    w.inactive.bg_fill = card_bg(dark);
    w.inactive.weak_bg_fill = card_bg(dark);
    w.inactive.bg_stroke = line;
    w.inactive.fg_stroke = Stroke::new(1.0, text(dark));

    w.hovered.bg_fill = surface(dark);
    w.hovered.weak_bg_fill = surface(dark);
    w.hovered.bg_stroke = Stroke::new(1.0, surface_stroke(dark));
    w.hovered.fg_stroke = Stroke::new(1.0, text(dark));

    w.active.bg_fill = accent(Mode::Quiet, dark);
    w.active.weak_bg_fill = accent(Mode::Quiet, dark);
    w.active.bg_stroke = Stroke::new(1.0, accent(Mode::Quiet, dark));
    w.active.fg_stroke = Stroke::new(1.0, if dark { D_WINDOW } else { Color32::WHITE });

    w.open.bg_fill = card_bg(dark);
    w.open.weak_bg_fill = card_bg(dark);
    w.open.bg_stroke = Stroke::new(1.0, accent(Mode::Quiet, dark));
    w.open.fg_stroke = Stroke::new(1.0, text(dark));

    v.selection.bg_fill = accent(Mode::Quiet, dark).gamma_multiply(if dark { 0.45 } else { 0.30 });
    v.selection.stroke = Stroke::new(1.0, accent(Mode::Quiet, dark));

    v.hyperlink_color = accent(Mode::Quiet, dark);
    v.warn_fg_color = warn_color();
    v.error_fg_color = err_color();
    // text_cursor is left at whatever Visuals::dark()/light() chose: it is a
    // struct with per-theme stroke colours, not an enum, and the defaults are
    // already right for each mode.

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
    fn light_mode_text_is_dark_enough_to_read() {
        // The light theme is meant to be "clean white and grey"; a violet wash
        // that dark would make small text unreadable.
        let l = L_TEXT;
        let bg = L_PANEL;
        let lum = |c: Color32| 0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32;
        assert!(
            lum(bg) - lum(l) > 150.0,
            "light theme text is too close to its background"
        );
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