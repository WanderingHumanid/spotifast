//! The right-hand Now playing column, following Spotify's own view: the full
//! cover and song at the top, then the lyrics, the artist, the queue, and the
//! credits as quiet unboxed sections.
//!
//! It reuses the artist page's cached artist, so opening the column costs no
//! second metadata path. Spotify does not expose a biography, tour dates or
//! composer credits through the Web API; the column shows the metadata it does
//! have instead of inventing the rest.

use egui::{Align, Color32, Frame, Layout, Margin, Rect, Sense, pos2, vec2};

use crate::api::models::{Artist, PlayableItem, pick_image};
use crate::app::App;
use crate::i18n::{gettext, ngettext};
use crate::model::{Action, Page, RowContext};
use crate::theme::{self, Icon};
use crate::util;

use super::widgets;

const PANEL_ID: &str = "now-playing-panel";
const COVER_RADIUS: f32 = 8.0;

/// The song column beside the page. It shares the queue's remembered width,
/// since only one right-hand column can be open at a time.
pub fn side_panel(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    // Spotify's Now Playing column is wider than the bare queue list.
    let remembered = if super::lucid::enabled(app) && (app.settings.queue_width - 360.0).abs() < 1.0
    {
        416.0
    } else {
        app.settings.queue_width
    };
    let fit = super::yielding_panel(
        ui.ctx(),
        PANEL_ID,
        theme::SIDE_PANEL_MIN_WIDTH..=460.0,
        remembered,
        ui.available_width() - super::topbar::least_width(ui.ctx()),
    );
    let frame = if super::lucid::enabled(app) {
        let surface = if palette.dark {
            palette.surface
        } else {
            palette.panel
        };
        let fill = super::lucid::glass(app, surface);
        super::lucid::card(app, super::lucid::RADIUS)
            .fill(fill)
            .inner_margin(Margin::symmetric(16, 12))
            .outer_margin(Margin {
                left: 8,
                right: 8,
                top: 8,
                bottom: 8,
            })
    } else {
        Frame::new()
            .fill(palette.panel)
            .inner_margin(Margin::symmetric(16, 12))
    };
    let panel = egui::Panel::right(PANEL_ID)
        .resizable(true)
        .default_size(remembered)
        .size_range(fit.range.clone())
        .show_separator_line(false)
        .frame(frame);
    let response = panel.show(ui, |ui| {
        let window_controls = super::window_controls_reservation(
            ui.ctx(),
            app.show_queue_panel,
            app.show_now_playing_panel,
            ui.available_width(),
        );
        ui.add_space(window_controls.lyrics_top);
        header(app, ui, &palette);
        ui.add_space(12.0);
        egui::ScrollArea::vertical()
            .id_salt("now-playing-panel-scroll")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .auto_shrink([false, false])
            .show(ui, |ui| contents(app, ui, &palette));
    });
    let width = response.response.rect.width();
    if (width - app.settings.queue_width).abs() > 1.0
        && super::panel_width_chosen(ui.ctx(), PANEL_ID, &fit)
    {
        app.settings.queue_width = width;
        app.actions.push(Action::SettingsChanged);
    }
}

fn header(app: &mut App, ui: &mut egui::Ui, palette: &theme::Palette) {
    let playing = app.now_playing().is_some_and(|now| now.playing);
    ui.horizontal(|ui| {
        ui.add_space(2.0);
        theme::text(
            ui,
            gettext(app.locale, "Now playing"),
            theme::semibold(16.0),
            palette.text,
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let close = theme::icon_button(
                ui,
                Icon::X,
                14.0,
                palette.secondary,
                palette.text,
                &gettext(app.locale, "Close"),
            );
            #[cfg(feature = "demo")]
            if ui.data_mut(|data| {
                data.get_persisted::<bool>(egui::Id::new("demo-focus-close"))
                    .unwrap_or(false)
            }) {
                close.request_focus();
            }
            if close.is_pointer_button_down_on() {
                ui.painter().rect_stroke(
                    close.rect.shrink(2.0),
                    4.0,
                    egui::Stroke::new(1.5, palette.accent),
                    egui::StrokeKind::Inside,
                );
            }
            if close.clicked() {
                app.actions.push(Action::ToggleNowPlayingPanel);
            }
            if app.now_playing().is_some() {
                let (icon, label, color) = if playing {
                    (
                        Icon::AudioLines,
                        gettext(app.locale, "Playing"),
                        palette.accent,
                    )
                } else {
                    (
                        Icon::PauseFilled,
                        gettext(app.locale, "Paused"),
                        palette.secondary,
                    )
                };
                ui.add_space(4.0);
                theme::icon(ui, icon, 12.0, color);
                theme::text(ui, label, theme::medium(11.0), color);
            }
        });
    });
}

