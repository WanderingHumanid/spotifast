//! Lucid's floating chrome.
//!
//! The theme's signature is not only its colours: its library, page and player
//! are separate rounded cards over a blurred picture of the cover, with an 8px
//! gap between them. This module paints that backdrop and supplies the frame
//! and glass colours the panels use.

use egui::epaint::{Mesh, Shadow, Vertex};
use egui::{Color32, Frame, Margin, Painter, Rect, Shape, Stroke, Ui, pos2, vec2};

use crate::app::App;
use crate::settings::{LucidBackground, LucidGlass, ThemeChoice};

/// Lucid's `--panel-gap`.
pub const GAP: f32 = 8.0;
/// Its `--l-br`, the radius of the library and the page.
pub const RADIUS: u8 = 16;
/// Its `--m-br`, the radius of the player.
pub const PLAYER_RADIUS: u8 = 12;
/// The cover rail Lucid shows in place of the expanded library.
pub const RAIL_WIDTH: f32 = 104.0;

pub fn enabled(app: &App) -> bool {
    app.settings.theme == ThemeChoice::Lucid
}

/// A panel colour with the transparency Lucid gives its cards. The cover's
/// colour is mixed into the surface as well, so a card takes the hue of the
/// playlist or song it is showing rather than staying on the theme's seed.
pub fn glass(app: &App, color: Color32) -> Color32 {
    if app.settings.lucid_background == LucidBackground::Dark {
        // The clean background has no artwork to show through, so its panels
        // are plain surfaces.
        return color;
    }
    let alpha = glass_alpha(app.palette.dark, app.settings.lucid_glass);
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// How opaque a Lucid card is: `Off` is a plain surface, and the two glass
/// settings step further back until the backdrop is clearly visible. `Clear`
/// is close to Lucid's own `--main-bg-alpha` of 0.3.
fn glass_alpha(dark: bool, glass: LucidGlass) -> u8 {
    match glass {
        LucidGlass::Off => 255,
        LucidGlass::Soft if dark => 152,
        LucidGlass::Soft => 205,
        LucidGlass::Clear if dark => 92,
        LucidGlass::Clear => 178,
    }
}

/// The page's cover colour, or the playing song's, for the glass and the
/// backdrop. It is only used, and only asked for, while Lucid is showing.
fn cover_tint(app: &App) -> Option<Color32> {
    if !app.settings.accent_from_art
        || !app.settings.lucid_cover_tint
        || app.settings.lucid_background == LucidBackground::Dark
    {
        return None;
    }
    if let Some(url) = super::page_art(app, 64)
        && let Some(color) = app.accents.get(&url)
    {
        return Some(*color);
    }
    app.now_playing_tint()
}

/// The hairline border Lucid draws around every floating surface.
pub fn border(app: &App) -> Stroke {
    let outline = app.palette.outline;
    Stroke::new(
        1.0,
        Color32::from_rgba_unmultiplied(
            outline.r(),
            outline.g(),
            outline.b(),
            border_alpha(app.settings.lucid_background),
        ),
    )
}

/// How firm that hairline is. Over the cover the artwork already separates
/// the cards; on the clean background the line is the only edge they have, so
/// it is drawn at nearly full strength.
fn border_alpha(background: LucidBackground) -> u8 {
    match background {
        LucidBackground::Cover => 145,
        LucidBackground::Dark => 235,
    }
}

pub fn shadow(app: &App) -> Shadow {
    Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: if app.palette.dark {
            Color32::from_black_alpha(90)
        } else {
            Color32::from_black_alpha(40)
        },
    }
}

/// A rounded glass card, before its margins and radius are adapted by the
/// panel that wears it.
pub fn card(app: &App, radius: u8) -> Frame {
    let base = if app.palette.dark {
        app.palette.surface
    } else {
        app.palette.panel
    };
    Frame::new()
        .fill(glass(app, base))
        .stroke(border(app))
        .corner_radius(radius)
        .shadow(shadow(app))
}

/// The page's card: a gap on every side, so the backdrop shows between it and
/// the library and player.
pub fn page_frame(app: &App) -> Frame {
    card(app, RADIUS).outer_margin(Margin {
        left: GAP as i8,
        right: GAP as i8,
        top: GAP as i8,
        bottom: GAP as i8,
    })
}

