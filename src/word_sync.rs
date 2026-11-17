//! Optional word-level lyrics from the Spicy Lyrics catalogue.
//!
//! The developer API returns the best sync it has for a Spotify track: a
//! `Syllable` body carries a start and end for every word or syllable, which
//! is what the animated view needs. It is off unless the listener chooses the
//! provider and saves a secret key. The key lives in a private state file,
//! never in the settings JSON or logs.
//!
//! Using the API requires showing its attribution with the lyrics. The parsed
//! [`Lyrics`] carries it, and the views render it at the end of the words.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::lyrics::{Attribution, Credit, Line, Lyrics, Syllable};
use crate::paths::AppDirs;

/// The developer endpoint. The track id is appended to it.
const API: &str = "https://api.spicylyrics.org/v1/lyrics";
/// Where a secret key is issued.
pub const DEVELOPERS_URL: &str = "https://developers.spicylyrics.org/";
const KEY_FILE: &str = "spicy-lyrics-key";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(7);

pub fn key_path(dirs: &AppDirs) -> PathBuf {
    dirs.state.join(KEY_FILE)
}

pub fn load_key(dirs: &AppDirs) -> Option<String> {
    load_key_at(&key_path(dirs))
}

/// The key is trimmed because an editor or a paste can add a newline.
pub fn load_key_at(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

/// A secret key from the developer dashboard. Publishable keys are meant for
/// browsers with an origin allowlist, which a native client cannot provide.
pub fn valid_key(key: &str) -> bool {
    let key = key.trim();
    key.starts_with("sl_sk_") && key.len() > 16
}

/// The catalogue's best sync for `track_id`, when it is word-level.
///
/// A line-level or static answer is `None`: Spotifast already has line lyrics
/// from its own sources, and replacing them would not add timings. A failed
/// request is an error so the caller can fall back without caching a negative
/// answer that the next attempt could correct.
pub async fn fetch(http: &reqwest::Client, track_id: &str, key: &str) -> Result<Option<Lyrics>> {
    if !valid_key(key) || track_id.is_empty() {
        return Ok(None);
    }
    let response = http
        .get(format!("{API}/{track_id}"))
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", key.trim()),
        )
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(reqwest::Error::without_url)
        .context("cannot reach Spicy Lyrics")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        anyhow::bail!("Spicy Lyrics answered {}", response.status());
    }
    let body: serde_json::Value = response
        .json()
        .await
        .context("unexpected answer from Spicy Lyrics")?;
    Ok(parse(&body))
}

/// Reads a `GET /v1/lyrics/{id}` answer. Public for tests.
pub fn parse(json: &serde_json::Value) -> Option<Lyrics> {
    let body = json.get("Body")?;
    if body.get("Type")?.as_str()? != "Syllable" {
        return None;
    }
    // The API requires its attribution wherever the words appear. Without a
    // source there is nothing to show, so the answer is not usable.
    let attribution = attribution(body)?;
    let mut lines = Vec::new();
    for item in body.get("Content")?.as_array()? {
        let lead = item.get("Lead")?;
        let Some(syllables) = lead.get("Syllables").and_then(|value| value.as_array()) else {
            continue;
        };
        let mut text = String::new();
        let mut timed: Vec<Syllable> = Vec::new();
        // `IsPartOfWord` looks forward: a syllable carrying it continues into
        // the next syllable without a space. The space before this syllable
        // therefore depends on the *previous* syllable's flag.
        let mut previous_joins = false;
        for raw in syllables {
            let Some(part) = raw.get("Text").and_then(|value| value.as_str()) else {
                continue;
            };
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let Some(at_ms) = seconds_ms(raw.get("StartTime")) else {
                continue;
            };
            let end_ms = seconds_ms(raw.get("EndTime")).filter(|end| *end > at_ms);
            let joins = previous_joins;
            if !text.is_empty() && !joins {
                text.push(' ');
            }
            text.push_str(part);
            // Keep every provider syllable as its own timed span. `units`
            // joins the word fragments for layout and uses these spans to
            // pace the letters inside the word. The leading space on a new
            // word is also how that layout tells words apart.
            let continues = joins && !timed.is_empty();
            let stored = if continues || timed.is_empty() {
                part.to_string()
            } else {
                format!(" {part}")
            };
            timed.push(Syllable {
                at_ms,
                end_ms,
                text: stored,
                continues,
            });
            previous_joins = raw
                .get("IsPartOfWord")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
        }
        if timed.is_empty() {
            continue;
        }
        let at_ms = seconds_ms(lead.get("StartTime")).or_else(|| timed.first().map(|s| s.at_ms));
        lines.push(Line {
            at_ms,
            text,
            syllables: timed,
        });
    }
    if lines.is_empty() {
        return None;
    }
    Some(Lyrics {
        synced: true,
        instrumental: false,
        lines,
        attribution: Some(attribution),
    })
}

/// The API's times are seconds with a fraction; our model uses milliseconds.
fn seconds_ms(value: Option<&serde_json::Value>) -> Option<u32> {
    let seconds = value?.as_f64()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    let ms = (seconds * 1000.0).round();
    (ms <= f64::from(u32::MAX)).then_some(ms as u32)
}

