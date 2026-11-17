//! Spicy Lyrics: the animated lyrics view, in the right column or full screen.
//!
//! The motion follows Spicy Lyrics' own renderer: a word held for at least a
//! second splits into letters, and each letter swells, lifts and glows as it is
//! sung, while a soft edge sweeps across the active word. Those timings are
//! used only when a source carries them: Spotify's colour lyrics syllables or
//! enhanced LRC word stamps. A line-synced source has no word or letter times,
//! so the line is swept as a whole at the line's own pace instead of inventing
//! a time for every character.
//!
//! Everything is drawn from glyphs and shapes. There is no browser engine and
//! no per-frame work proportional to the whole lyric sheet.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{
    Align, Color32, FontId, Frame, Layout, Margin, Pos2, Rect, Sense, UiBuilder, pos2, vec2,
};

use crate::app::App;
use crate::i18n::{gettext, pgettext};
use crate::lyrics::Lyrics;
use crate::model::{Action, Loadable, Page};
use crate::theme::{self, Icon};

use super::widgets;

/// Where the line being sung sits in the view: the reference keeps it on the
/// vertical centre of the words column.
const SUNG_LINE_AT: f32 = 0.5;
/// How often the view repaints while a song plays but nothing is moving.
/// While springs run, it asks for the next frame immediately, so the
/// compositor's own clock paces the motion instead of a timer racing vsync.
const FRAME_MS: u64 = 16;
/// Spicy's `--lyrics-line-height`, and the gap it bakes into each line as a
/// percentage of the lyrics container: `1cqw` for static or word-synced words
/// and `2cqw` for a line-synced line.
const LINE_LEADING: f32 = 1.181_818_2;
const WORD_LINE_GAP: f32 = 0.01;
const SYNCED_LINE_GAP: f32 = 0.02;
/// How opaque a sung line and a line still to come are. The main page uses
/// 0.497 and 0.51; the right-hand sidebar keeps upcoming lines brighter and
/// lets sung ones fall much further back (0.34 and 0.75).
const PAGE_SUNG_ALPHA: u8 = 127;
const PAGE_UNSUNG_ALPHA: u8 = 130;
const SIDEBAR_SUNG_ALPHA: u8 = 87;
const SIDEBAR_UNSUNG_ALPHA: u8 = 191;
/// How far behind the active line a line falls. Spicy's depth of field grows
/// with the distance from the active line and stops growing after five lines;
/// its sidebar blur is twice the page's, so the sidebar dims further.
const DEPTH_LINES: f32 = 5.465;
const PAGE_DEPTH_FLOOR: f32 = 0.58;
const SIDEBAR_DEPTH_FLOOR: f32 = 0.48;
/// The karaoke fill. Words run from 0.5 to 0.85 white across their own box;
/// a line-synced line runs from 0.35 to 0.85 down the whole line. The overlay
/// alpha is chosen so compositing over the rest alpha lands on Spicy's 0.85.
const WORD_REST_ALPHA: u8 = 128;
const WORD_FILL_ALPHA: u8 = 179;
const LINE_REST_ALPHA: u8 = 89;
const LINE_FILL_ALPHA: u8 = 196;
/// The fill stop starts before the box and crosses it a little past the end,
/// so the words are fully unsung at 0 and fully sung at 1. The visible edge is
/// a 20% soft band, not a hard wipe.
const FILL_START: f32 = -0.20;
const FILL_WIDTH: f32 = 1.20;
const FILL_BAND: f32 = 0.20;
/// A line-synced line gets one glow for the whole line.
const LINE_GLOW: &[(f32, f32)] = &[(0.0, 0.0), (0.5, 1.0), (1.0, 0.0)];
const LINE_GLOW_HZ: f32 = 1.0;
const LINE_GLOW_DAMPING: f32 = 0.50;
/// The letter and word animation curves, copied from Spicy Lyrics'
/// `LetterScaleRange`, `YOffsetRange` and `GlowRange`.
const LETTER_SCALE: &[(f32, f32)] = &[(0.0, 0.95), (0.7, 1.175), (1.0, 1.0)];
const WORD_SCALE: &[(f32, f32)] = &[(0.0, 0.95), (0.7, 1.0505), (1.0, 1.0)];
const LETTER_LIFT: &[(f32, f32)] = &[(0.0, 0.01), (0.9, -1.0 / 56.0), (1.0, 0.0)];
const WORD_LIFT: &[(f32, f32)] = &[(0.0, 0.01), (0.9, -1.0 / 60.0), (1.0, 0.0)];
const GLOW: &[(f32, f32)] = &[(0.0, 0.0), (0.15, 1.0), (0.6, 1.0), (1.0, 0.0)];
/// Spicy Lyrics chases those curves with damped springs rather than reading
/// them directly. These are its scale, Y offset and glow spring settings.
const SCALE_HZ: f32 = 0.88;
const SCALE_DAMPING: f32 = 0.64;
const LIFT_HZ: f32 = 1.45;
const LIFT_DAMPING: f32 = 0.40;
const GLOW_HZ: f32 = 1.18;
const GLOW_DAMPING: f32 = 0.56;

const PANEL_ID: &str = "spicy-lyrics-panel";

/// The lyrics beside the page, as Spicy Lyrics sits in the Spotify client's
/// right column. The page stays where it was; full screen is a separate step.
pub fn side_panel(app: &mut App, ui: &mut egui::Ui) {
    let palette = theme::Palette::dark();
    let fit = super::yielding_panel(
        ui.ctx(),
        PANEL_ID,
        theme::SIDE_PANEL_MIN_WIDTH..=640.0,
        app.settings.lyrics_width,
        ui.available_width() - super::topbar::least_width(ui.ctx()),
    );
    let frame = if super::lucid::enabled(app) {
        super::lucid::card(app, super::lucid::RADIUS)
            .inner_margin(Margin::symmetric(12, 12))
            .outer_margin(Margin {
                left: 8,
                right: 8,
                top: 8,
                bottom: 8,
            })
    } else {
        Frame::new()
            .fill(palette.panel)
            .inner_margin(Margin::symmetric(12, 12))
    };
    let panel = egui::Panel::right(PANEL_ID)
        .resizable(true)
        .default_size(app.settings.lyrics_width)
        .size_range(fit.range.clone())
        .show_separator_line(false)
        .frame(frame);
    let response = panel.show(ui, |ui| {
        // Local to the column, so the page beside it keeps its own theme.
        theme::apply_local(ui, &palette);
        let window_controls = super::window_controls_reservation(
            ui.ctx(),
            app.show_queue_panel,
            app.spicy_lyrics,
            ui.available_width(),
        );
        ui.add_space(window_controls.lyrics_top);
        panel_header(app, ui, &palette);
        ui.add_space(6.0);
        track_identity(app, ui, &palette);
        ui.add_space(8.0);
        let area = ui.available_rect_before_wrap();
        contents(app, ui, area, 4.0, false);
    });
    let width = response.response.rect.width();
    if (app.settings.lyrics_width - width).abs() > 1.0
        && super::panel_width_chosen(ui.ctx(), PANEL_ID, &fit)
    {
        app.settings.lyrics_width = width;
        app.actions.push(Action::SettingsChanged);
    }
}

/// The track title and artist under the Spicy masthead, with the provider
/// credit at the far right. Spicy keeps the identity block quiet so the words
/// remain the loudest thing in the column.
fn track_identity(app: &mut App, ui: &mut egui::Ui, palette: &theme::Palette) {
    let Some(now) = app.now_playing() else {
        return;
    };
    let (title, artist) = (now.title.clone(), now.subtitle.clone());
    let credit = match &app.lyrics {
        Loadable::Loaded(Some(lyrics)) => lyrics
            .attribution
            .as_ref()
            .map(|attribution| attribution.provider.clone()),
        _ => None,
    };
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            theme::text(ui, title, theme::semibold(13.0), Color32::WHITE);
            theme::text(ui, artist, theme::regular(11.5), palette.secondary);
        });
        if let Some(provider) = credit {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                theme::text(
                    ui,
                    format!("{} {provider}", gettext(app.locale, "Lyrics by")),
                    theme::medium(10.5),
                    palette.dim,
                );
            });
        }
    });
}