/// The library's card, which keeps the window's left edge.
pub fn library_frame(app: &App) -> Frame {
    card(app, RADIUS)
        .fill(glass(app, app.palette.panel))
        .outer_margin(Margin {
            left: GAP as i8,
            right: 0,
            top: GAP as i8,
            bottom: GAP as i8,
        })
}

/// The player's card, floating over the bottom edge.
pub fn player_frame(app: &App) -> Frame {
    Frame::new()
        .fill(Color32::TRANSPARENT)
        .corner_radius(PLAYER_RADIUS)
        .outer_margin(Margin {
            left: GAP as i8,
            right: GAP as i8,
            top: 0,
            bottom: GAP as i8,
        })
        .inner_margin(Margin::symmetric(16, 0))
        .stroke(border(app))
        .shadow(shadow(app))
}

/// The playing card's own height, upright: the transport, the song, and the
/// progress bar it draws, with Lucid's tighter padding around them.
pub const PLAYER_CARD_HEIGHT: f32 = 72.0;

/// The room the player's card takes above and below its content: the gap
/// under it and the hairline that draws inside its edge. A panel asking for
/// an exact size counts both, so the frame's height is its content's plus
/// this, or the card clips what it holds.
pub fn card_chrome(app: &App) -> f32 {
    player_frame(app).total_margin().sum().y
}

/// The cover the backdrop and the bloom follow: the page's playlist, album,
/// artist or show cover, or the playing song's where the page has none.
fn backdrop_art(app: &App) -> Option<String> {
    super::page_art(app, 640).or_else(|| {
        app.now_playing()
            .and_then(|now| now.art_url.or(now.art_small))
    })
}

/// Paints the blurred cover behind the panels. Called once, before them.
pub fn backdrop(app: &mut App, ui: &Ui) {
    let rect = ui.max_rect();
    let painter = ui.painter().with_clip_rect(rect);
    if app.settings.lucid_background == LucidBackground::Dark {
        // The clean background: one flat dark field, no art and no wash.
        painter.rect_filled(rect, 0.0, app.palette.window);
        return;
    }
    let art = backdrop_art(app);
    match app
        .lyrics_backdrop
        .texture(ui.ctx(), app.backend.art(), art.as_deref())
    {
        Some(texture) => {
            // Lucid crops the cover with `background-size: cover` and runs it
            // through `blur(32px) saturate(1.5) brightness(0.4)`: the art is
            // always full-bleed, never stretched, and dark enough for text.
            let [width, height] = texture.size();
            let uv = cover_uv(vec2(width as f32, height as f32), rect.size());
            painter.image(texture.id(), rect, uv, Color32::from_gray(102));
        }
        None => {
            // No cover loaded yet: still lay its colour behind the glass, so
            // a card never sits on a flat grey field while the art arrives.
            painter.rect_filled(rect, 0.0, app.palette.window);
            let accent = cover_tint(app).unwrap_or(app.palette.accent);
            let alpha = if cover_tint(app).is_some() { 120 } else { 18 };
            painter.rect_filled(
                rect,
                0.0,
                Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), alpha),
            );
        }
    }
}

/// Lucid's `UnderMainView`: a second, brighter copy of the page cover at the
/// top of the page, fading out before the rows begin. It is what gives the
/// top of a playlist or album its glow over the darker backdrop.
pub fn under_main_view(app: &mut App, ui: &Ui, page: Rect) {
    // It lives under the page's glass: an opaque page has nothing to show it
    // through, and the clean dark background has no artwork at all.
    if app.settings.lucid_background != LucidBackground::Cover
        || app.settings.lucid_glass == LucidGlass::Off
    {
        return;
    }
    let height = (page.height() / 1.75).min(page.height());
    if height < 48.0 {
        return;
    }
    // Lucid's header image is the page's artwork. Home, Search and the rest
    // have none, and official Lucid leaves them without a bloom rather than
    // borrowing the playing song's cover. Never ask the shared backdrop for a
    // missing cover: that resets the one it is already showing, and every
    // frame would blur the cover again.
    let Some(art) = super::page_art(app, 640) else {
        return;
    };
    let luma = app.lyrics_backdrop.bloom_luma();
    let Some(texture) = app.lyrics_backdrop.bloom(ui.ctx(), app.backend.art(), &art) else {
        return;
    };
    let rect = Rect::from_min_size(page.min, vec2(page.width(), height));
    let [width, height] = texture.size();
    let uv = cover_uv(vec2(width as f32, height as f32), rect.size());
    let painter = ui.painter().with_clip_rect(page.intersect(ui.clip_rect()));
    paint_bloom(
        &painter,
        rect,
        texture,
        uv,
        bloom_glow(luma, app.palette.dark),
        0.45,
        RADIUS as f32,
    );
}

