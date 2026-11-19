//! The Search page.

use std::sync::Arc;

use egui::{
    Align, Color32, CornerRadius, Layout, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2,
};

use crate::api::models::{Artist, ArtistRef, PlayableItem, SearchResults, pick_image};
use crate::app::App;
use crate::i18n::gettext;
use crate::model::{Action, Loadable, Page, RowContext, SearchFilter};
use crate::theme::{self, Icon};

use super::widgets::{self, TrackRow};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    header(app, ui);
    if app.search.query.trim().is_empty() {
        recent(app, ui);
        return;
    }
    let labels: Vec<_> = SearchFilter::ALL
        .iter()
        .map(|f| (*f, f.label(app.locale)))
        .collect();
    let options: Vec<(SearchFilter, &str)> = labels
        .iter()
        .map(|(filter, label)| (*filter, label.as_ref()))
        .collect();
    if let Some(filter) = widgets::chips(ui, &palette, &options, app.search.filter) {
        app.actions.push(Action::SetSearchFilter(filter));
    }
    ui.add_space(12.0);
    let pending = app.search.catalogue_pending || app.search.playlists_pending;
    if pending {
        widgets::loading_row(ui, &palette, app.locale);
    }
    if let Some(error) = app.search.error.clone() {
        widgets::error_row(ui, app, &error, None);
    }
    let results = match &app.search.results {
        Loadable::Loaded(results) => results.clone(),
        Loadable::Loading | Loadable::NotLoaded => {
            if !pending {
                widgets::skeleton_rows(ui, &palette, app.locale, 6);
            }
            return;
        }
        Loadable::Failed(error) => {
            let error = error.clone();
            if app.search.error.is_none() {
                widgets::error_row(ui, app, &error, None);
            }
            return;
        }
    };
    if results.is_empty() && (pending || app.search.error.is_some()) {
        return;
    }
    if results.is_empty() {
        widgets::empty_state(
            ui,
            &palette,
            Icon::Search,
            // Translators: {query} is the text the user searched for.
            &gettext(app.locale, "No results for “{query}”")
                .replace("{query}", &app.search.committed),
            &gettext(app.locale, "Check the spelling, or try fewer words."),
        );
        return;
    }
    match app.search.filter {
        SearchFilter::All => all(app, ui, &results),
        SearchFilter::Songs => songs(app, ui, &results, usize::MAX),
        SearchFilter::Artists => artists_grid(app, ui, &results),
        SearchFilter::Albums => albums_grid(app, ui, &results),
        SearchFilter::Playlists => playlists_grid(app, ui, &results),
        SearchFilter::Podcasts => shows_grid(app, ui, &results),
        SearchFilter::Episodes => episodes(app, ui, &results, usize::MAX),
    }
}

/// The Search page's own heading and field, as Spotify's page has above its
/// results. It edits the same query as the top bar's field.
fn header(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    ui.add_space(8.0);
    theme::text(
        ui,
        gettext(app.locale, "Search").as_ref(),
        theme::bold(30.0),
        palette.text,
    );
    ui.add_space(12.0);
    let id = egui::Id::new("search-page");
    let before = app.search.query.clone();
    let selected = egui::TextEdit::load_state(ui.ctx(), id)
        .and_then(|state| state.cursor.char_range())
        .is_some_and(|range| !range.is_empty());
    let width = ui.available_width().min(520.0);
    let response = widgets::search_field_sized(
        ui,
        &palette,
        app.locale,
        id,
        &mut app.search.query,
        &gettext(app.locale, "What do you want to play?"),
        width,
        46.0,
    );
    if app.search.query != before {
        // Typing over a selection, or emptying the field, starts a new query
        // rather than editing the last one.
        if selected || app.search.query.is_empty() {
            app.actions.push(Action::EndSearchRun);
        }
        app.search.typed_at = Some(std::time::Instant::now());
    }
    if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
        let query = app.search.query.clone();
        app.actions.push(Action::Search(query));
    }
    ui.add_space(12.0);
}