fn panel_header(app: &mut App, ui: &mut egui::Ui, palette: &theme::Palette) {
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        theme::icon(ui, Icon::Sparkles, 15.0, palette.accent);
        ui.add_space(4.0);
        theme::text(
            ui,
            gettext(app.locale, "Spicy Lyrics"),
            theme::bold(18.0),
            Color32::WHITE,
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::icon_button(
                ui,
                Icon::X,
                18.0,
                palette.secondary,
                palette.text,
                &gettext(app.locale, "Close"),
            )
            .clicked()
            {
                app.actions.push(Action::ToggleSpicyLyrics);
            }
            if theme::icon_button(
                ui,
                Icon::Expand,
                18.0,
                palette.secondary,
                palette.text,
                &gettext(app.locale, "Full screen lyrics"),
            )
            .clicked()
            {
                app.actions.push(Action::SetLyricsFullscreen(true));
            }
            if theme::icon_button(
                ui,
                Icon::Settings,
                18.0,
                palette.secondary,
                palette.text,
                &gettext(app.locale, "Settings"),
            )
            .clicked()
            {
                app.actions.push(Action::Open(Page::Settings));
            }
            let loaded = matches!(&app.lyrics, Loadable::Loaded(Some(_)));
            if loaded
                && !app.lyrics_following
                && theme::pill_button(
                    ui,
                    palette,
                    &pgettext(app.locale, "lyrics", "Follow"),
                    false,
                )
                .clicked()
            {
                app.actions.push(Action::FollowLyrics);
            }
        });
    });
}

/// The whole window, entered only from the full screen button, F, or the
/// player bar's expand button.
pub fn fullscreen(app: &mut App, ui: &mut egui::Ui) {
    egui::CentralPanel::default()
        .frame(Frame::new().fill(Color32::BLACK))
        .show(ui, |ui| {
            let rect = ui.max_rect();
            if app.settings.spicy_fullscreen_backdrop {
                backdrop(app, ui, rect);
            } else {
                ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
            }
            header(app, ui, rect);
            let top = rect.top() + theme::titlebar_inset(ui.ctx()) + 84.0;
            let outer = Rect::from_min_max(
                pos2(rect.left() + 48.0, top),
                pos2(rect.right() - 48.0, rect.bottom() - 40.0),
            );
            if app.now_playing().is_none() || outer.width() < 820.0 {
                contents(app, ui, outer, 0.0, true);
                return;
            }
            // Spicy's full page is a media column and a lyric column, not one
            // centred sheet. The cover anchors the song; the words take the
            // rest and keep their own left edge.
            let gap = 96.0;
            let side = (outer.height() - 96.0)
                .min(outer.width() * 0.42)
                .clamp(200.0, 450.0);
            let lyrics_width = (outer.width() - side - gap).min(720.0);
            let left = outer.center().x - (side + gap + lyrics_width) / 2.0;
            let media = Rect::from_min_size(
                pos2(left, outer.center().y - (side + 88.0) / 2.0),
                vec2(side, side + 88.0),
            );
            media_panel(app, ui, media);
            let lyrics = Rect::from_min_max(
                pos2(media.right() + gap, outer.top()),
                pos2(media.right() + gap + lyrics_width, outer.bottom()),
            );
            contents(app, ui, lyrics, 0.0, true);
        });
}

/// The cover, progress and song identity beside the full-screen words.
fn media_panel(app: &mut App, ui: &mut egui::Ui, column: Rect) {
    let Some(now) = app.now_playing() else {
        return;
    };
    let side = column.width().min(column.height() - 88.0);
    let cover = Rect::from_min_size(column.min, vec2(side, side));
    ui.painter().add(
        egui::epaint::Shadow {
            offset: [0, 20],
            blur: 54,
            spread: 0,
            color: Color32::from_black_alpha(145),
        }
        .as_shape(cover, 10.0),
    );
    widgets::playing_cover(
        ui,
        &theme::Palette::dark(),
        app.backend.art(),
        "spicy-fullscreen",
        now.art_url.as_deref().or(now.art_small.as_deref()),
        cover,
        10.0,
    );
    let bar = Rect::from_min_size(
        pos2(cover.left(), cover.bottom() + 18.0),
        vec2(cover.width(), 3.0),
    );
    let painter = ui.painter();
    painter.rect_filled(bar, 2.0, Color32::from_white_alpha(48));
    let progress = if now.duration_ms == 0 {
        0.0
    } else {
        (now.position_ms as f32 / now.duration_ms as f32).clamp(0.0, 1.0)
    };
    let played = Rect::from_min_size(bar.min, vec2(bar.width() * progress, bar.height()));
    painter.rect_filled(played, 2.0, Color32::from_white_alpha(235));
    painter.text(
        pos2(bar.left(), bar.bottom() + 6.0),
        egui::Align2::LEFT_TOP,
        format_time(now.position_ms),
        theme::regular(11.0),
        Color32::from_white_alpha(175),
    );
    painter.text(
        pos2(bar.right(), bar.bottom() + 6.0),
        egui::Align2::RIGHT_TOP,
        format_time(now.duration_ms),
        theme::regular(11.0),
        Color32::from_white_alpha(175),
    );
    let text = Rect::from_min_max(
        pos2(cover.left(), bar.bottom() + 24.0),
        pos2(cover.right(), column.bottom()),
    );
    let mut text = ui.new_child(
        UiBuilder::new()
            .max_rect(text)
            .layout(Layout::top_down(Align::Center)),
    );
    text.spacing_mut().item_spacing.y = 3.0;
    text.add(
        egui::Label::new(
            egui::RichText::new(&now.title)
                .font(theme::semibold(21.0))
                .color(Color32::WHITE),
        )
        .wrap(),
    );
    text.add(
        egui::Label::new(
            egui::RichText::new(&now.subtitle)
                .font(theme::regular(13.5))
                .color(Color32::from_white_alpha(190)),
        )
        .wrap(),
    );
    if let Loadable::Loaded(Some(lyrics)) = &app.lyrics
        && let Some(attribution) = lyrics.attribution.as_ref()
    {
        text.add_space(2.0);
        text.add(
            egui::Label::new(
                egui::RichText::new(format!(
                    "{} {}",
                    gettext(app.locale, "Lyrics by"),
                    attribution.provider
                ))
                .font(theme::medium(11.0))
                .color(Color32::from_white_alpha(125)),
            )
            .wrap(),
        );
    }
}

fn format_time(ms: u32) -> String {
    let seconds = ms / 1_000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// The playing cover's blurred backdrop under a dark scrim and an accent
/// bloom, so the words read whatever the art is.
fn backdrop(app: &mut App, ui: &mut egui::Ui, rect: Rect) {
    theme::apply_local(ui, &theme::Palette::dark());
    let art = app
        .now_playing()
        .and_then(|now| now.art_url.or(now.art_small));
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    let painter = ui.painter().with_clip_rect(rect);
    match app
        .lyrics_backdrop
        .texture(ui.ctx(), app.backend.art(), art.as_deref())
    {
        Some(texture) => {
            painter.image(texture.id(), rect, uv, Color32::from_gray(200));
        }
        None => {
            // No cover to blur yet: keep Spicy's coloured field rather than
            // dropping to a flat black sheet.
            painter.rect_filled(rect, 0.0, Color32::from_rgb(12, 13, 15));
            let accent = app.palette.accent;
            widgets::paint_vertical_gradient(
                ui,
                rect,
                Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 64),
                Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 7),
            );
        }
    }
    widgets::paint_vertical_gradient(
        ui,
        rect,
        Color32::from_black_alpha(110),
        Color32::from_black_alpha(195),
    );
    let accent = app.palette.accent;
    let bloom = Rect::from_min_max(
        rect.min,
        pos2(rect.right(), rect.top() + rect.height() * 0.45),
    );
    widgets::paint_vertical_gradient(
        ui,
        bloom,
        Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 22),
        Color32::TRANSPARENT,
    );
}