fn contents(app: &mut App, ui: &mut egui::Ui, palette: &theme::Palette) {
    let Some(now) = app.now_playing() else {
        // Centre the idle state in the column instead of leaving it pinned
        // under the header with the whole panel empty below it.
        let room = (ui.available_height() - 180.0).max(0.0);
        ui.add_space((room * 0.5).clamp(24.0, 360.0));
        widgets::empty_state(
            ui,
            palette,
            Icon::AudioLines,
            &gettext(app.locale, "Nothing playing"),
            &gettext(app.locale, "Play a song to see it here."),
        );
        return;
    };
    let title = now.title.clone();
    let subtitle = now.subtitle.clone();
    let artists = now.artists.clone();
    let album_name = now.album_name.clone();
    let album_id = now.album_id.clone();
    let art_url = now.art_url.clone().or_else(|| now.art_small.clone());
    let is_episode = now.is_episode;
    let uri = now.uri.clone();

    cover(app, ui, palette, art_url.as_deref(), album_id.as_deref());
    ui.add_space(16.0);
    song_heading(app, ui, palette, &title, &artists, &subtitle, &uri);
    if !is_episode && app.settings.spicy_now_playing {
        ui.add_space(22.0);
        super::spicy_lyrics::now_playing(app, ui, palette);
    }
    if !is_episode {
        ui.add_space(22.0);
        next_in_queue(app, ui, palette);
        if let Some(artist) = artist(app, &artists) {
            ui.add_space(24.0);
            about_artist(app, ui, palette, &artist);
        }
    }
    ui.add_space(24.0);
    credits(
        app,
        ui,
        palette,
        Credits {
            title: &title,
            subtitle: &subtitle,
            artists: &artists,
            album: &album_name,
            is_episode,
        },
    );
    ui.add_space(18.0);
}

fn cover(
    app: &mut App,
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    art_url: Option<&str>,
    album_id: Option<&str>,
) {
    let side = ui.available_width().min(400.0);
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), Sense::click());
    ui.painter().add(
        egui::epaint::Shadow {
            offset: [0, 10],
            blur: 28,
            spread: 0,
            color: Color32::from_black_alpha(80),
        }
        .as_shape(rect, COVER_RADIUS),
    );
    widgets::paint_cover(
        ui,
        palette,
        art_url,
        rect,
        COVER_RADIUS,
        Icon::Music,
        Some(app.backend.art()),
    );
    theme::focus_ring(ui, &response);
    if response.hovered() || response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            COVER_RADIUS,
            egui::Stroke::new(1.0, palette.accent),
            egui::StrokeKind::Inside,
        );
    }
    if response.is_pointer_button_down_on() {
        ui.painter()
            .rect_filled(rect, COVER_RADIUS, Color32::from_black_alpha(24));
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            COVER_RADIUS,
            egui::Stroke::new(2.0, palette.accent),
            egui::StrokeKind::Inside,
        );
    }
    if response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
        && let Some(id) = album_id
    {
        app.actions.push(Action::Open(Page::Album(id.to_string())));
    }
}