fn recent(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    if !app.network_reachable && app.settings.search_history.is_empty() {
        widgets::empty_state(
            ui,
            &palette,
            Icon::WifiOff,
            &gettext(app.locale, "You're offline"),
            &gettext(
                app.locale,
                "Connect to the internet to search songs, artists, and playlists.",
            ),
        );
        return;
    }
    if !app.settings.search_history.is_empty() {
        theme::section_title(ui, &palette, &gettext(app.locale, "Recent searches"));
        ui.add_space(6.0);
        let history = app.settings.search_history.clone();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            for query in &history {
                let (response, forget) =
                    theme::soft_button_dismiss(ui, &palette, Icon::Clock, query);
                if forget {
                    app.actions.push(Action::ForgetSearch(query.clone()));
                } else if response.clicked() {
                    app.actions.push(Action::Search(query.clone()));
                }
            }
        });
        ui.add_space(22.0);
    }
    browse(app, ui);
}

/// A section heading at the size Spotify gives its search and browse titles.
fn heading(ui: &mut egui::Ui, palette: &theme::Palette, text: &str) {
    theme::text(ui, text, theme::bold(24.0), palette.text);
}

/// Spotify's browse grid: colour blocks that open a search for a genre or
/// mood, each with a tilted cover in the corner.
fn browse(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let locale = app.locale;
    heading(ui, &palette, &gettext(locale, "Browse all"));
    ui.add_space(12.0);
    // The tiles are search seeds rather than interface wording: each one runs
    // the query Spotify's catalogue matches for that genre or mood, so the
    // names stay as those queries.
    let labels: [&str; 15] = [
        "Pop",
        "Hip-Hop",
        "Rock",
        "Indie",
        "Electronic",
        "R&B",
        "Jazz",
        "Classical",
        "Metal",
        "Chill",
        "Focus",
        "Workout",
        "Latin",
        "Country",
        "Party",
    ];
    let colours = [
        Color32::from_rgb(0x8D, 0x67, 0xAB),
        Color32::from_rgb(0xBA, 0x5D, 0x07),
        Color32::from_rgb(0xE1, 0x33, 0x00),
        Color32::from_rgb(0x60, 0x81, 0x08),
        Color32::from_rgb(0xDC, 0x14, 0x8C),
        Color32::from_rgb(0x8C, 0x19, 0x32),
        Color32::from_rgb(0x47, 0x7D, 0x95),
        Color32::from_rgb(0x7D, 0x4B, 0x32),
        Color32::from_rgb(0xE8, 0x11, 0x5B),
        Color32::from_rgb(0x1E, 0x32, 0x64),
        Color32::from_rgb(0x50, 0x37, 0x50),
        Color32::from_rgb(0x77, 0x77, 0x77),
        Color32::from_rgb(0xE1, 0x11, 0x8C),
        Color32::from_rgb(0xD8, 0x40, 0x00),
        Color32::from_rgb(0xAF, 0x28, 0x96),
    ];
    let available = ui.available_width();
    let gap = 12.0;
    let columns = ((available / 210.0).floor() as usize).clamp(2, 6);
    let tile_width = (available - gap * (columns as f32 - 1.0)) / columns as f32;
    let tile_height = (tile_width * 0.56).clamp(82.0, 130.0);
    let rows = labels.len().div_ceil(columns);
    for row in 0..rows {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for column in 0..columns {
                let index = row * columns + column;
                let Some(label) = labels.get(index) else {
                    break;
                };
                let colour = colours[index];
                let (rect, response) =
                    ui.allocate_exact_size(vec2(tile_width, tile_height), Sense::click());
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, *label)
                });
                if ui.is_rect_visible(rect) {
                    let fill = if ui.rect_contains_pointer(rect) {
                        super::blend(colour, Color32::WHITE, 0.10)
                    } else {
                        colour
                    };
                    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
                    painter.rect_filled(rect, CornerRadius::same(10), fill);
                    painter.rect_stroke(
                        rect,
                        CornerRadius::same(10),
                        Stroke::new(1.0, Color32::from_white_alpha(20)),
                        StrokeKind::Inside,
                    );
                    // A tilted cover hangs off the corner, as Spotify's
                    // browse cards carry one.
                    let size = tile_height * 0.82;
                    let centre = pos2(rect.right() - size * 0.18, rect.bottom() + size * 0.16);
                    let angle = -0.4_f32;
                    let axis = vec2(angle.cos(), angle.sin()) * (size * 0.5);
                    let perp = vec2(-angle.sin(), angle.cos()) * (size * 0.5);
                    painter.add(egui::Shape::convex_polygon(
                        vec![
                            centre - axis - perp,
                            centre + axis - perp,
                            centre + axis + perp,
                            centre - axis + perp,
                        ],
                        Color32::from_white_alpha(22),
                        Stroke::new(1.0, Color32::from_white_alpha(30)),
                    ));
                    let text = widgets::ellipsized(
                        ui,
                        label,
                        theme::bold(20.0),
                        Color32::WHITE,
                        tile_width - 28.0,
                        1,
                    );
                    painter.galley(
                        pos2(rect.left() + 14.0, rect.top() + 12.0),
                        text,
                        Color32::WHITE,
                    );
                }
                if response.clicked() {
                    app.actions.push(Action::Search((*label).to_string()));
                }
            }
        });
        ui.add_space(gap);
    }
}