fn header(app: &mut App, ui: &mut egui::Ui, rect: Rect) {
    let top = rect.top() + theme::titlebar_inset(ui.ctx()) + 20.0;
    let band = Rect::from_min_size(
        pos2(rect.left() + 28.0, top),
        vec2((rect.width() - 56.0).max(120.0), 30.0),
    );
    let mut row = ui.new_child(
        UiBuilder::new()
            .max_rect(band)
            .layout(Layout::left_to_right(Align::Center)),
    );
    theme::icon(&mut row, Icon::Sparkles, 15.0, app.palette.accent);
    row.add_space(4.0);
    theme::text(
        &mut row,
        gettext(app.locale, "Spicy Lyrics"),
        theme::bold(15.0),
        Color32::WHITE,
    );
    let palette = theme::Palette::dark();
    row.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if theme::icon_button(
            ui,
            Icon::X,
            18.0,
            Color32::WHITE,
            Color32::WHITE,
            &gettext(app.locale, "Close Spicy Lyrics (Esc)"),
        )
        .clicked()
        {
            app.actions.push(Action::ToggleSpicyLyrics);
        }
        if theme::icon_button(
            ui,
            Icon::Shrink,
            18.0,
            Color32::WHITE,
            Color32::WHITE,
            &gettext(app.locale, "Leave full screen (Esc)"),
        )
        .clicked()
        {
            app.actions.push(Action::SetLyricsFullscreen(false));
        }
        if theme::icon_button(
            ui,
            Icon::Settings,
            18.0,
            Color32::WHITE,
            Color32::WHITE,
            &gettext(app.locale, "Settings"),
        )
        .clicked()
        {
            app.actions.push(Action::SetLyricsFullscreen(false));
            app.actions.push(Action::Open(Page::Settings));
        }
        let loaded = matches!(&app.lyrics, Loadable::Loaded(Some(_)));
        if loaded
            && !app.lyrics_following
            && theme::pill_button(
                ui,
                &palette,
                &pgettext(app.locale, "lyrics", "Follow"),
                false,
            )
            .clicked()
        {
            app.actions.push(Action::FollowLyrics);
        }
    });
}

/// The room the lyrics and their messages take, below the header.
fn inner(rect: Rect) -> Rect {
    Rect::from_min_max(
        pos2(rect.left() + 40.0, rect.top() + 90.0),
        pos2(rect.right() - 40.0, rect.bottom() - 30.0),
    )
}

/// Spicy's type scale: 7cqw of the lyrics column, floored at 2.1rem in the
/// sidebar and 1.85rem on the full page, capped at 3.5rem. Static lyrics are
/// deliberately smaller and lighter: 5cqw, clamped between 0.8 and 2.5rem.
fn lyric_size(width: f32, fullscreen: bool, synced: bool) -> f32 {
    if !synced {
        return (width * 0.05).clamp(12.8, 40.0);
    }
    let floor = if fullscreen { 29.6 } else { 33.6 };
    (width * 0.07).clamp(floor, 56.0)
}

/// The space after each line, in the container-relative units Spicy uses.
/// A line-synced source gets 2cqw; static and word-synced lines get 1cqw.
fn line_gap(width: f32, synced: bool, word_synced: bool) -> f32 {
    width
        * if synced && !word_synced {
            SYNCED_LINE_GAP
        } else {
            WORD_LINE_GAP
        }
}

/// How far a line falls back behind the active one, as a multiplier on its
/// opacity.
///
/// Spicy blurs by distance from the active line, which egui cannot do to one
/// line's glyphs without an offscreen pass. Drawing the line once and dimming
/// it by the same distance keeps the depth ordering and costs one draw per
/// glyph; repeating a glyph to fake the gaussian reads as a halo and multiplies
/// the draw count for every line on screen.
fn line_depth(index: usize, active: Option<usize>, fullscreen: bool) -> f32 {
    let Some(active) = active else {
        return 1.0;
    };
    if index == active {
        return 1.0;
    }
    let floor = if fullscreen {
        PAGE_DEPTH_FLOOR
    } else {
        SIDEBAR_DEPTH_FLOOR
    };
    let ramp = (index.abs_diff(active) as f32 / DEPTH_LINES).min(1.0);
    1.0 - (1.0 - floor) * ramp
}

/// A centred lyrics notice, as Spicy shows one in place of the words: a quiet
/// title, a line of explanation under it, and no icon or card around them.
/// Returns whether the optional button under it was clicked.
fn notice(
    ui: &mut egui::Ui,
    rect: Rect,
    fullscreen: bool,
    title: &str,
    body: &str,
    button: Option<&str>,
) -> bool {
    let palette = theme::Palette::dark();
    let title_size = if fullscreen { 22.0 } else { 15.0 };
    let body_size = if fullscreen { 14.0 } else { 12.0 };
    let mut area = ui.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Center)),
    );
    area.add_space((rect.height() * SUNG_LINE_AT - title_size * 1.4).max(24.0));
    theme::text(&mut area, title, theme::semibold(title_size), palette.text);
    area.add_space(6.0);
    theme::text(
        &mut area,
        body,
        theme::regular(body_size),
        palette.secondary,
    );
    let Some(label) = button else {
        return false;
    };
    area.add_space(14.0);
    theme::pill_button(&mut area, &palette, label, false).clicked()
}

/// How many lines the Now Playing preview shows, the sung one included.
const NOW_PLAYING_LINES: usize = 5;

/// The small live lyrics block in the Now Playing column: the same Spicy
/// renderer the column and full view use, showing a few lines around the sung
/// one with word fills, letter swells and glow, and a way into the full view.
/// It takes no scroll and no more height than its lines.
pub fn now_playing(app: &mut App, ui: &mut egui::Ui, palette: &theme::Palette) {
    if !app.settings.spicy_now_playing {
        return;
    }
    let Some(now) = app.now_playing() else {
        return;
    };
    let uri = now.uri.clone();
    let position = now.position_ms;
    ui.horizontal(|ui| {
        theme::section_title(ui, palette, &gettext(app.locale, "Spicy Lyrics"));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::icon_button(
                ui,
                Icon::Expand,
                15.0,
                palette.secondary,
                palette.text,
                &gettext(app.locale, "Full screen lyrics"),
            )
            .clicked()
            {
                app.actions.push(Action::SetLyricsFullscreen(true));
            }
        });
    });
    ui.add_space(6.0);
    let lyrics = match &app.lyrics {
        Loadable::Loaded(Some(lyrics))
            if app.lyrics_uri.as_deref() == Some(uri.as_str()) && !lyrics.instrumental =>
        {
            lyrics.clone()
        }
        _ => {
            theme::subtle(ui, palette, &gettext(app.locale, "No lyrics"));
            return;
        }
    };
    let Some(active) = lyrics.active_line(position) else {
        theme::subtle(
            ui,
            palette,
            &gettext(app.locale, "No timed lyrics for this track."),
        );
        return;
    };
    // Keep the sung line inside a full window near the song's end instead of
    // leaving the block mostly empty there.
    let start = active
        .saturating_sub(1)
        .min(lyrics.lines.len().saturating_sub(NOW_PLAYING_LINES));
    let end = (start + NOW_PLAYING_LINES).min(lyrics.lines.len());
    let width = ui.available_width();
    let word_synced = lyrics.lines.iter().any(|line| !line.syllables.is_empty());
    // Spicy's sidebar type is larger than this block can hold, so its shapes
    // are drawn at the size the column has room for.
    let size = (width * 0.052).clamp(14.5, 17.5);
    let gap = line_gap(width, lyrics.synced, word_synced).max(5.0);
    let motion_id = egui::Id::new("spicy-now-playing-motion");
    let mut motion = ui
        .ctx()
        .data_mut(|data| data.remove_temp::<Motion>(motion_id))
        .unwrap_or_default();
    motion.begin(&uri);
    if !app.settings.spicy_now_playing_motion {
        motion.dt = 0.0;
    }
    let left = ui.available_rect_before_wrap().left();
    // The column can sit on a light surface, where Spicy's white words would
    // vanish; use the surface's own text colour there.
    let text_color = if palette.dark {
        Color32::WHITE
    } else {
        palette.text
    };
    let mut measure = Measure::default();
    for (index, line) in lyrics.lines.iter().enumerate().take(end).skip(start) {
        draw_line(
            app,
            ui,
            &lyrics,
            index,
            line,
            position,
            now.duration_ms,
            left,
            width,
            size,
            text_color,
            index == active,
            line_depth(index, Some(active), false),
            SIDEBAR_SUNG_ALPHA,
            SIDEBAR_UNSUNG_ALPHA,
            &mut measure,
            &mut motion,
        );
        ui.add_space(gap);
    }
    let animating = motion.animating();
    ui.ctx()
        .data_mut(|data| data.insert_temp(motion_id, motion));
    // The block keeps its own frame clock, as the column does; without one it
    // only moved when something else happened to repaint.
    if animating {
        ui.ctx().request_repaint();
    } else if now.playing {
        ui.ctx()
            .request_repaint_after(Duration::from_millis(FRAME_MS));
    }
}