fn song_heading(
    app: &mut App,
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    title: &str,
    artists: &[crate::api::models::ArtistRef],
    subtitle: &str,
    uri: &str,
) {
    let is_episode = app.now_playing().is_some_and(|now| now.is_episode);
    let album_id = app.now_playing().and_then(|now| now.album_id.clone());
    let show_id = app.now_playing().and_then(|now| now.show_id.clone());
    let title_response = ui.add(
        egui::Label::new(
            egui::RichText::new(title)
                .font(theme::bold(24.0))
                .color(palette.text),
        )
        .wrap()
        .sense(Sense::click()),
    );
    theme::focus_ring(ui, &title_response);
    if title_response.hovered() || title_response.is_pointer_button_down_on() {
        let active = title_response.is_pointer_button_down_on();
        ui.painter().hline(
            title_response.rect.x_range(),
            title_response.rect.bottom() - 1.0,
            egui::Stroke::new(if active { 2.0 } else { 1.0 }, palette.text),
        );
    }
    if title_response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        if is_episode {
            if let Some(id) = show_id {
                app.actions.push(Action::Open(Page::Show(id)));
            }
        } else if let Some(id) = album_id {
            app.actions.push(Action::Open(Page::Album(id)));
        }
    }
    ui.add_space(2.0);
    if artists.is_empty() {
        theme::text(ui, subtitle, theme::regular(14.0), palette.secondary);
    } else {
        ui.horizontal_wrapped(|ui| {
            widgets::artist_links(ui, app, artists, theme::regular(14.0), palette.secondary);
        });
    }
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let saved = app.is_saved(uri).unwrap_or(false);
        let (icon, tooltip) = if saved {
            (
                Icon::HeartFilled,
                gettext(app.locale, "Remove from Liked Songs"),
            )
        } else {
            (Icon::Heart, gettext(app.locale, "Save to Liked Songs"))
        };
        if theme::icon_button(
            ui,
            icon,
            24.0,
            if saved {
                palette.accent
            } else {
                palette.secondary
            },
            palette.text,
            &tooltip,
        )
        .clicked()
        {
            app.actions.push(Action::ToggleSaved(uri.to_string()));
        }
        if !is_episode
            && theme::icon_button(
                ui,
                Icon::ListEnd,
                24.0,
                palette.secondary,
                palette.text,
                &gettext(app.locale, "Add to queue"),
            )
            .clicked()
        {
            app.actions.push(Action::AddToQueue {
                uri: uri.to_string(),
                label: title.to_string(),
            });
        }
    });
}

fn artist(app: &App, artists: &[crate::api::models::ArtistRef]) -> Option<Artist> {
    let id = artists.iter().find_map(|artist| artist.id.as_deref())?;
    app.artist_pages.get(id)?.artist.get().cloned()
}

fn about_artist(app: &mut App, ui: &mut egui::Ui, palette: &theme::Palette, artist: &Artist) {
    theme::section_title(ui, palette, &gettext(app.locale, "About the artist"));
    ui.add_space(8.0);
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), 160.0), Sense::click());
    widgets::paint_cover(
        ui,
        palette,
        pick_image(&artist.images, app.art_px(rect.height())),
        rect,
        8.0,
        Icon::Users,
        Some(app.backend.art()),
    );
    theme::focus_ring(ui, &response);
    if response.hovered() || response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            8.0,
            egui::Stroke::new(1.0, palette.accent),
            egui::StrokeKind::Inside,
        );
    }
    if response.is_pointer_button_down_on() {
        ui.painter()
            .rect_filled(rect, 8.0, Color32::from_black_alpha(24));
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            8.0,
            egui::Stroke::new(2.0, palette.accent),
            egui::StrokeKind::Inside,
        );
    }
    if response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        app.actions
            .push(Action::Open(Page::Artist(artist.id.clone())));
    }
    ui.add_space(10.0);
    if theme::link(ui, &artist.name, theme::bold(18.0), palette.text).clicked() {
        app.actions
            .push(Action::Open(Page::Artist(artist.id.clone())));
    }
    let mut meta = Vec::new();
    if let Some(followers) = &artist.followers {
        meta.push(
            ngettext(
                app.locale,
                "{count} follower",
                "{count} followers",
                u32::try_from(followers.total).unwrap_or(u32::MAX),
            )
            .replace("{count}", &util::format_count(followers.total)),
        );
    }
    if !artist.genres.is_empty() {
        meta.push(
            artist
                .genres
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if !meta.is_empty() {
        ui.add_space(2.0);
        theme::subtle(ui, palette, &meta.join(" · "));
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let following = app.is_saved(&artist.uri).unwrap_or(false);
        if theme::pill_button(
            ui,
            palette,
            &if following {
                gettext(app.locale, "Following")
            } else {
                gettext(app.locale, "Follow")
            },
            false,
        )
        .clicked()
        {
            app.actions.push(Action::ToggleSaved(artist.uri.clone()));
        }
        if theme::soft_button(
            ui,
            palette,
            None,
            &gettext(app.locale, "View artist"),
            false,
        )
        .clicked()
        {
            app.actions
                .push(Action::Open(Page::Artist(artist.id.clone())));
        }
    });
}