fn all(app: &mut App, ui: &mut egui::Ui, results: &SearchResults) {
    let palette = app.palette;
    let locale = app.locale;
    let query = app.search.committed.to_lowercase();
    let top_artist = results
        .artists
        .as_ref()
        .and_then(|page| page.items.first())
        .filter(|artist| {
            artist.name.to_lowercase() == query
                || results.tracks.as_ref().is_none_or(|t| t.items.is_empty())
        });
    let top_art = app.art_px(TOP_RESULT_ART);
    let wide = ui.available_width() > 720.0;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 24.0;
        let top_width = if wide {
            (ui.available_width() * 0.36).clamp(240.0, 380.0)
        } else {
            ui.available_width()
        };
        ui.vertical(|ui| {
            ui.set_width(top_width);
            heading(ui, &palette, &gettext(locale, "Top result"));
            ui.add_space(4.0);
            if let Some(artist) = top_artist {
                top_result(
                    app,
                    ui,
                    pick_image(&artist.images, top_art),
                    &artist.name,
                    TopResultSubtitle::Text(&gettext(locale, "Artist")),
                    true,
                    Some(artist.uri.clone()),
                    Page::Artist(artist.id.clone()),
                    |ui, app| {
                        widgets::context_menu_items(ui, app, &artist.uri, &artist.name, None);
                    },
                );
            } else if let Some(track) = results.tracks.as_ref().and_then(|page| page.items.first())
            {
                let page = track
                    .album
                    .as_ref()
                    .map(|album| Page::Album(album.id.clone()))
                    .unwrap_or(Page::Search);
                top_result(
                    app,
                    ui,
                    track.image(top_art),
                    &track.name,
                    TopResultSubtitle::SongArtists(&track.artists),
                    false,
                    Some(track.uri.clone()),
                    page,
                    |ui, app| {
                        widgets::item_menu(
                            ui,
                            app,
                            &PlayableItem::Track(track.clone()),
                            None,
                            None,
                        );
                    },
                );
            } else if let Some(album) = results.albums.as_ref().and_then(|page| page.items.first())
            {
                top_result(
                    app,
                    ui,
                    pick_image(&album.images, top_art),
                    &album.name,
                    TopResultSubtitle::Text(
                        // Translators: {artists} is the album's artist names.
                        &gettext(locale, "Album • {artists}").replace(
                            "{artists}",
                            &crate::api::models::join_names(
                                album.artists.iter().map(|a| a.name.as_str()),
                            ),
                        ),
                    ),
                    false,
                    Some(album.uri.clone()),
                    Page::Album(album.id.clone()),
                    |ui, app| {
                        widgets::context_menu_items(ui, app, &album.uri, &album.name, None);
                    },
                );
            } else if let Some(playlist) = results
                .playlists
                .as_ref()
                .and_then(|page| page.items.first())
            {
                top_result(
                    app,
                    ui,
                    pick_image(&playlist.images, top_art),
                    &playlist.name,
                    TopResultSubtitle::Text(
                        // Translators: {owner} is the name of the playlist's owner.
                        &gettext(locale, "Playlist • {owner}")
                            .replace("{owner}", playlist.owner_name()),
                    ),
                    false,
                    Some(playlist.uri.clone()),
                    Page::Playlist(playlist.id.clone()),
                    |ui, app| {
                        let owned = app.user_id().is_some_and(|id| playlist.owned_by(id));
                        widgets::context_menu_items(
                            ui,
                            app,
                            &playlist.uri,
                            &playlist.name,
                            owned.then_some(playlist),
                        );
                    },
                );
            } else if let Some(show) = results.shows.as_ref().and_then(|page| page.items.first()) {
                top_result(
                    app,
                    ui,
                    pick_image(&show.images, top_art),
                    &show.name,
                    TopResultSubtitle::Text(
                        // Translators: {publisher} is the podcast's publisher.
                        &gettext(locale, "Podcast • {publisher}")
                            .replace("{publisher}", &show.publisher),
                    ),
                    false,
                    Some(show.uri.clone()),
                    Page::Show(show.id.clone()),
                    |ui, app| {
                        widgets::context_menu_items(ui, app, &show.uri, &show.name, None);
                    },
                );
            }
        });
        if wide {
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                songs(app, ui, results, 4);
            });
        }
    });
    if !wide {
        ui.add_space(12.0);
        songs(app, ui, results, 4);
    }
    ui.add_space(8.0);
    shelf_artists(app, ui, results);
    shelf_albums(app, ui, results);
    shelf_playlists(app, ui, results);
    shelf_shows(app, ui, results);
    if results
        .episodes
        .as_ref()
        .is_some_and(|page| !page.items.is_empty())
    {
        heading(ui, &palette, &gettext(locale, "Episodes"));
        ui.add_space(4.0);
        episodes(app, ui, results, 4);
    }
}

