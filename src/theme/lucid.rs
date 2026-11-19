//! Lucid, the Spicetify theme by Sachu, as a Spotifast palette.
//!
//! Lucid draws its colours from a Material 3 tonal palette: one seed colour
//! is spread across a hue-held ladder of tones, and every surface, outline
//! and text colour comes from that ladder. This module builds the same ladder
//! in OKLCH, once per palette, so the theme has Lucid's green-cast near-black
//! surfaces in dark mode, its paper-toned light surfaces, and the green
//! accent both use. Its clean background, which shows no artwork at all, uses
//! the same ladder without the seed's chroma: at those near-black tones even
//! a trace of it reads as a green cast.
//!
//! Everything here runs when a palette is chosen, never per frame: the
//! derived colours are plain [`Color32`] values afterwards.

use egui::Color32;

use super::Palette;

/// Lucid's default seed: Spotify's green.
pub const SEED: Color32 = Color32::from_rgb(0x1e, 0xd7, 0x60);

/// The hue of Lucid's warning tone, taken from its own `#ffa42b`.
const WARNING_HUE: f32 = 1.22;
/// The hue of Material's error tone, near `#ff5a62`.
const ERROR_HUE: f32 = 0.47;

/// Lucid's palette for the desktop's dark or light appearance.
pub fn palette(dark: bool) -> Palette {
    palette_from_tint(dark, SEED)
}

/// Lucid's clean background, `Background: Dark`: the same floating cards and
/// green accent over a neutral ladder. Lucid's own near-blacks keep a trace
/// of its seed, and at that darkness the trace is what makes the interface
/// look green rather than black, so this ladder has no chroma at all: the
/// cards are greys, and the window behind them is black.
pub fn clean_palette() -> Palette {
    let seed = Tonal::from_seed(SEED);
    let n = |tone: f32| seed.tone_at(seed.hue, 0.0, tone);
    Palette {
        dark: true,
        // The window is black, and the cards step up from it just enough to
        // read as surfaces of their own; the outline is the hairline that
        // separates them.
        window: n(6.0),
        panel: n(17.0),
        surface: n(21.0),
        surface_hover: n(24.0),
        surface_active: n(27.0),
        // A hairline that stays visible on the cards without becoming a rule
        // of its own.
        outline: n(42.0),
        text: n(97.0),
        secondary: n(75.0),
        dim: n(54.0),
        accent: seed.primary(80.0),
        accent_hover: seed.primary(88.0),
        on_accent: seed.primary(16.0),
        danger: seed.tone_at(ERROR_HUE, 0.15, 80.0),
        warning: seed.tone_at(WARNING_HUE, 0.14, 82.0),
        overlay: n(22.0),
        shadow: Color32::from_black_alpha(200),
        tint: with_alpha(seed.primary(70.0), 30),
    }
}

/// Lucid's palette seeded from a cover's colour. Lucid derives every surface
/// from one seed, so a page, a search field, a column or a bar all take the
/// cover's cast rather than staying on the theme's green.
pub fn palette_from_tint(dark: bool, tint: Color32) -> Palette {
    if dark {
        dark_palette(tint)
    } else {
        light_palette(tint)
    }
}

fn dark_palette(tint: Color32) -> Palette {
    let seed = Tonal::from_seed(tint);
    // A cover's hue needs more chroma than Lucid's quiet green seed before it
    // reads as the cover in a dark surface; its tones stay the same, so text
    // contrast does not change.
    let tinted = tint != SEED;
    let n = |tone: f32| seed.tone_at(seed.hue, if tinted { 0.034 } else { 0.010 }, tone);
    let nv = |tone: f32| seed.tone_at(seed.hue, if tinted { 0.068 } else { 0.020 }, tone);
    Palette {
        dark: true,
        // The surface ladder: neutral tones for the window, neutral-variant
        // tones for everything raised above it, as Lucid's surface tokens do.
        window: n(6.0),
        panel: nv(20.0),
        surface: nv(25.0),
        surface_hover: nv(30.0),
        surface_active: nv(35.0),
        outline: nv(44.0),
        text: seed.neutral_variant(92.0),
        secondary: seed.neutral_variant(78.0),
        dim: seed.neutral_variant(58.0),
        accent: seed.primary(80.0),
        accent_hover: seed.primary(88.0),
        on_accent: seed.primary(16.0),
        danger: seed.tone_at(ERROR_HUE, 0.15, 80.0),
        warning: seed.tone_at(WARNING_HUE, 0.14, 82.0),
        // Menus and dialogs sit above the raised surfaces.
        overlay: nv(28.0),
        shadow: Color32::from_black_alpha(160),
        // Lucid's pages bloom with the accent behind their header.
        tint: with_alpha(seed.primary(70.0), 30),
    }
}