/// How the page bloom paints over a cover of lightness `luma`, for a dark or
/// light page.
///
/// Lucid draws it at 80% opacity and 80% brightness. A dark page carries white
/// title text, so a bright cover's bloom is dimmed until the words keep their
/// contrast. A light page carries dark text: a bright bloom is already the
/// field it needs, while a dark cover's bloom is thinned so the light page
/// glass, not the artwork, sits behind the words.
fn bloom_glow(luma: f32, dark: bool) -> Color32 {
    if dark {
        let bright = ((luma - 0.5) / 0.35).clamp(0.0, 1.0);
        let tint = (204.0 - 96.0 * bright) as u8;
        let alpha = (204.0 - 44.0 * bright) as u8;
        Color32::from_rgba_unmultiplied(tint, tint, tint, alpha)
    } else {
        let deep = ((0.5 - luma) / 0.35).clamp(0.0, 1.0);
        let tint = (212.0 + 32.0 * deep) as u8;
        let alpha = (200.0 - 120.0 * deep) as u8;
        Color32::from_rgba_unmultiplied(tint, tint, tint, alpha)
    }
}

/// The part of an image that fills `out` while keeping the image's aspect, as
/// CSS `background-size: cover` crops it: centred, cropping the longer side.
pub fn cover_uv(image: egui::Vec2, out: egui::Vec2) -> Rect {
    let whole = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    if image.x <= 0.0 || image.y <= 0.0 || out.x <= 0.0 || out.y <= 0.0 {
        return whole;
    }
    let image_aspect = image.x / image.y;
    let out_aspect = out.x / out.y;
    if image_aspect > out_aspect {
        let width = out_aspect / image_aspect;
        let margin = (1.0 - width) / 2.0;
        Rect::from_min_max(pos2(margin, 0.0), pos2(1.0 - margin, 1.0))
    } else {
        let height = image_aspect / out_aspect;
        let margin = (1.0 - height) / 2.0;
        Rect::from_min_max(pos2(0.0, margin), pos2(1.0, 1.0 - margin))
    }
}