enum TopResultSubtitle<'a> {
    Text(&'a str),
    SongArtists(&'a [ArtistRef]),
}

/// The top result's cover size.
const TOP_RESULT_ART: f32 = 96.0;

#[allow(clippy::too_many_arguments)]
fn top_result(
    app: &mut App,
    ui: &mut egui::Ui,
    image: Option<&str>,
    title: &str,
    subtitle: TopResultSubtitle<'_>,
    round: bool,
    play_uri: Option<String>,
    page: Page,
    menu: impl FnOnce(&mut egui::Ui, &mut App),
) {
    let palette = app.palette;
    let mut subtitle_clicked = false;
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), 232.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = ui.rect_contains_pointer(rect);
        let fill = if hovered {
            palette.surface_hover
        } else {
            palette.surface
        };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(theme::RADIUS), fill);
        let image_rect =
            Rect::from_min_size(rect.min + vec2(20.0, 20.0), Vec2::splat(TOP_RESULT_ART));
        widgets::paint_shadow(ui, &palette, image_rect, if round { 48.0 } else { 6.0 });
        widgets::paint_cover(
            ui,
            &palette,
            image,
            image_rect,
            if round { 48.0 } else { 6.0 },
            if round { Icon::User } else { Icon::Music },
            Some(app.backend.art()),
        );
        let text_clip = Rect::from_min_max(
            pos2(rect.left() + 20.0, image_rect.bottom() + 12.0),
            pos2(rect.right() - 20.0, rect.bottom()),
        );
        let painter = ui.painter().with_clip_rect(text_clip);
        crate::bidi::paint_line(
            &painter,
            text_clip.left(),
            text_clip.right(),
            text_clip.top() + 16.0,
            title,
            theme::bold(26.0),
            palette.text,
        );
        match subtitle {
            TopResultSubtitle::SongArtists(artists) => {
                let subtitle_rect = Rect::from_min_max(
                    pos2(text_clip.left(), text_clip.top() + 36.0),
                    pos2(text_clip.right(), text_clip.top() + 56.0),
                );
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(subtitle_rect)
                        .layout(Layout::left_to_right(Align::Center)),
                );
                child.set_clip_rect(subtitle_rect.intersect(ui.clip_rect()));
                child.spacing_mut().item_spacing.x = 0.0;
                theme::text(
                    &mut child,
                    // Translators: Precedes the song's artist names, which follow as links.
                    gettext(app.locale, "Song • ").as_ref(),
                    theme::regular(13.5),
                    palette.secondary,
                );
                subtitle_clicked = widgets::artist_links(
                    &mut child,
                    app,
                    artists,
                    theme::regular(13.5),
                    palette.secondary,
                );
            }
            TopResultSubtitle::Text(subtitle) => {
                crate::bidi::paint_line(
                    &painter,
                    text_clip.left(),
                    text_clip.right(),
                    text_clip.top() + 46.0,
                    subtitle,
                    theme::regular(13.5),
                    palette.secondary,
                );
            }
        }
        if hovered && let Some(uri) = &play_uri {
            let button = Rect::from_center_size(
                pos2(rect.right() - 44.0, rect.bottom() - 44.0),
                Vec2::splat(48.0),
            );
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(button)
                    .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
            );
            if theme::circle_button(
                &mut child,
                Icon::PlayFilled,
                48.0,
                palette.accent,
                palette.accent_hover,
                palette.on_accent,
                &gettext(app.locale, "Play"),
            )
            .clicked()
            {
                if uri.starts_with("spotify:track:") {
                    app.actions.push(Action::PlayUris {
                        uris: vec![uri.clone()],
                        index: 0,
                    });
                } else {
                    app.actions.push(Action::PlayContext {
                        uri: uri.clone(),
                        offset_uri: None,
                        offset_index: None,
                    });
                }
            }
        }
    }
    if response.clicked() && !subtitle_clicked && page != Page::Search {
        app.actions.push(Action::Open(page));
    }
    egui::Popup::context_menu(&response)
        .frame(widgets::menu_frame(&palette))
        .show(|ui| menu(ui, app));
}