fn light_palette(tint: Color32) -> Palette {
    let seed = Tonal::from_seed(tint);
    let tinted = tint != SEED;
    let n = |tone: f32| seed.tone_at(seed.hue, if tinted { 0.028 } else { 0.010 }, tone);
    let nv = |tone: f32| seed.tone_at(seed.hue, if tinted { 0.055 } else { 0.020 }, tone);
    Palette {
        dark: false,
        window: n(98.0),
        panel: n(100.0),
        surface: nv(96.0),
        surface_hover: nv(94.0),
        surface_active: nv(90.0),
        outline: nv(86.0),
        text: seed.neutral_variant(12.0),
        secondary: seed.neutral_variant(32.0),
        dim: seed.neutral_variant(52.0),
        accent: seed.primary(38.0),
        accent_hover: seed.primary(30.0),
        on_accent: seed.primary(100.0),
        danger: seed.tone_at(ERROR_HUE, 0.15, 42.0),
        warning: seed.tone_at(WARNING_HUE, 0.14, 40.0),
        overlay: n(100.0),
        shadow: Color32::from_black_alpha(40),
        tint: with_alpha(seed.primary(50.0), 24),
    }
}

fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    let [r, g, b, _] = color.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

/// A hue-held ladder of tones, as Material's color utilities call it: each
/// role is the same hue at a different perceptual lightness.
struct Tonal {
    hue: f32,
}

impl Tonal {
    fn from_seed(seed: Color32) -> Self {
        let [r, g, b, _] = seed.to_srgba_unmultiplied();
        let (_, _, hue) = oklch([r, g, b]);
        Self { hue }
    }

    /// The tinted near-grey ladder: panels, cards and outlines.
    fn neutral_variant(&self, tone: f32) -> Color32 {
        self.tone_at(self.hue, 0.020, tone)
    }

    fn primary(&self, tone: f32) -> Color32 {
        self.tone_at(self.hue, 0.150, tone)
    }

    /// One tone at `hue`, with its chroma given up until the colour fits in
    /// sRGB, so no role is silently clipped to a different hue.
    fn tone_at(&self, hue: f32, chroma: f32, tone: f32) -> Color32 {
        let lightness = (tone.clamp(0.0, 100.0) / 100.0).clamp(0.0, 1.0);
        let mut chroma = chroma;
        for _ in 0..16 {
            if let Some(color) = from_oklch(lightness, chroma, hue) {
                return color;
            }
            chroma *= 0.82;
        }
        from_oklch(lightness, 0.0, hue).unwrap_or(Color32::BLACK)
    }
}

fn oklch([r, g, b]: [u8; 3]) -> (f32, f32, f32) {
    let [r, g, b] = [r, g, b].map(linear);
    let l = 0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_99 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    let lightness = 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s;
    let a = 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s;
    let b = 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s;
    (lightness, a.hypot(b), b.atan2(a))
}

fn from_oklch(lightness: f32, chroma: f32, hue: f32) -> Option<Color32> {
    let (sin, cos) = hue.sin_cos();
    let a = chroma * cos;
    let b = chroma * sin;
    let l = lightness + 0.396_337_78 * a + 0.215_803_76 * b;
    let m = lightness - 0.105_561_346 * a - 0.063_854_17 * b;
    let s = lightness - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    let channels = [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ];
    // Out of sRGB: the caller gives up chroma and tries again.
    if channels
        .iter()
        .any(|channel| !(-0.000_5..=1.000_5).contains(channel))
    {
        return None;
    }
    let [r, g, b] = channels.map(|channel| encode(channel.clamp(0.0, 1.0)));
    Some(Color32::from_rgb(r, g, b))
}