fn contents(app: &mut App, ui: &mut egui::Ui, rect: Rect, top_inset: f32, fullscreen: bool) {
    let palette = theme::Palette::dark();
    let area_rect = || if fullscreen { rect } else { inner(rect) };
    let Some(now) = app.now_playing() else {
        notice(
            ui,
            area_rect(),
            fullscreen,
            &gettext(app.locale, "Nothing playing"),
            &gettext(app.locale, "Play a song to see its lyrics."),
            None,
        );
        return;
    };
    let lyrics = match &app.lyrics {
        Loadable::NotLoaded | Loadable::Loading => {
            let mut area = ui.new_child(UiBuilder::new().max_rect(area_rect()));
            widgets::loading_row(&mut area, &palette, app.locale);
            return;
        }
        Loadable::Failed(error) => {
            let message = gettext(
                app.locale,
                // Translators: Keep {error} unchanged. It is the original failure detail.
                "Couldn't fetch the lyrics: {error}",
            )
            .replace("{error}", error);
            if notice(
                ui,
                area_rect(),
                fullscreen,
                &gettext(app.locale, "Couldn't get the lyrics"),
                &message,
                Some(&gettext(app.locale, "Try again")),
            ) {
                app.actions.push(Action::RetryLyrics);
            }
            return;
        }
        Loadable::Loaded(None) => {
            notice(
                ui,
                area_rect(),
                fullscreen,
                &gettext(app.locale, "No lyrics"),
                &gettext(app.locale, "No lyrics found for this track."),
                None,
            );
            return;
        }
        Loadable::Loaded(Some(lyrics)) if lyrics.instrumental => {
            notice(
                ui,
                area_rect(),
                fullscreen,
                &gettext(app.locale, "Instrumental"),
                &gettext(app.locale, "No timed lyrics for this track."),
                None,
            );
            return;
        }
        Loadable::Loaded(Some(lyrics)) => lyrics.clone(),
    };
    let motion_id = egui::Id::new("spicy-lyrics-motion");
    let mut motion = ui
        .ctx()
        .data_mut(|data| data.remove_temp::<Motion>(motion_id))
        .unwrap_or_default();
    motion.begin(&now.uri);

    let active = lyrics.active_line(now.position_ms);
    // A Follow click resets the remembered line after drawing, as the side
    // panel does; other frames record the shown line first.
    if !app
        .actions
        .iter()
        .any(|action| matches!(action, Action::FollowLyrics))
    {
        app.actions.push(Action::LyricsLineShown(active));
    }
    let width = if fullscreen {
        rect.width()
    } else {
        (rect.width() * 0.86).clamp(200.0, 1180.0).min(rect.width())
    };
    let column = if fullscreen {
        rect
    } else {
        Rect::from_min_max(
            pos2(rect.center().x - width / 2.0, rect.top() + top_inset),
            pos2(rect.center().x + width / 2.0, rect.bottom() - 18.0),
        )
    };
    let mut ui = ui.new_child(UiBuilder::new().max_rect(column));
    let viewport = ui.available_rect_before_wrap();
    let size = lyric_size(width, fullscreen, lyrics.synced);
    let word_synced = lyrics.lines.iter().any(|line| !line.syllables.is_empty());
    let gap = line_gap(width, lyrics.synced, word_synced);
    let (sung_alpha, unsung_alpha) = if fullscreen {
        (PAGE_SUNG_ALPHA, PAGE_UNSUNG_ALPHA)
    } else {
        (SIDEBAR_SUNG_ALPHA, SIDEBAR_UNSUNG_ALPHA)
    };
    let manual_scroll = ui.rect_contains_pointer(viewport)
        && ui.input(|input| {
            input.smooth_scroll_delta.y != 0.0
                || (input.pointer.primary_down() && input.pointer.delta().y != 0.0)
        });
    let following = app.lyrics_following && !manual_scroll;
    // Keep re-asserting the active line while following. The first layout can
    // still be settling when a line changes, and a one-shot scroll would stay
    // a row or two behind it.
    let follow = following;
    let animation = crate::motion::scroll(ui.ctx(), 0.45);
    // egui's edge fade paints the panel's fill over the scroll edge. On the
    // full-screen art backdrop that becomes an opaque black band, so the
    // full-screen view opts out and keeps Spicy's clean edge.
    ui.spacing_mut().scroll.fade.strength = if fullscreen { 0.0 } else { 0.6 };
    egui::ScrollArea::vertical()
        .id_salt(("spicy-lyrics-scroll", &now.uri))
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .auto_shrink([false, false])
        .show(&mut ui, |ui| {
            // Room for the first line to sit where a sung line sits, and for
            // the last to rise to it.
            let padding = if lyrics.synced {
                (viewport.height() * SUNG_LINE_AT - size).max(12.0)
            } else {
                12.0
            };
            ui.add_space(padding);
            let mut measure = Measure::default();
            let mut active_rect = None;
            for (index, line) in lyrics.lines.iter().enumerate() {
                let is_active = active == Some(index);
                let depth = line_depth(index, active, fullscreen);
                let rect = draw_line(
                    app,
                    ui,
                    &lyrics,
                    index,
                    line,
                    now.position_ms,
                    now.duration_ms,
                    column.left(),
                    width,
                    size,
                    Color32::WHITE,
                    is_active,
                    depth,
                    sung_alpha,
                    unsung_alpha,
                    &mut measure,
                    &mut motion,
                );
                if is_active {
                    active_rect = Some(rect);
                }
                ui.add_space(gap);
            }
            if let Some(attribution) = lyrics.attribution.as_ref() {
                widgets::lyrics_attribution(
                    app,
                    ui,
                    attribution,
                    (size * 0.55).clamp(11.0, 15.0),
                    &palette,
                );
            }
            // Words without timing can only be followed by the clock.
            if following && !lyrics.synced && now.duration_ms > 0 {
                let fraction =
                    (f64::from(now.position_ms) / f64::from(now.duration_ms)).clamp(0.0, 1.0);
                let content = ui.min_rect();
                let y = content.top() + content.height() * fraction as f32;
                ui.scroll_to_rect_animation(
                    Rect::from_min_max(pos2(content.left(), y), pos2(content.right(), y + 1.0)),
                    Some(Align::Center),
                    animation,
                );
            }
            ui.add_space((viewport.height() * (1.0 - SUNG_LINE_AT)).max(60.0));
            // Scroll only after the lines, credits and bottom spacer are all
            // allocated, so the target is never clamped to the content built
            // so far. That clamp is what kept the active line above centre.
            if let Some(rect) = active_rect
                && follow
            {
                show_sung_line(ui, rect, Some(animation), SUNG_LINE_AT);
            }
        });
    let animating = motion.animating();
    ui.ctx()
        .data_mut(|data| data.insert_temp(motion_id, motion));
    // Scrolling by hand means the reader wants to look elsewhere; the Follow
    // button in the header picks the song back up.
    if manual_scroll && app.lyrics_following {
        app.actions.push(Action::PauseLyricsFollow);
    }
    if animating {
        ui.ctx().request_repaint();
    } else if now.playing {
        ui.ctx()
            .request_repaint_after(Duration::from_millis(FRAME_MS));
    }
}