fn songs(app: &mut App, ui: &mut egui::Ui, results: &SearchResults, limit: usize) {
    let palette = app.palette;
    let Some(page) = &results.tracks else {
        return;
    };
    if page.items.is_empty() {
        return;
    }
    heading(ui, &palette, &gettext(app.locale, "Songs"));
    ui.add_space(4.0);
    let items: Vec<PlayableItem> = page
        .items
        .iter()
        .cloned()
        .map(PlayableItem::Track)
        .collect();
    for (index, item) in items.iter().take(limit).enumerate() {
        // A search result plays on its own, and Spotify continues with what
        // follows it (autoplay), as the Spotify apps do; the other results are
        // not a queue. A one-song list loads as that song's own context.
        let context = RowContext::Uris(Arc::from([item.uri().to_string()]));
        widgets::track_row(
            ui,
            app,
            TrackRow {
                index,
                number: None,
                item,
                context: &context,
                show_cover: true,
                show_album: limit == usize::MAX,
                added_at: None,
                added_by: None,
                show_added_by: false,
                show_plays: false,
                compact: limit != usize::MAX,
                thin: false,
                shift: 0.0,
                picked: false,
                picked_songs: &[],
                picked_positions: &[],
                position: None,
            },
        );
    }
}

fn artist_card(app: &mut App, ui: &mut egui::Ui, artist: &Artist) {
    let context_here = app.playing_context_uri().as_deref() == Some(artist.uri.as_str());
    let playing_here = context_here && app.believed_playing();
    let card_art = app.art_px(widgets::CARD_ART);
    let card = widgets::card(
        ui,
        app,
        pick_image(&artist.images, card_art),
        &artist.name,
        &gettext(app.locale, "Artist"),
        widgets::CardCover::portrait(playing_here),
    );
    if card.play {
        if context_here {
            app.actions.push(Action::TogglePlay);
        } else {
            app.actions.push(Action::PlayContext {
                uri: artist.uri.clone(),
                offset_uri: None,
                offset_index: None,
            });
        }
    }
    if card.clicked {
        app.actions
            .push(Action::Open(Page::Artist(artist.id.clone())));
    }
    egui::Popup::context_menu(&card.response)
        .id(ui.make_persistent_id(("search-artist-menu", &artist.uri)))
        .frame(widgets::menu_frame(&app.palette))
        .show(|ui| {
            widgets::context_menu_items(ui, app, &artist.uri, &artist.name, None);
        });
}