/// Paints a cover through a vertical mask: solid until `hold`, then fading to
/// nothing at the bottom. The top edge follows the card's rounded corners.
fn paint_bloom(
    painter: &Painter,
    rect: Rect,
    texture: &egui::TextureHandle,
    uv: Rect,
    glow: Color32,
    hold: f32,
    radius: f32,
) {
    let mut rows = Vec::new();
    // A few extra rows across the corner radius, so its curve is followed.
    for step in 0..=6 {
        rows.push(radius * step as f32 / 6.0);
    }
    let steps = 24;
    for step in 1..=steps {
        rows.push(radius + (rect.height() - radius) * step as f32 / steps as f32);
    }
    let mut mesh = Mesh {
        texture_id: texture.id(),
        ..Default::default()
    };
    for (index, down) in rows.iter().enumerate() {
        let t = (down / rect.height()).clamp(0.0, 1.0);
        let y = rect.top() + down;
        let fade = if t <= hold {
            1.0
        } else {
            (1.0 - (t - hold) / (1.0 - hold)).clamp(0.0, 1.0)
        };
        // `glow` is premultiplied by its own alpha; the per-row fade needs the
        // colour before that, or a thin bloom would darken as it thins.
        let [red, green, blue, _] = glow.to_srgba_unmultiplied();
        let color =
            Color32::from_rgba_unmultiplied(red, green, blue, (f32::from(glow.a()) * fade) as u8);
        let across = uv.top() + uv.height() * t;
        // The rounded top corners: at height `down` the card's edge is this
        // far in from the straight side.
        let inset = if *down < radius {
            radius
                - (radius * radius - (radius - down) * (radius - down))
                    .max(0.0)
                    .sqrt()
        } else {
            0.0
        };
        let left = rect.left() + inset;
        let right = rect.right() - inset;
        mesh.vertices.push(Vertex {
            pos: pos2(left, y),
            uv: pos2(uv.left(), across),
            color,
        });
        mesh.vertices.push(Vertex {
            pos: pos2(right, y),
            uv: pos2(uv.right(), across),
            color,
        });
        if index > 0 {
            let base = (index as u32 - 1) * 2;
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base + 1, base + 3, base + 2);
        }
    }
    painter.add(Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unmultiplied(colour: Color32) -> [u8; 4] {
        colour.to_srgba_unmultiplied()
    }

    #[test]
    fn a_white_bloom_stays_bright_on_a_light_page() {
        let dark = bloom_glow(1.0, true);
        let light = bloom_glow(1.0, false);
        assert!(
            dark.a() < 180,
            "white text needs a dimmed bloom: {}",
            dark.a()
        );
        assert!(
            light.a() > dark.a() && unmultiplied(light)[0] > 200,
            "dark text needs a light field: {light:?}"
        );
        assert_eq!(
            bloom_glow(0.5, true).a(),
            204,
            "a mid-tone cover keeps Lucid's own strength"
        );
    }

    #[test]
    fn a_dark_bloom_is_thinned_on_a_light_page() {
        let light = bloom_glow(0.0, false);
        let dark = bloom_glow(0.0, true);
        assert!(
            light.a() < 100,
            "the light page glass must set the field: {}",
            light.a()
        );
        assert!(
            unmultiplied(light)[0] > 220,
            "it stays a light wash: {light:?}"
        );
        assert!(dark.a() > light.a());
    }

    /// Lucid's three glass choices step from an opaque card to one the
    /// backdrop clearly shows through, in both appearances.
    #[test]
    fn the_glass_choices_step_from_opaque_to_clear() {
        for dark in [true, false] {
            let clear = glass_alpha(dark, LucidGlass::Clear);
            let soft = glass_alpha(dark, LucidGlass::Soft);
            let off = glass_alpha(dark, LucidGlass::Off);
            assert_eq!(off, 255, "Off is opaque");
            assert!(clear < soft, "Clear shows more than Soft");
            assert!(soft < off);
            assert!(clear >= 80, "text stays legible over the cover");
            assert!(dark || clear >= 160, "light glass keeps its contrast");
        }
    }

    /// The clean background's cards are separated by their hairline alone, so
    /// it is drawn more firmly than over the cover.
    #[test]
    fn the_clean_background_draws_a_firmer_border() {
        let cover = border_alpha(LucidBackground::Cover);
        let dark = border_alpha(LucidBackground::Dark);
        assert!(dark > cover, "the clean cards need their own edge");
        assert!(cover >= 120, "the cover's hairline still shows");
        assert!(dark >= 200, "the clean hairline is not a whisper");
    }

    /// The backdrop crops rather than stretches: a wide cover fills a tall
    /// window by losing its sides, and keeps its centre.
    #[test]
    fn the_backdrop_crops_a_cover_to_fill_the_window() {
        let wide = cover_uv(vec2(960.0, 540.0), vec2(1440.0, 900.0));
        assert_eq!(wide.top(), 0.0);
        assert_eq!(wide.bottom(), 1.0);
        assert!(wide.left() > 0.0 && wide.right() < 1.0, "sides are cropped");
        assert!((wide.center().x - 0.5).abs() < f32::EPSILON, "centred");

        let tall = cover_uv(vec2(480.0, 800.0), vec2(1440.0, 900.0));
        assert_eq!(tall.left(), 0.0);
        assert_eq!(tall.right(), 1.0);
        assert!(
            tall.top() > 0.0 && tall.bottom() < 1.0,
            "top and bottom are cropped"
        );
        assert!((tall.center().y - 0.5).abs() < f32::EPSILON, "centred");

        // A square cover in a square hole is shown whole.
        assert_eq!(
            cover_uv(vec2(640.0, 640.0), vec2(400.0, 400.0)),
            cover_uv(vec2(1.0, 1.0), vec2(1.0, 1.0))
        );
    }
}