fn linear(channel: u8) -> f32 {
    let channel = f32::from(channel) / 255.0;
    if channel <= 0.040_45 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

fn encode(channel: f32) -> u8 {
    let channel = if channel <= 0.003_130_8 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    };
    (channel.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(color: Color32) -> f32 {
        let [r, g, b, _] = color.to_srgba_unmultiplied();
        let [r, g, b] = [r, g, b].map(linear);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    /// The tonal ladder stays in order: every raised surface is lighter than
    /// the one below it in dark mode, darker in light mode, and the text is
    /// at the far end of the ladder from the window.
    #[test]
    fn lucid_surfaces_climb_their_ladder() {
        let dark = dark_palette(SEED);
        assert!(dark.dark);
        let ladder = [
            dark.window,
            dark.panel,
            dark.surface,
            dark.surface_hover,
            dark.surface_active,
        ];
        for pair in ladder.windows(2) {
            assert!(
                luminance(pair[1]) > luminance(pair[0]),
                "{:?} is not above {:?}",
                pair[1],
                pair[0]
            );
        }
        assert!(luminance(dark.text) > luminance(dark.window) * 8.0);

        let light = light_palette(SEED);
        assert!(!light.dark);
        let ladder = [
            light.window,
            light.surface,
            light.surface_hover,
            light.surface_active,
        ];
        for pair in ladder.windows(2) {
            assert!(
                luminance(pair[1]) < luminance(pair[0]),
                "{:?} is not below {:?}",
                pair[1],
                pair[0]
            );
        }
        assert!(luminance(light.text) < luminance(light.window) * 0.1);
    }

    /// The accent is green like Lucid's, and reads against both the window
    /// and the words that sit on it.
    #[test]
    fn lucid_keeps_its_green_and_its_contrast() {
        for palette in [dark_palette(SEED), light_palette(SEED)] {
            assert!(
                palette.accent.g() > palette.accent.r() && palette.accent.g() > palette.accent.b(),
                "accent is green: {:?}",
                palette.accent
            );
            let contrast = |a: Color32, b: Color32| {
                let (a, b) = (luminance(a) + 0.05, luminance(b) + 0.05);
                if a > b { a / b } else { b / a }
            };
            assert!(
                contrast(palette.text, palette.window) >= 7.0,
                "text on window: {}",
                contrast(palette.text, palette.window)
            );
            assert!(
                contrast(palette.on_accent, palette.accent) >= 4.5,
                "words on the accent: {}",
                contrast(palette.on_accent, palette.accent)
            );
            assert_ne!(palette.tint.a(), 0, "the header bloom is visible");
            assert!(palette.tint.a() < 64, "a quiet bloom");
        }
    }

    /// The clean background carries no hue at all: every grey stays neutral,
    /// the ladder still climbs, the window is black, and Lucid's green accent
    /// is the one colour left.
    #[test]
    fn the_clean_background_is_neutral() {
        let palette = clean_palette();
        assert!(palette.dark);
        for (name, color) in [
            ("window", palette.window),
            ("panel", palette.panel),
            ("surface", palette.surface),
            ("surface_hover", palette.surface_hover),
            ("surface_active", palette.surface_active),
            ("outline", palette.outline),
            ("overlay", palette.overlay),
            ("text", palette.text),
            ("secondary", palette.secondary),
            ("dim", palette.dim),
        ] {
            let [r, g, b, _] = color.to_srgba_unmultiplied();
            assert!(
                r == g && g == b,
                "{name} keeps a cast from the seed: {color:?}"
            );
        }
        let ladder = [
            palette.window,
            palette.panel,
            palette.surface,
            palette.surface_hover,
            palette.surface_active,
        ];
        for pair in ladder.windows(2) {
            assert!(
                luminance(pair[1]) > luminance(pair[0]),
                "{:?} is not above {:?}",
                pair[1],
                pair[0]
            );
        }
        assert!(
            luminance(palette.window) < 0.001,
            "the window is black: {:?}",
            palette.window
        );
        assert!(
            palette.accent.g() > palette.accent.r() && palette.accent.g() > palette.accent.b(),
            "the accent stays Lucid's green: {:?}",
            palette.accent
        );
        let contrast = |a: Color32, b: Color32| {
            let (a, b) = (luminance(a) + 0.05, luminance(b) + 0.05);
            if a > b { a / b } else { b / a }
        };
        assert!(
            contrast(palette.text, palette.surface) >= 7.0,
            "text on a card: {}",
            contrast(palette.text, palette.surface)
        );
        assert!(
            contrast(palette.outline, palette.surface) >= 2.0,
            "the hairline is visible on a card: {}",
            contrast(palette.outline, palette.surface)
        );
    }

    /// Choosing Lucid twice gives the same colours, so a palette comparison
    /// never sees a change that is not one.
    #[test]
    fn lucid_palettes_are_deterministic() {
        for dark in [true, false] {
            assert_eq!(palette(dark), palette(dark));
            assert_eq!(palette(dark).dark, dark);
        }
    }
}