/// The provider is always named. Community syncs also credit the uploader and
/// the maker; commercial sources have no such fields, and must not show empty
/// credits.
fn attribution(body: &serde_json::Value) -> Option<Attribution> {
    let source = body.get("source").and_then(|value| value.as_str())?;
    let (provider, url) = match source {
        "spicy_lyrics" => ("Spicy Lyrics", Some("https://spicylyrics.org")),
        "apple_music" => ("Apple Music", Some("https://music.apple.com")),
        "spotify" => ("Spotify", Some("https://open.spotify.com")),
        other => (other, None),
    };
    let credit = |key: &str| -> Option<Credit> {
        let value = body.get("UploadAttribution")?.get(key)?;
        let name = value
            .get("username")
            .and_then(|value| value.as_str())?
            .trim();
        if name.is_empty() {
            return None;
        }
        Some(Credit {
            name: name.to_string(),
            url: value
                .get("url")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        })
    };
    let community = source == "spicy_lyrics";
    Some(Attribution {
        provider: provider.to_string(),
        provider_url: url.map(str::to_string),
        uploader: community.then(|| credit("Uploader")).flatten(),
        maker: community.then(|| credit("Maker")).flatten(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syllable(text: &str, start: f64, end: f64, joins: bool) -> serde_json::Value {
        serde_json::json!({
            "Text": text,
            "StartTime": start,
            "EndTime": end,
            "IsPartOfWord": joins,
        })
    }

    fn response(source: &str, syllables: Vec<serde_json::Value>) -> serde_json::Value {
        serde_json::json!({
            "Body": {
                "Type": "Syllable",
                "source": source,
                "StartTime": 1.0,
                "Content": [{
                    "Type": "Vocal",
                    "Lead": {
                        "StartTime": 1.0,
                        "EndTime": 3.0,
                        "Syllables": syllables,
                    }
                }],
                "UploadAttribution": {
                    "Uploader": {"username": "spikerko", "url": "https://spicylyrics.org/uid/1"},
                    "Maker": {"username": "gc", "url": "https://spicylyrics.org/uid/2"}
                }
            }
        })
    }

    #[test]
    fn syllable_timings_become_one_line_with_their_own_spans() {
        let json = response(
            "spicy_lyrics",
            vec![
                syllable("Hel", 1.0, 1.8, true),
                syllable("lo", 1.8, 2.0, false),
                syllable("world", 2.0, 3.0, false),
            ],
        );
        let lyrics = parse(&json).unwrap();
        assert!(lyrics.synced);
        assert_eq!(lyrics.lines.len(), 1);
        assert_eq!(lyrics.lines[0].text, "Hello world");
        assert_eq!(lyrics.lines[0].at_ms, Some(1_000));
        let units = lyrics.units(0, 5_000);
        assert_eq!(units.len(), 2, "the syllables join into two words");
        assert_eq!(units[0].text, "Hello");
        assert_eq!(units[0].at_ms, 1_000);
        assert_eq!(units[0].end_ms, 2_000);
        assert!(units[0].letters, "a held word splits into letters");
        assert_eq!(
            units[0].parts.len(),
            2,
            "each provider syllable keeps its span"
        );
        assert_eq!(units[0].parts[0].chars, 3);
        assert_eq!(units[0].parts[1].chars, 2);
        let letters = Lyrics::letters(&units[0]);
        assert_eq!(
            letters[3].1, 1_800,
            "the second syllable starts its own span"
        );
        assert!(units[1].space_before);
        assert_eq!(units[1].text, "world");
    }

    #[test]
    fn a_forward_part_of_word_flag_keeps_split_words_together() {
        let json = response(
            "spicy_lyrics",
            vec![
                syllable("L", 1.0, 1.2, true),
                syllable("ife", 1.2, 1.6, false),
                syllable("is", 1.6, 1.8, false),
                syllable("short", 1.8, 2.4, false),
            ],
        );
        let lyrics = parse(&json).unwrap();
        assert_eq!(lyrics.lines[0].text, "Life is short");
        let units = lyrics.units(0, 3_000);
        assert_eq!(
            units
                .iter()
                .map(|unit| unit.text.as_str())
                .collect::<Vec<_>>(),
            ["Life", "is", "short"]
        );
    }

    #[test]
    fn community_answers_credit_the_uploader_and_the_maker() {
        let json = response("spicy_lyrics", vec![syllable("word", 1.0, 2.0, false)]);
        let attribution = parse(&json).unwrap().attribution.unwrap();
        assert_eq!(attribution.provider, "Spicy Lyrics");
        assert_eq!(attribution.uploader.as_ref().unwrap().name, "spikerko");
        assert_eq!(
            attribution.maker.as_ref().unwrap().url.as_deref(),
            Some("https://spicylyrics.org/uid/2")
        );
    }

    #[test]
    fn commercial_answers_name_only_the_provider() {
        let mut json = response("apple_music", vec![syllable("word", 1.0, 2.0, false)]);
        json["Body"]
            .as_object_mut()
            .unwrap()
            .remove("UploadAttribution");
        let attribution = parse(&json).unwrap().attribution.unwrap();
        assert_eq!(attribution.provider, "Apple Music");
        assert!(attribution.uploader.is_none());
        assert!(attribution.maker.is_none());
    }

    #[test]
    fn a_line_level_answer_is_not_used() {
        let json = serde_json::json!({
            "Body": {
                "Type": "Line",
                "source": "spotify",
                "Content": []
            }
        });
        assert!(parse(&json).is_none());
    }

    #[test]
    fn an_answer_without_a_source_is_not_used() {
        let mut json = response("spicy_lyrics", vec![syllable("word", 1.0, 2.0, false)]);
        json["Body"].as_object_mut().unwrap().remove("source");
        assert!(parse(&json).is_none());
    }

    #[test]
    fn only_secret_keys_are_accepted() {
        assert!(valid_key("sl_sk_abcdefghijklmnop"));
        assert!(!valid_key("sl_pk_abcdefghijklmnop"));
        assert!(!valid_key(""));
    }
}