fn shelf_artists(app: &mut App, ui: &mut egui::Ui, results: &SearchResults) {
    let palette = app.palette;
    let Some(page) = &results.artists else { return };
    if page.items.is_empty() {
        return;
    }
    widgets::large_shelf(
        ui,
        &palette,
        "search-artists",
        &gettext(app.locale, "Artists"),
        |ui| {
            for artist in &page.items {
                artist_card(app, ui, artist);
            }
        },
    );
}

fn artists_grid(app: &mut App, ui: &mut egui::Ui, results: &SearchResults) {
    let Some(page) = &results.artists else { return };
    widgets::grid(ui, |ui| {
        for artist in &page.items {
            artist_card(app, ui, artist);
        }
    });
}

fn album_card(app: &mut App, ui: &mut egui::Ui, album: &crate::api::models::Album) {
    let subtitle = format!(
        "{} • {}",
        album.year().unwrap_or(""),
        crate::api::models::join_names(album.artists.iter().map(|a| a.name.as_str()))
    );
    let context_here = app.playing_context_uri().as_deref() == Some(album.uri.as_str());
    let playing_here = context_here && app.believed_playing();
    let card_art = app.art_px(widgets::CARD_ART);
    let card = widgets::card(
        ui,
        app,
        pick_image(&album.images, card_art),
        &album.name,
        subtitle.trim_start_matches(" • "),
        widgets::CardCover::square(playing_here),
    );
    if card.play {
        if context_here {
            app.actions.push(Action::TogglePlay);
        } else {
            app.actions.push(Action::PlayContext {
                uri: album.uri.clone(),
                offset_uri: None,
                offset_index: None,
            });
        }
    }
    if card.clicked {
        app.actions
            .push(Action::Open(Page::Album(album.id.clone())));
    }
    egui::Popup::context_menu(&card.response)
        .id(ui.make_persistent_id(("search-album-menu", &album.uri)))
        .frame(widgets::menu_frame(&app.palette))
        .show(|ui| {
            widgets::context_menu_items(ui, app, &album.uri, &album.name, None);
        });
}

fn shelf_albums(app: &mut App, ui: &mut egui::Ui, results: &SearchResults) {
    let palette = app.palette;
    let Some(page) = &results.albums else { return };
    if page.items.is_empty() {
        return;
    }
    widgets::large_shelf(
        ui,
        &palette,
        "search-albums",
        &gettext(app.locale, "Albums"),
        |ui| {
            for album in &page.items {
                album_card(app, ui, album);
            }
        },
    );
}

fn albums_grid(app: &mut App, ui: &mut egui::Ui, results: &SearchResults) {
    let Some(page) = &results.albums else { return };
    widgets::grid(ui, |ui| {
        for album in &page.items {
            album_card(app, ui, album);
        }
    });
}