/// A glyph size rounded to a quarter pixel. The springs change the scale
/// every frame; laying text out at a slightly different size each time makes
/// the shaper rasterise a new glyph and the hinting snap between sizes, which
/// reads as a tremor. A quarter pixel is invisible and leaves one galley per
/// real step.
fn animated_font(size: f32) -> FontId {
    theme::bold((size * 4.0).round() / 4.0)
}

/// One word or split letter, with its karaoke fill and optional glow. Glow is
/// drawn behind every glyph in a separate pass, as the extension's text shadow
/// is, so it can never cover the words around it.
struct SpicyGlyph {
    galley: Arc<egui::Galley>,
    halo: Option<Arc<egui::Galley>>,
    pos: Pos2,
    halo_pos: Pos2,
    halo_alpha: u8,
    fill: f32,
    vertical: bool,
    rest_alpha: u8,
    fill_alpha: u8,
}

#[allow(clippy::too_many_arguments)]
fn push_glyph(
    glyphs: &mut Vec<SpicyGlyph>,
    painter: &egui::Painter,
    text: &str,
    font: FontId,
    center: Pos2,
    halo_alpha: u8,
    fill: f32,
    vertical: bool,
    rest_alpha: u8,
    fill_alpha: u8,
) {
    let galley = painter.layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER);
    let pos = pos2(
        center.x - galley.size().x / 2.0,
        center.y - galley.size().y / 2.0,
    );
    let (halo, halo_pos) = if halo_alpha > 0 {
        let halo = painter.layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER);
        let halo_pos = pos2(
            center.x - halo.size().x / 2.0,
            center.y - halo.size().y / 2.0,
        );
        (Some(halo), halo_pos)
    } else {
        (None, pos)
    };
    glyphs.push(SpicyGlyph {
        galley,
        halo,
        pos,
        halo_pos,
        halo_alpha,
        fill,
        vertical,
        rest_alpha,
        fill_alpha,
    });
}

/// Spicy draws white glyphs because its surfaces are dark. The Now Playing
/// column keeps the interface's light appearance, so the same renderer takes
/// its glyph colour from the surface it sits on.
fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

fn paint_glyphs(
    painter: &egui::Painter,
    glyphs: &[SpicyGlyph],
    color: Color32,
    line_fill_y: f32,
    clip: Rect,
) {
    // Same-size copies at the edge of a small disc make a soft shadow behind
    // the glyph. A larger second galley reads as a duplicate word, not glow.
    for glyph in glyphs
        .iter()
        .filter_map(|glyph| glyph.halo.as_ref().map(|halo| (glyph, halo)))
    {
        let alpha = (u16::from(glyph.0.halo_alpha) / 8).max(1) as u8;
        for offset in [
            vec2(-3.0, 0.0),
            vec2(3.0, 0.0),
            vec2(0.0, -3.0),
            vec2(0.0, 3.0),
            vec2(-2.0, -2.0),
            vec2(2.0, 2.0),
            vec2(-2.0, 2.0),
            vec2(2.0, -2.0),
        ] {
            painter.galley(
                glyph.0.halo_pos + offset,
                glyph.1.clone(),
                with_alpha(color, alpha),
            );
        }
    }
    // One draw per glyph. Spicy blurs inactive lines as a depth-of-field
    // effect; here the same distance falloff is carried by the line's alpha
    // (see `line_depth`), because repeating a glyph to fake a gaussian reads
    // as a halo and multiplies the draw count for every line on screen.
    for glyph in glyphs {
        painter.galley(
            glyph.pos,
            glyph.galley.clone(),
            with_alpha(color, glyph.rest_alpha),
        );
    }
    for glyph in glyphs {
        if glyph.fill <= 0.001 {
            continue;
        }
        let rect = Rect::from_min_size(glyph.pos, glyph.galley.size());
        // The gradient is a 20% soft band, not a hard wipe. Paint the settled
        // part solid, then fade the band out in slices over the resting glyph.
        let (solid, band, edge) = if glyph.vertical {
            let edge = line_fill_y.clamp(rect.top(), rect.bottom());
            (
                Rect::from_min_max(pos2(rect.left(), rect.top()), pos2(rect.right(), edge)),
                rect.height() * FILL_BAND,
                edge,
            )
        } else {
            let edge = (rect.left() + rect.width() * glyph.fill).clamp(rect.left(), rect.right());
            (
                Rect::from_min_max(rect.min, pos2(edge, rect.bottom())),
                rect.width() * FILL_BAND,
                edge,
            )
        };
        let solid = solid.intersect(rect).intersect(clip);
        if solid.width() > 0.0 && solid.height() > 0.0 {
            painter.with_clip_rect(solid).galley(
                glyph.pos,
                glyph.galley.clone(),
                with_alpha(color, glyph.fill_alpha),
            );
        }
        if band <= 0.0 || glyph.fill >= 0.999 && !glyph.vertical {
            continue;
        }
        const STEPS: u8 = 6;
        for step in 0..STEPS {
            let from = f32::from(step) / f32::from(STEPS);
            let to = f32::from(step + 1) / f32::from(STEPS);
            let piece = if glyph.vertical {
                Rect::from_min_max(
                    pos2(rect.left(), edge + band * from),
                    pos2(rect.right(), edge + band * to),
                )
            } else {
                Rect::from_min_max(
                    pos2(edge + band * from, rect.top()),
                    pos2(edge + band * to, rect.bottom()),
                )
            };
            let piece = piece.intersect(rect).intersect(clip);
            if piece.width() <= 0.0 || piece.height() <= 0.0 {
                continue;
            }
            // The band falls from the filled alpha to the resting alpha; the
            // overlay carries only the filled part because the rest glyph is
            // already underneath.
            let t = 1.0 - (from + to) * 0.5;
            let alpha = (f32::from(glyph.fill_alpha) * t).round() as u8;
            if alpha == 0 {
                continue;
            }
            painter.with_clip_rect(piece).galley(
                glyph.pos,
                glyph.galley.clone(),
                with_alpha(color, alpha),
            );
        }
    }
}

/// Font advances reused across a line and between lines with the same size.
#[derive(Default)]
struct Measure {
    size: f32,
    advances: HashMap<char, f32>,
    words: HashMap<String, f32>,
}

/// A damped spring, solved in closed form so its motion does not depend on
/// how often the window happens to repaint.
#[derive(Clone, Copy)]
struct Spring {
    value: f32,
    velocity: f32,
    goal: f32,
}

impl Spring {
    fn new(value: f32) -> Self {
        Self {
            value,
            velocity: 0.0,
            goal: value,
        }
    }

    fn moving(&self) -> bool {
        self.velocity.abs() > 0.0005 || (self.value - self.goal).abs() > 0.0005
    }

    fn step(&mut self, dt: f32, hz: f32, damping: f32) -> f32 {
        if dt <= 0.0 {
            return self.value;
        }
        let omega = std::f32::consts::TAU * hz;
        let decay = (-damping * omega * dt).exp();
        let offset = self.value - self.goal;
        if damping >= 1.0 {
            let c = self.velocity + damping * omega * offset;
            self.value = self.goal + (offset + c * dt) * decay;
            self.velocity = (self.velocity - c * damping * omega * dt) * decay;
            return self.value;
        }
        // Underdamped springs oscillate, so the exact solution needs both a
        // sine and a cosine term. A linear envelope would only critically damp.
        let damped = omega * (1.0 - damping * damping).sqrt();
        let (sin, cos) = (damped * dt).sin_cos();
        let b = (self.velocity + damping * omega * offset) / damped;
        self.value = self.goal + decay * (offset * cos + b * sin);
        self.velocity = decay
            * ((-damping * omega * offset + b * damped) * cos
                + (-damping * omega * b - offset * damped) * sin);
        self.value
    }
}

impl Default for Spring {
    fn default() -> Self {
        Self::new(0.0)
    }
}

#[derive(Clone, Copy)]
struct Springs {
    scale: Spring,
    lift: Spring,
    glow: Spring,
}

impl Springs {
    fn idle() -> Self {
        Self {
            scale: Spring::new(spline(LETTER_SCALE, 0.0)),
            lift: Spring::new(spline(LETTER_LIFT, 0.0)),
            glow: Spring::new(0.0),
        }
    }
}