fn next_in_queue(app: &mut App, ui: &mut egui::Ui, palette: &theme::Palette) {
    let items: Vec<PlayableItem> = app
        .queue
        .get()
        .map(|queue| queue.queue.iter().take(3).cloned().collect())
        .unwrap_or_default();
    if items.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        theme::section_title(ui, palette, &gettext(app.locale, "Next in queue"));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::icon_button(
                ui,
                Icon::ListMusic,
                17.0,
                palette.secondary,
                palette.text,
                &gettext(app.locale, "Queue"),
            )
            .clicked()
            {
                app.actions.push(Action::ToggleQueuePanel);
            }
        });
    });
    ui.add_space(5.0);
    for (index, item) in items.iter().enumerate() {
        queue_row(app, ui, palette, index, item);
    }
}

fn queue_row(
    app: &mut App,
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    index: usize,
    item: &PlayableItem,
) {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::click());
    theme::focus_ring(ui, &response);
    if response.hovered() || response.is_pointer_button_down_on() {
        ui.painter().rect_filled(
            rect,
            6.0,
            if response.is_pointer_button_down_on() {
                palette.surface_active
            } else {
                palette.surface_hover
            },
        );
    }
    if response.has_focus() || response.is_pointer_button_down_on() {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            6.0,
            egui::Stroke::new(
                if response.is_pointer_button_down_on() {
                    2.0
                } else {
                    1.0
                },
                palette.accent,
            ),
            egui::StrokeKind::Inside,
        );
    }
    let cover = Rect::from_min_size(rect.min + vec2(4.0, 6.0), vec2(40.0, 40.0));
    widgets::paint_cover(
        ui,
        palette,
        item.image(app.art_px(40.0)),
        cover,
        4.0,
        Icon::Music,
        Some(app.backend.art()),
    );
    let left = cover.right() + 10.0;
    let duration = util::format_duration_ms(item.duration_ms());
    let duration = ui
        .painter()
        .layout_no_wrap(duration, theme::regular(12.0), palette.secondary);
    let width = (rect.right() - left - duration.size().x - 14.0).max(20.0);
    ui.painter().galley(
        pos2(rect.right() - duration.size().x - 4.0, rect.top() + 19.0),
        duration,
        palette.secondary,
    );
    let title = widgets::ellipsized(ui, item.name(), theme::medium(14.0), palette.text, width, 1);
    ui.painter()
        .galley(pos2(left, rect.top() + 8.0), title, palette.text);
    let subtitle = widgets::ellipsized(
        ui,
        &item.subtitle(),
        theme::regular(12.0),
        palette.secondary,
        width,
        1,
    );
    ui.painter()
        .galley(pos2(left, rect.top() + 27.0), subtitle, palette.secondary);
    if response.clicked() {
        app.actions.push(Action::PlayFromRow {
            context: RowContext::Queue,
            uri: item.uri().to_string(),
            index: index as u32,
        });
    }
}

struct Credits<'a> {
    title: &'a str,
    subtitle: &'a str,
    artists: &'a [crate::api::models::ArtistRef],
    album: &'a str,
    is_episode: bool,
}

fn credits(app: &mut App, ui: &mut egui::Ui, palette: &theme::Palette, credits: Credits<'_>) {
    theme::section_title(ui, palette, &gettext(app.locale, "Credits"));
    ui.add_space(8.0);
    let names = credits
        .artists
        .iter()
        .map(|artist| artist.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if !names.is_empty() {
        credit_row(ui, palette, &gettext(app.locale, "Performed by"), &names);
    }
    if credits.is_episode {
        credit_row(
            ui,
            palette,
            &gettext(app.locale, "Podcast"),
            credits.subtitle,
        );
    } else if !credits.album.is_empty() {
        credit_row(ui, palette, &gettext(app.locale, "Album"), credits.album);
    }
    credit_row(ui, palette, &gettext(app.locale, "Song"), credits.title);
}

fn credit_row(ui: &mut egui::Ui, palette: &theme::Palette, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        theme::text(ui, label, theme::regular(12.0), palette.secondary);
        theme::text(ui, value, theme::regular(13.0), palette.text);
    });
    ui.add_space(6.0);
}