fn playlist_card(app: &mut App, ui: &mut egui::Ui, playlist: &crate::api::models::Playlist) {
    let context_here = app.playing_context_uri().as_deref() == Some(playlist.uri.as_str());
    let playing_here = context_here && app.believed_playing();
    let card_art = app.art_px(widgets::CARD_ART);
    let card = widgets::card(
        ui,
        app,
        pick_image(&playlist.images, card_art),
        &playlist.name,
        // Translators: {owner} is the name of the playlist's owner.
        &gettext(app.locale, "By {owner}").replace("{owner}", playlist.owner_name()),
        widgets::CardCover::square(playing_here),
    );
    if card.play {
        if context_here {
            app.actions.push(Action::TogglePlay);
        } else {
            app.actions.push(Action::PlayContext {
                uri: playlist.uri.clone(),
                offset_uri: None,
                offset_index: None,
            });
        }
    }
    if card.clicked {
        app.actions
            .push(Action::Open(Page::Playlist(playlist.id.clone())));
    }
    egui::Popup::context_menu(&card.response)
        .id(ui.make_persistent_id(("search-playlist-menu", &playlist.uri)))
        .frame(widgets::menu_frame(&app.palette))
        .show(|ui| {
            let owned = app.user_id().is_some_and(|id| playlist.owned_by(id));
            widgets::context_menu_items(
                ui,
                app,
                &playlist.uri,
                &playlist.name,
                owned.then_some(playlist),
            );
        });
}

fn shelf_playlists(app: &mut App, ui: &mut egui::Ui, results: &SearchResults) {
    let palette = app.palette;
    let Some(page) = &results.playlists else {
        return;
    };
    if page.items.is_empty() {
        return;
    }
    widgets::large_shelf(
        ui,
        &palette,
        "search-playlists",
        &gettext(app.locale, "Playlists"),
        |ui| {
            for playlist in &page.items {
                playlist_card(app, ui, playlist);
            }
        },
    );
}

fn playlists_grid(app: &mut App, ui: &mut egui::Ui, results: &SearchResults) {
    let Some(page) = &results.playlists else {
        return;
    };
    widgets::grid(ui, |ui| {
        for playlist in &page.items {
            playlist_card(app, ui, playlist);
        }
    });
}

fn show_card(app: &mut App, ui: &mut egui::Ui, show: &crate::api::models::Show) {
    let card_art = app.art_px(widgets::CARD_ART);
    let card = widgets::card(
        ui,
        app,
        pick_image(&show.images, card_art),
        &show.name,
        &show.publisher,
        widgets::CardCover::default(),
    );
    if card.clicked {
        app.actions.push(Action::Open(Page::Show(show.id.clone())));
    }
    egui::Popup::context_menu(&card.response)
        .id(ui.make_persistent_id(("search-show-menu", &show.uri)))
        .frame(widgets::menu_frame(&app.palette))
        .show(|ui| {
            widgets::context_menu_items(ui, app, &show.uri, &show.name, None);
        });
}

fn shelf_shows(app: &mut App, ui: &mut egui::Ui, results: &SearchResults) {
    let palette = app.palette;
    let Some(page) = &results.shows else { return };
    if page.items.is_empty() {
        return;
    }
    widgets::large_shelf(
        ui,
        &palette,
        "search-shows",
        &gettext(app.locale, "Podcasts"),
        |ui| {
            for show in &page.items {
                show_card(app, ui, show);
            }
        },
    );
}

fn shows_grid(app: &mut App, ui: &mut egui::Ui, results: &SearchResults) {
    let Some(page) = &results.shows else { return };
    widgets::grid(ui, |ui| {
        for show in &page.items {
            show_card(app, ui, show);
        }
    });
}

fn episodes(app: &mut App, ui: &mut egui::Ui, results: &SearchResults, limit: usize) {
    let Some(page) = &results.episodes else {
        return;
    };
    for episode in page.items.iter().take(limit) {
        super::show::episode_row(app, ui, episode, None);
    }
}

#[allow(dead_code)]
fn align_right(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.with_layout(Layout::right_to_left(Align::Center), add);
}