impl Default for Springs {
    fn default() -> Self {
        Self::idle()
    }
}

/// Per-element spring state for the view. Keeping it in egui's temporary data
/// lets the side panel, full screen and Now playing preview share one clock.
#[derive(Clone, Default)]
struct Motion {
    uri: String,
    last_frame: Option<Instant>,
    dt: f32,
    springs: HashMap<(u32, u32, u32), Springs>,
}

impl Motion {
    fn begin(&mut self, uri: &str) {
        let now = Instant::now();
        if self.uri != uri {
            self.uri.clear();
            self.uri.push_str(uri);
            self.springs.clear();
            self.last_frame = Some(now);
            self.dt = 0.0;
            return;
        }
        self.dt = self
            .last_frame
            .map_or(0.0, |last| (now - last).as_secs_f32())
            .clamp(0.0, 0.05);
        self.last_frame = Some(now);
    }

    fn animating(&self) -> bool {
        self.springs
            .values()
            .any(|springs| springs.scale.moving() || springs.lift.moving() || springs.glow.moving())
    }
}

fn proximity(distance: f32) -> f32 {
    1.0 / (1.0 + distance.powf(2.8))
}

fn glow_proximity(distance: f32) -> f32 {
    1.0 / (1.0 + 0.9 * distance)
}

impl Measure {
    fn advance(&mut self, ui: &egui::Ui, font: &FontId, size: f32, ch: char) -> f32 {
        if (self.size - size).abs() > f32::EPSILON {
            self.size = size;
            self.advances.clear();
            self.words.clear();
        }
        if let Some(width) = self.advances.get(&ch) {
            return *width;
        }
        let width = ui
            .painter()
            .layout_no_wrap(ch.to_string(), font.clone(), Color32::WHITE)
            .size()
            .x
            .max(if ch == ' ' { size * 0.25 } else { 0.0 });
        self.advances.insert(ch, width);
        width
    }

    /// The shaped width of a word, so manual wrapping uses the same advances
    /// as the galley the passive path paints.
    fn word(&mut self, ui: &egui::Ui, font: &FontId, size: f32, text: &str) -> f32 {
        if (self.size - size).abs() > f32::EPSILON {
            self.size = size;
            self.advances.clear();
            self.words.clear();
        }
        if let Some(width) = self.words.get(text) {
            return *width;
        }
        let width = ui
            .painter()
            .layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)
            .size()
            .x;
        self.words.insert(text.to_owned(), width);
        width
    }
}

/// Draws one line and returns the rect its words occupy.
#[allow(clippy::too_many_arguments)]
fn draw_line(
    app: &mut App,
    ui: &mut egui::Ui,
    lyrics: &Lyrics,
    index: usize,
    line: &crate::lyrics::Line,
    position: u32,
    duration: u32,
    left: f32,
    width: f32,
    size: f32,
    text_color: Color32,
    is_active: bool,
    depth: f32,
    sung_alpha: u8,
    unsung_alpha: u8,
    measure: &mut Measure,
    motion: &mut Motion,
) -> Rect {
    let font = theme::bold(size);
    let text = if line.text.is_empty() && lyrics.synced {
        "\u{266a}"
    } else {
        line.text.as_str()
    };
    if !lyrics.synced || crate::bidi::is_rtl(text) {
        let alpha = if lyrics.synced { unsung_alpha } else { 255 };
        return draw_plain_line(
            app,
            ui,
            line,
            text,
            left,
            width,
            size,
            text_color,
            is_active,
            lyrics.synced,
            alpha,
            depth,
        );
    }
    let units = lyrics.units(index, duration);
    if units.is_empty() {
        return draw_plain_line(
            app,
            ui,
            line,
            text,
            left,
            width,
            size,
            text_color,
            is_active,
            lyrics.synced,
            unsung_alpha,
            depth,
        );
    }
    let max_width = width;
    let line_length = lyrics
        .line_end(index, duration)
        .unwrap_or(line.at_ms.unwrap_or(0));
    // Only source-timed lyrics carry word or syllable times. Everything else
    // is one line sweep at the line's own pace.
    let precise = lyrics.synced && !line.syllables.is_empty();
    let spring_dt = motion.dt * app.settings.spicy_motion.scale();
    let line_progress = if lyrics.synced {
        progress(position, line.at_ms.unwrap_or(0), line_length)
    } else {
        1.0
    };
    // Where each word landed, and on which row.
    // Lay every line out at full size. Active words grow from the extension's
    // 0.95 resting scale inside those slots, and inactive lines are painted
    // through the same slots at 0.95. Activation therefore cannot rewrap a
    // line, and word gaps stay full size instead of being eaten by the swell.
    let layout_size = size;
    let layout_font = theme::bold(layout_size);
    let mut x = 0.0;
    let mut row = 0;
    let mut unit_layout: Vec<(usize, f32, f32, u32)> = Vec::new();
    for (unit_index, unit) in units.iter().enumerate() {
        let width = measure.word(ui, &layout_font, layout_size, &unit.text);
        let gap = if unit.space_before && !unit_layout.is_empty() {
            // The same advance the passive galley uses, so a line never
            // reserves a wrapped row the painted text does not fill.
            measure.advance(ui, &layout_font, layout_size, ' ')
        } else {
            0.0
        };
        if x > 0.0 && x + gap + width > max_width {
            row += 1;
            x = 0.0;
            unit_layout.push((unit_index, 0.0, width, row));
            x += width;
            continue;
        }
        unit_layout.push((unit_index, x + gap, width, row));
        x += gap + width;
    }
    let line_height = size * LINE_LEADING;
    let (slot, _) = ui.allocate_exact_size(
        vec2(max_width, row as f32 * line_height + line_height),
        Sense::hover(),
    );
    let rect = Rect::from_min_size(pos2(left, slot.top()), slot.size());
    let response = ui.interact(rect, ui.id().with(("spicy-line", index)), Sense::click());
    theme::focus_ring(ui, &response);
    let hovered = response.hovered();
    let painter = ui.painter().with_clip_rect(ui.clip_rect());
    if hovered && !is_active {
        // Spicy's hover is a glassy rounded backdrop, not a selection bar.
        // The active line already carries the eye; keep its fill clean.
        painter.rect_filled(
            rect.expand2(vec2(width * 0.0025, width * 0.0085)),
            16.0,
            with_alpha(text_color, 26),
        );
    }
    let line_alpha = if !lyrics.synced || hovered || is_active {
        255.0
    } else if position >= line_length {
        f32::from(sung_alpha)
    } else {
        f32::from(unsung_alpha)
    };
    let line_depth = if hovered || is_active { 1.0 } else { depth };
    // Every line uses the same word slots. An inactive line is the same text
    // at the extension's 0.95 resting scale, with no fill or glow, so becoming
    // active changes colour and motion but never position.
    if !is_active {
        let mut glyphs = Vec::with_capacity(units.len());
        let idle_font = theme::bold(size * 0.95);
        for (unit_index, unit_x, unit_width, unit_row) in &unit_layout {
            let unit = &units[*unit_index];
            let unit_y = rect.top() + 2.0 + *unit_row as f32 * line_height + size * 0.5;
            push_glyph(
                &mut glyphs,
                &painter,
                &unit.text,
                idle_font.clone(),
                pos2(rect.left() + unit_x + unit_width / 2.0, unit_y),
                0,
                0.0,
                false,
                (line_alpha * line_depth) as u8,
                line_alpha as u8,
            );
        }
        paint_glyphs(&painter, &glyphs, text_color, rect.top(), ui.clip_rect());
        if lyrics.synced
            && response.clicked()
            && let Some(at_ms) = line.at_ms
        {
            app.actions.push(Action::Seek(at_ms));
            app.actions.push(Action::FollowLyrics);
        }
        return rect;
    }
    let line_fill_y = rect.top() + 2.0 + rect.height() * line_progress;
    let line_glow = if precise || !app.settings.spicy_glow {
        0.0
    } else {
        let springs = motion
            .springs
            .entry((index as u32, u32::MAX, u32::MAX))
            .or_default();
        springs.glow.goal = spline(LINE_GLOW, line_progress);
        springs
            .glow
            .step(spring_dt, LINE_GLOW_HZ, LINE_GLOW_DAMPING)
    };
    let mut glyphs = Vec::with_capacity(units.len());
    let active_font = if precise {
        font.clone()
    } else {
        // A line-synced active line takes Spicy's 1.05 scale.
        animated_font(size * 1.05)
    };
    for (unit_index, unit_x, unit_width, unit_row) in &unit_layout {
        let unit_x = rect.left() + *unit_x;
        let unit = &units[*unit_index];
        let unit_y = rect.top() + 2.0 + *unit_row as f32 * line_height + size * 0.5;
        let word_center = unit_x + unit_width / 2.0;
        if !precise {
            // A line-synced line fills from top to bottom as one slow wipe.
            let halo_alpha = (line_glow.max(0.0) * 110.0).clamp(0.0, 255.0) as u8;
            push_glyph(
                &mut glyphs,
                &painter,
                &unit.text,
                active_font.clone(),
                pos2(word_center, unit_y),
                halo_alpha,
                line_progress,
                true,
                LINE_REST_ALPHA,
                LINE_FILL_ALPHA,
            );
            continue;
        }
        let word_p = progress(position, unit.at_ms, unit.end_ms);
        let word_active = position >= unit.at_ms && position < unit.end_ms;
        if unit.letters {
            // Spicy only splits a word it holds for a second or more. Each
            // letter then carries its own fill and proximity wave.
            let letters = Lyrics::letters(unit);
            let active_letter = if word_active {
                letters
                    .iter()
                    .position(|(_, at, end)| position >= *at && position < *end)
                    .or_else(|| letters.iter().rposition(|(_, at, _)| position >= *at))
            } else {
                None
            };
            let rest_p = if position >= unit.end_ms { 1.0 } else { 0.0 };
            let rest_scale = spline(LETTER_SCALE, rest_p);
            let rest_lift = spline(LETTER_LIFT, rest_p) * size * 2.0;
            let rest_glow = spline(GLOW, rest_p);
            let mut offset = 0.0;
            let mut held_glow: Option<(f32, f32)> = None;
            for (letter_index, (character, at, end)) in letters.into_iter().enumerate() {
                let advance = measure.advance(ui, &font, size, character);
                let char_center = unit_x + offset + advance / 2.0;
                offset += advance;
                if character.is_whitespace() {
                    continue;
                }
                let (goal_scale, goal_lift, goal_glow) = if position < at {
                    // A letter that has not begun stays at rest; the wave only
                    // pulls letters the voice has already reached.
                    (rest_scale, rest_lift, rest_glow)
                } else if let Some(active) = active_letter {
                    let letter_p = progress(position, at, end);
                    let distance = letter_index.abs_diff(active) as f32;
                    let falloff = proximity(distance);
                    let glow_falloff = glow_proximity(distance);
                    (
                        rest_scale + (spline(LETTER_SCALE, letter_p) - rest_scale) * falloff,
                        rest_lift
                            + (spline(LETTER_LIFT, letter_p) * size * 2.0 - rest_lift) * falloff,
                        rest_glow + (spline(GLOW, letter_p) - rest_glow) * glow_falloff,
                    )
                } else {
                    (rest_scale, rest_lift, rest_glow)
                };
                let springs = motion
                    .springs
                    .entry((index as u32, *unit_index as u32, letter_index as u32))
                    .or_default();
                springs.scale.goal = goal_scale;
                springs.lift.goal = goal_lift;
                springs.glow.goal = goal_glow;
                let (scale, lift, glow) = (
                    springs.scale.step(spring_dt, SCALE_HZ, SCALE_DAMPING),
                    springs.lift.step(spring_dt, LIFT_HZ, LIFT_DAMPING),
                    springs.glow.step(spring_dt, GLOW_HZ, GLOW_DAMPING),
                );
                let fill = if position < at {
                    0.0
                } else if position >= end {
                    1.0
                } else {
                    fill_fraction(progress(position, at, end), true)
                };
                if Some(letter_index) == active_letter {
                    held_glow = Some((glow, lift));
                }
                push_glyph(
                    &mut glyphs,
                    &painter,
                    &character.to_string(),
                    animated_font(size * scale),
                    pos2(char_center, unit_y + lift),
                    0,
                    fill,
                    false,
                    WORD_REST_ALPHA,
                    WORD_FILL_ALPHA,
                );
            }
            // A held word glows as one word. Per-letter halos would put a
            // separate shadow behind every character.
            if let Some((glow, lift)) = held_glow
                && glow > 0.02
            {
                push_glyph(
                    &mut glyphs,
                    &painter,
                    &unit.text,
                    font.clone(),
                    pos2(word_center, unit_y + lift),
                    (glow * 140.0).clamp(0.0, 255.0) as u8,
                    0.0,
                    false,
                    0,
                    0,
                );
            }
        } else {
            // The normal path: one shaped word, one spring, one fill edge.
            let springs = motion
                .springs
                .entry((index as u32, *unit_index as u32, u32::MAX))
                .or_default();
            springs.scale.goal = spline(WORD_SCALE, word_p);
            springs.lift.goal = spline(WORD_LIFT, word_p);
            springs.glow.goal = spline(GLOW, word_p);
            let scale = springs.scale.step(spring_dt, SCALE_HZ, SCALE_DAMPING);
            let lift = springs.lift.step(spring_dt, LIFT_HZ, LIFT_DAMPING) * size;
            let glow = springs.glow.step(spring_dt, GLOW_HZ, GLOW_DAMPING);
            let fill = if position < unit.at_ms {
                0.0
            } else if position >= unit.end_ms {
                1.0
            } else {
                fill_fraction(word_p, false)
            };
            let halo_alpha = (glow.max(0.0) * 140.0).clamp(0.0, 255.0) as u8;
            let word_font = if (scale - 1.0).abs() > 0.001 {
                animated_font(size * scale)
            } else {
                font.clone()
            };
            push_glyph(
                &mut glyphs,
                &painter,
                &unit.text,
                word_font,
                pos2(word_center, unit_y + lift),
                halo_alpha,
                fill,
                false,
                WORD_REST_ALPHA,
                WORD_FILL_ALPHA,
            );
        }
    }
    paint_glyphs(&painter, &glyphs, text_color, line_fill_y, ui.clip_rect());
    if lyrics.synced
        && response.clicked()
        && let Some(at_ms) = line.at_ms
    {
        app.actions.push(Action::Seek(at_ms));
        app.actions.push(Action::FollowLyrics);
    }
    Rect::from_min_size(rect.min, vec2(max_width, (row + 1) as f32 * line_height))
}

/// The passive renderer for RTL lines, static lyrics, and anything the word
/// view cannot lay out.
#[allow(clippy::too_many_arguments)]
fn draw_plain_line(
    app: &mut App,
    ui: &mut egui::Ui,
    line: &crate::lyrics::Line,
    text: &str,
    left: f32,
    width: f32,
    size: f32,
    text_color: Color32,
    is_active: bool,
    synced: bool,
    alpha: u8,
    depth: f32,
) -> Rect {
    let font = if synced {
        theme::bold(size)
    } else {
        // Static lyrics are the one place Spicy drops to 500 weight.
        theme::medium(size)
    };
    let color = with_alpha(text_color, if synced && !is_active { alpha } else { 255 });
    let galley = crate::bidi::layout(ui.painter(), text, font, color, width, usize::MAX, None);
    let (slot, _) = ui.allocate_exact_size(vec2(width, galley.size().y + 6.0), Sense::hover());
    let rect = Rect::from_min_size(pos2(left, slot.top()), slot.size());
    let response = ui.interact(
        rect,
        ui.id().with(("spicy-plain-line", line.at_ms)),
        Sense::click(),
    );
    theme::focus_ring(ui, &response);
    let hovered = response.hovered();
    if synced && hovered && !is_active {
        ui.painter().rect_filled(
            rect.expand2(vec2(width * 0.0025, width * 0.0085)),
            16.0,
            with_alpha(text_color, 26),
        );
    }
    let alpha = if hovered || is_active || !synced {
        255
    } else {
        (f32::from(alpha) * depth).round() as u8
    };
    ui.painter().galley(
        pos2(rect.left(), rect.top() + 2.0),
        galley,
        with_alpha(text_color, alpha),
    );
    if synced
        && response.clicked()
        && let Some(at_ms) = line.at_ms
    {
        app.actions.push(Action::Seek(at_ms));
        app.actions.push(Action::FollowLyrics);
    }
    rect
}

/// How far through a unit `position` is: 0 before it starts, 1 after it ends.
fn progress(position: u32, at: u32, end: u32) -> f32 {
    if end <= at {
        return if position >= at { 1.0 } else { 0.0 };
    }
    ((position.saturating_sub(at)) as f32 / (end - at) as f32).clamp(0.0, 1.0)
}

/// How far the karaoke edge has crossed a word. Held words use the sine curve
/// from Spicy's letter animation; ordinary words use their linear progress.
fn fill_fraction(progress: f32, letters: bool) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    let eased = if letters {
        (progress * std::f32::consts::FRAC_PI_2).sin()
    } else {
        progress
    };
    (FILL_START + FILL_WIDTH * eased).clamp(0.0, 1.0)
}

/// A smooth curve through the animation points, eased between each pair so
/// the peak and the rest are not corners.
fn spline(points: &[(f32, f32)], t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if points.len() < 2 {
        return points.first().map_or(0.0, |point| point.1);
    }
    if t <= points[0].0 {
        return points[0].1;
    }
    for pair in points.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        if t <= to.0 {
            let span = (to.0 - from.0).max(f32::EPSILON);
            let u = ((t - from.0) / span).clamp(0.0, 1.0);
            let eased = u * u * (3.0 - 2.0 * u);
            return from.1 + (to.1 - from.1) * eased;
        }
    }
    points.last().map_or(0.0, |point| point.1)
}

/// Scrolls so the middle of `line` sits `at` of the way down the visible
/// lyrics.
fn sung_line_target(clip: Rect, line: Rect, at: f32) -> Rect {
    let above = (clip.height() * at - line.height() / 2.0).max(0.0);
    Rect::from_min_max(pos2(line.left(), line.top() - above), line.max)
}

fn show_sung_line(
    ui: &egui::Ui,
    line: Rect,
    animation: Option<egui::style::ScrollAnimation>,
    at: f32,
) {
    let target = sung_line_target(ui.clip_rect(), line, at);
    match animation {
        Some(animation) => ui.scroll_to_rect_animation(target, Some(Align::Min), animation),
        None => ui.scroll_to_rect(target, Some(Align::Min)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_letter_curve_swells_before_it_settles() {
        assert!((spline(LETTER_SCALE, 0.0) - 0.95).abs() < 0.001);
        assert!(spline(LETTER_SCALE, 0.7) > 1.17);
        assert!((spline(LETTER_SCALE, 1.0) - 1.0).abs() < 0.001);
        let idle = spline(LETTER_LIFT, 0.0);
        let peak = spline(LETTER_LIFT, 0.9);
        assert!(peak < idle, "the letter lifts: {peak} < {idle}");
        assert!((spline(LETTER_LIFT, 1.0)).abs() < 0.001);
        assert!(spline(GLOW, 0.2) > 0.9);
        assert!(spline(GLOW, 1.0).abs() < 0.001);
    }

    #[test]
    fn a_word_swells_less_than_a_split_letter() {
        let word = spline(WORD_SCALE, 0.7);
        let letter = spline(LETTER_SCALE, 0.7);
        assert!(word > 1.04 && word < 1.06, "{word}");
        assert!(letter > word);
    }

    #[test]
    fn only_the_active_letter_pulls_its_neighbours() {
        let active = proximity(0.0);
        let next = proximity(1.0);
        let far = proximity(3.0);
        assert!((active - 1.0).abs() < 0.001);
        assert!(next < active && next > far);
        assert!(glow_proximity(1.0) > glow_proximity(4.0));
    }

    #[test]
    fn animated_glyph_sizes_repeat_on_a_quarter_pixel_grid() {
        assert_eq!(animated_font(40.0).size, 40.0);
        assert_eq!(animated_font(40.1).size, 40.0);
        assert_eq!(animated_font(40.2).size, 40.25);
        // A pixel of continuous motion may only make a handful of sizes.
        let sizes: std::collections::HashSet<u32> = (0..100)
            .map(|step| (animated_font(40.0 + step as f32 * 0.01).size * 4.0).round() as u32)
            .collect();
        assert!(
            sizes.len() <= 5,
            "one pixel of motion made {} sizes",
            sizes.len()
        );
    }

    #[test]
    fn a_spring_overshoots_then_settles_on_its_target() {
        let mut spring = Spring::new(0.95);
        spring.goal = 1.15;
        let mut peak = 0.0f32;
        for _ in 0..240 {
            peak = peak.max(spring.step(1.0 / 60.0, SCALE_HZ, SCALE_DAMPING));
        }
        assert!(peak > 1.15, "the swell overshoots: {peak}");
        assert!((spring.value - 1.15).abs() < 0.002, "{}", spring.value);
    }

    #[test]
    fn progress_clamps_at_both_ends() {
        assert_eq!(progress(0, 1_000, 2_000), 0.0);
        assert_eq!(progress(1_500, 1_000, 2_000), 0.5);
        assert_eq!(progress(9_000, 1_000, 2_000), 1.0);
        assert_eq!(progress(1_000, 1_000, 1_000), 1.0);
    }

    #[test]
    fn the_karaoke_fill_crosses_a_word_from_left_to_right() {
        assert_eq!(fill_fraction(0.0, false), 0.0);
        assert!((fill_fraction(0.5, false) - 0.4).abs() < 0.001);
        assert_eq!(fill_fraction(1.0, false), 1.0);
        // Held words ease into their fill instead of running linearly.
        assert!(fill_fraction(0.5, true) > fill_fraction(0.5, false));
        assert_eq!(fill_fraction(1.0, true), 1.0);
    }

    #[test]
    fn the_lyrics_scale_matches_spicys_clamps() {
        assert!((lyric_size(287.0, false, true) - 33.6).abs() < 0.01);
        assert!((lyric_size(720.0, true, true) - 50.4).abs() < 0.01);
        assert!((lyric_size(287.0, false, false) - 14.35).abs() < 0.01);
        assert!((lyric_size(1200.0, true, true) - 56.0).abs() < 0.01);
    }

    #[test]
    fn line_spacing_uses_the_container_width() {
        assert!((line_gap(500.0, true, false) - 10.0).abs() < 0.001);
        assert!((line_gap(500.0, true, true) - 5.0).abs() < 0.001);
        assert!((line_gap(500.0, false, false) - 5.0).abs() < 0.001);
    }

    #[test]
    fn depth_falls_off_with_distance_then_stops() {
        assert!((line_depth(4, Some(4), false) - 1.0).abs() < 0.001);
        assert!(line_depth(5, Some(4), false) < 1.0);
        assert!(line_depth(6, Some(4), false) < line_depth(5, Some(4), false));
        // The sidebar dims further than the page at the same distance.
        assert!(line_depth(5, Some(4), false) < line_depth(5, Some(4), true));
        // Past five lines nothing falls back any further, and nothing vanishes.
        let far = line_depth(20, Some(4), false);
        assert!((line_depth(30, Some(4), false) - far).abs() < 0.001);
        assert!((far - SIDEBAR_DEPTH_FLOOR).abs() < 0.001);
        assert_eq!(line_depth(4, None, false), 1.0);
    }

    #[test]
    fn the_sung_line_is_centred_in_the_view() {
        let clip = Rect::from_min_size(pos2(0.0, 100.0), vec2(720.0, 600.0));
        let line = Rect::from_min_size(pos2(24.0, 1_000.0), vec2(680.0, 40.0));
        let target = sung_line_target(clip, line, SUNG_LINE_AT);
        assert_eq!(SUNG_LINE_AT, 0.5);
        assert_eq!(target.top(), 720.0);
        let line_centre_after_scroll =
            clip.top() + (line.top() - target.top()) + line.height() / 2.0;
        assert_eq!(line_centre_after_scroll, 400.0);
    }
}
