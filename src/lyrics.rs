//! Lyrics for the playing track, from Spotify, a local file or LRCLIB.
//!
//! [LRCLIB](https://lrclib.net) provides plain and LRC-synced lyrics without an
//! account or key. Spotifast tries Spotify's transcription first when the
//! playback session is signed in, then LRCLIB.
//!
//! Matching starts with an exact lookup, then ranks search results. Track
//! length is the strongest signal for distinguishing versions with the same
//! title.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

const API: &str = "https://lrclib.net/api";
/// Lyrics do not change; a cached answer is good for this long.
const CACHE_LIFETIME: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// A cached "Spotify has none" is good for this long: it may be wrong, and
/// costs a look at LRCLIB every time.
const NONE_LIFETIME: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// A candidate this far from the playing track's length is another
/// recording, however well the names match.
const MAX_DRIFT_SECS: f64 = 30.0;
/// LRCLIB answers about one request a second and says how long to wait.
/// A search right after a missed exact lookup often lands in that second.
const RATE_LIMIT_RETRIES: u32 = 3;
const SERVICE_RETRIES: u32 = 1;
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Any longer and the song may have moved on; the error shows instead,
/// and a click on it asks again.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(5);
/// The optional word-level provider must not hold up the ordinary lyrics.
const WORD_SYNC_TIMEOUT: Duration = Duration::from_secs(7);

/// Track metadata used for lyrics lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration_ms: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    /// When the line starts; `None` in lyrics that carry no timing.
    pub at_ms: Option<u32>,
    pub text: String,
    /// Word timings, when the source has them: Spotify's `syllables` array or
    /// an enhanced LRC line. Empty for line-synced words, which are spread
    /// across the line instead. Older cached answers load as empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub syllables: Vec<Syllable>,
}

/// One timed word or syllable inside a line, in milliseconds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Syllable {
    pub at_ms: u32,
    /// The source's own end, when it has one. Enhanced LRC does not, so the
    /// next syllable or the line's end is used instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<u32>,
    pub text: String,
    /// Whether this syllable continues the word before it. Provider answers
    /// use this where their text has no separating space.
    #[serde(default)]
    pub continues: bool,
}

/// One timed syllable inside a word. Its character count lets the letter
/// animation spend each real syllable's own span, rather than spreading the
/// whole word evenly across a long hold.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WordPart {
    pub at_ms: u32,
    pub end_ms: u32,
    pub chars: usize,
}

/// One animation unit of a line: a word, or a syllable of one, that the view
/// can split into letters when it is held long enough.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Word {
    pub at_ms: u32,
    pub end_ms: u32,
    pub text: String,
    /// Whether a space separates this unit from the one before it.
    pub space_before: bool,
    /// Whether its letters animate one at a time, as Spicy Lyrics splits a
    /// word held for a second or more.
    pub letters: bool,
    /// Real syllable spans inside this word, when the source supplied them.
    /// Empty means the word's letters share its whole span evenly.
    pub parts: Vec<WordPart>,
}

/// The shortest unit Spicy Lyrics splits into letters.
const LETTER_SPLIT_MS: u32 = 1_000;
/// Its letters finish this much before the word does, so the last one lands
/// with the voice rather than after it.
const LETTER_TRIM_MS: u32 = 250;

/// One credited person or profile on a lyrics answer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credit {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Where the words came from, shown with them. The Spicy Lyrics API requires
/// this attribution; community syncs also name their uploader and maker.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attribution {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uploader: Option<Credit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maker: Option<Credit>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lyrics {
    pub lines: Vec<Line>,
    /// Every line has a time, so the panel can follow the song.
    pub synced: bool,
    /// The database knows there are no words.
    pub instrumental: bool,
    /// Present only when a word-level provider supplied the words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<Attribution>,
}

impl Lyrics {
    /// The line being sung at `position_ms`, or `None` before the first one
    /// starts and for lyrics without timing.
    pub fn active_line(&self, position_ms: u32) -> Option<usize> {
        if !self.synced {
            return None;
        }
        let started = self
            .lines
            .partition_point(|line| line.at_ms.is_some_and(|at| at <= position_ms));
        started.checked_sub(1)
    }

    /// When the line at `index` stops being sung: the next timed line's
    /// start, or the track's own length for the last one.
    pub fn line_end(&self, index: usize, track_duration_ms: u32) -> Option<u32> {
        let start = self.lines.get(index)?.at_ms?;
        let next = self
            .lines
            .iter()
            .skip(index + 1)
            .find_map(|line| line.at_ms)
            .or(Some(track_duration_ms).filter(|end| *end > start))
            .unwrap_or(start + 3_000);
        Some(next.max(start + 400))
    }

    /// The words of the line at `index`, each with the moment it starts.
    ///
    /// Spotify and LRCLIB time whole lines, not words, so a line's words are
    /// spread across its own time in proportion to how long they are. That is
    /// what lets a word-synced view follow a line-synced source. Lyrics
    /// without timing give every word `None`.
    pub fn words(&self, index: usize, track_duration_ms: u32) -> Vec<(&str, Option<u32>)> {
        let Some(line) = self.lines.get(index) else {
            return Vec::new();
        };
        let tokens: Vec<&str> = line.text.split_whitespace().collect();
        let Some(start) = line.at_ms.filter(|_| self.synced) else {
            return tokens.into_iter().map(|word| (word, None)).collect();
        };
        let end = self.line_end(index, track_duration_ms).unwrap_or(start);
        let span = u64::from(end.saturating_sub(start));
        let letters: u64 = tokens
            .iter()
            .map(|word| word.chars().count().max(1) as u64)
            .sum();
        let mut walked = 0;
        let mut words = Vec::with_capacity(tokens.len());
        for word in tokens {
            words.push((word, Some(start + (span * walked / letters.max(1)) as u32)));
            walked += word.chars().count().max(1) as u64;
        }
        words
    }

    /// The line's animation units.
    ///
    /// A source with word or syllable timings keeps them, and only those
    /// units animate one at a time. A line-synced source has no word times:
    /// its words carry the line's own span so the view can lay them out and
    /// sweep the line as a whole, without inventing a time for each word.
    pub fn units(&self, index: usize, track_duration_ms: u32) -> Vec<Word> {
        let Some(line) = self.lines.get(index) else {
            return Vec::new();
        };
        let line_end = self.line_end(index, track_duration_ms);
        let mut units: Vec<Word> = Vec::new();
        if self.synced && !line.syllables.is_empty() {
            for (at, syllable) in line.syllables.iter().enumerate() {
                let text = syllable.text.trim();
                if text.is_empty() {
                    continue;
                }
                let next = line
                    .syllables
                    .iter()
                    .skip(at + 1)
                    .find(|next| next.at_ms > syllable.at_ms)
                    .map(|next| next.at_ms);
                let end = syllable
                    .end_ms
                    .filter(|end| *end > syllable.at_ms)
                    .or(next)
                    .or(line_end)
                    .unwrap_or(syllable.at_ms);
                let raw = &syllable.text;
                let space_before = units.is_empty()
                    || raw.starts_with(char::is_whitespace)
                    || (at > 0 && line.syllables[at - 1].text.ends_with(char::is_whitespace));
                let end = end.max(syllable.at_ms);
                let chars = text.chars().count();
                if (syllable.continues || !space_before)
                    && let Some(unit) = units.last_mut()
                {
                    // A source can split a word without a leading space.
                    // Keep it in the same unit, with its own timed span.
                    unit.text.push_str(text);
                    unit.end_ms = unit.end_ms.max(end);
                    unit.parts.push(WordPart {
                        at_ms: syllable.at_ms,
                        end_ms: end,
                        chars,
                    });
                    unit.letters = unit.end_ms.saturating_sub(unit.at_ms) >= LETTER_SPLIT_MS;
                } else {
                    units.push(Word {
                        at_ms: syllable.at_ms,
                        end_ms: end,
                        text: text.to_string(),
                        space_before,
                        letters: end.saturating_sub(syllable.at_ms) >= LETTER_SPLIT_MS,
                        parts: vec![WordPart {
                            at_ms: syllable.at_ms,
                            end_ms: end,
                            chars,
                        }],
                    });
                }
            }
        }
        if units.is_empty() {
            let Some(start) = line.at_ms.filter(|_| self.synced) else {
                return units;
            };
            let end = line_end.unwrap_or(start);
            for text in line.text.split_whitespace() {
                units.push(Word {
                    at_ms: start,
                    end_ms: end,
                    text: text.to_string(),
                    space_before: !units.is_empty(),
                    letters: false,
                    parts: Vec::new(),
                });
            }
        }
        units
    }

    /// `unit`'s letters, each with the instant its own animation starts and
    /// ends. A unit that is not split gives every letter the unit's own span.
    pub fn letters(unit: &Word) -> Vec<(char, u32, u32)> {
        let characters: Vec<char> = unit.text.chars().collect();
        if characters.is_empty() {
            return Vec::new();
        }
        if !unit.letters || characters.len() < 2 {
            return characters
                .into_iter()
                .map(|character| (character, unit.at_ms, unit.end_ms))
                .collect();
        }
        let span = unit.end_ms.saturating_sub(unit.at_ms);
        let sung = span.saturating_sub(LETTER_TRIM_MS).max(span / 2);
        let parts_chars: usize = unit.parts.iter().map(|part| part.chars).sum();
        if unit.parts.is_empty() || parts_chars != characters.len() {
            let each = sung / characters.len() as u32;
            return characters
                .into_iter()
                .enumerate()
                .map(|(index, character)| {
                    let start = unit.at_ms + each * index as u32;
                    (character, start, start + each.max(1))
                })
                .collect();
        }
        let mut timed = Vec::with_capacity(characters.len());
        let mut index = 0;
        for part in &unit.parts {
            let count = part.chars.min(characters.len().saturating_sub(index));
            if count == 0 {
                continue;
            }
            let part_span = part.end_ms.saturating_sub(part.at_ms);
            let part_sung = part_span.saturating_sub(LETTER_TRIM_MS).max(part_span / 2);
            let each = (part_sung / count as u32).max(1);
            for offset in 0..count {
                let start = part.at_ms.saturating_add(each * offset as u32);
                timed.push((characters[index], start, start.saturating_add(each)));
                index += 1;
            }
        }
        debug_assert_eq!(index, characters.len());
        timed
    }
}

/// Parses Spotify's `color-lyrics` response.
pub fn from_spotify(json: &serde_json::Value) -> Option<Lyrics> {
    let lyrics = json.get("lyrics")?;
    // LINE_SYNCED is the usual value, but word and syllable sync modes use
    // their own names. Only UNSYNCED means the times are absent.
    let declared_synced =
        lyrics.get("syncType").and_then(|value| value.as_str()) != Some("UNSYNCED");
    let lines: Vec<Line> = lyrics
        .get("lines")?
        .as_array()?
        .iter()
        .filter_map(|line| {
            let text = line.get("words")?.as_str()?.trim();
            if text.is_empty() || text == "\u{266a}" {
                return None;
            }
            let at_ms = line
                .get("startTimeMs")
                .and_then(|value| {
                    value
                        .as_str()
                        .and_then(|text| text.parse().ok())
                        .or_else(|| value.as_u64().and_then(|n| u32::try_from(n).ok()))
                })
                .filter(|_| declared_synced);
            Some(Line {
                at_ms,
                text: text.to_string(),
                syllables: syllables_from(line),
            })
        })
        .collect();
    if lines.is_empty() {
        return None;
    }
    let synced = declared_synced && lines.iter().all(|line| line.at_ms.is_some());
    Some(Lyrics {
        lines,
        synced,
        instrumental: false,
        attribution: None,
    })
}

/// The word timings on one Spotify `color-lyrics` line, if it has any.
fn syllables_from(line: &serde_json::Value) -> Vec<Syllable> {
    line.get("syllables")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    // The raw text is kept: the spaces around it are how the
                    // view knows two syllables are separate words.
                    let text = item.get("words")?.as_str()?;
                    if text.trim().is_empty() {
                        return None;
                    }
                    Some(Syllable {
                        at_ms: json_ms(item.get("startTimeMs")?)?,
                        end_ms: item.get("endTimeMs").and_then(json_ms),
                        text: text.to_string(),
                        continues: false,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A time Spotify sends as a string, and older answers as a number.
fn json_ms(value: &serde_json::Value) -> Option<u32> {
    value
        .as_str()
        .and_then(|text| text.parse().ok())
        .or_else(|| value.as_u64().and_then(|ms| u32::try_from(ms).ok()))
}

/// The cached answer at `path`, while it is fresh.
pub fn cached(path: &Path) -> Option<Option<Lyrics>> {
    let answer = read_cache(&path.to_path_buf())?;
    if answer.is_none() && older_than(path, NONE_LIFETIME) {
        return None;
    }
    Some(answer)
}

/// Whether the file at `path` was written over `age` ago.
fn older_than(path: &Path, age: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_none_or(|elapsed| elapsed > age)
}

/// Remember an answer at `path`, `None` included.
pub fn store(path: &Path, found: &Option<Lyrics>) {
    write_cache(path, found);
}

/// A previously found transcription still helps during an outage. An old
/// negative answer must never turn a failed request into "no lyrics".
pub fn previously_found(path: &Path) -> Option<Lyrics> {
    read_cache_contents(path).flatten()
}

/// A word-level provider to try before the line-level services.
pub struct WordSync<'a> {
    pub track_id: &'a str,
    pub key: &'a str,
}

/// Asks the optional provider for word timings, caching both answers and
/// misses. A miss falls back to the ordinary line-level lookup.
pub async fn word_sync(
    http: &reqwest::Client,
    cache_dir: &Path,
    word_sync: WordSync<'_>,
) -> Option<Lyrics> {
    // v2: the first parser read IsPartOfWord backwards and cached words with
    // spaces inside them. Ignore those files rather than showing them again.
    let path = cache_dir.join(format!("words-v2-{}.json", word_sync.track_id));
    if let Some(cached) = read_cache(&path)
        && (cached.is_some() || !older_than(&path, NONE_LIFETIME))
    {
        return cached;
    }
    let result = tokio::time::timeout(
        WORD_SYNC_TIMEOUT,
        crate::word_sync::fetch(http, word_sync.track_id, word_sync.key),
    )
    .await;
    match result {
        Ok(Ok(Some(found))) => {
            write_cache(&path, &Some(found.clone()));
            Some(found)
        }
        Ok(Ok(None)) => {
            write_cache(&path, &None);
            None
        }
        // A refused key or an outage must not be remembered as "no words".
        Ok(Err(_)) | Err(_) => None,
    }
}

/// Fetches lyrics for `query`, using the disk cache when available.
pub async fn fetch(
    http: &reqwest::Client,
    cache_dir: &Path,
    query: &Query,
) -> Result<Option<Lyrics>> {
    fetch_from(http, API, cache_dir, query).await
}

async fn fetch_from(
    http: &reqwest::Client,
    api: &str,
    cache_dir: &Path,
    query: &Query,
) -> Result<Option<Lyrics>> {
    let cache_path = cache_dir.join(format!("{}.json", cache_key(query)));
    if let Some(cached) = read_cache(&cache_path) {
        return Ok(cached);
    }
    let result = tokio::time::timeout(LOOKUP_TIMEOUT, lookup(http, api, query))
        .await
        .context("lyrics service did not respond in time")
        .and_then(|result| result);
    let found = match result {
        Ok(found) => found,
        Err(error) => return previously_found(&cache_path).map(Some).ok_or(error),
    };
    write_cache(&cache_path, &found);
    Ok(found)
}

async fn lookup(http: &reqwest::Client, api: &str, query: &Query) -> Result<Option<Lyrics>> {
    let title = clean_title(&query.title);
    let artist = clean_artist(&query.artist);
    if title.is_empty() || artist.is_empty() {
        return Ok(None);
    }
    let duration = (query.duration_ms / 1000).to_string();
    let mut exact = vec![
        ("artist_name", artist.as_str()),
        ("track_name", title.as_str()),
    ];
    if !query.album.trim().is_empty() {
        exact.push(("album_name", query.album.trim()));
    }
    if query.duration_ms > 0 {
        exact.push(("duration", duration.as_str()));
    }
    let exact_error = match get_from::<Record>(http, api, "/get", &exact).await {
        Ok(Some(record)) => {
            if let Some(lyrics) = record.lyrics() {
                return Ok(Some(lyrics));
            }
            None
        }
        Ok(None) => None,
        Err(error) => {
            // LRCLIB's exact lookup and search can fail independently. A
            // transient exact failure is not proof that search has no lyrics.
            let Some(wait) = search_after(&error) else {
                return Err(error);
            };
            tokio::time::sleep(wait).await;
            Some(error)
        }
    };
    let candidates = get_from::<Vec<Record>>(
        http,
        api,
        "/search",
        &[
            ("artist_name", artist.as_str()),
            ("track_name", title.as_str()),
        ],
    )
    .await?
    .unwrap_or_default();
    let found = pick(candidates, query).and_then(|record| record.lyrics());
    match (found, exact_error) {
        (None, Some(error)) => Err(error),
        (found, _) => Ok(found),
    }
}

#[derive(Debug)]
struct RequestFailure {
    status: reqwest::StatusCode,
    wait: Duration,
}

impl std::fmt::Display for RequestFailure {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "LRCLIB answered {}", self.status)
    }
}

impl std::error::Error for RequestFailure {}

fn service_busy(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 502..=504)
}

fn search_after(error: &anyhow::Error) -> Option<Duration> {
    if let Some(failure) = error.downcast_ref::<RequestFailure>() {
        return (service_busy(failure.status) && failure.wait <= MAX_RETRY_AFTER)
            .then_some(failure.wait);
    }
    error
        .downcast_ref::<reqwest::Error>()
        .filter(|error| error.is_connect() || error.is_timeout())
        .map(|_| Duration::from_secs(1))
}

fn retry_after(headers: &reqwest::header::HeaderMap) -> Duration {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .map_or(Duration::from_secs(1), |value| {
            value
                .to_str()
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .map_or(Duration::MAX, Duration::from_secs)
        })
}

/// One LRCLIB request. A 404 is "nobody has transcribed this" and a 400 is
/// "that is not a question I can answer"; neither is a fault worth showing.
/// Retry short overloads, but stop when the requested wait exceeds our
/// budget. Never shorten Retry-After, including an unparsed HTTP date.
async fn get_from<T: DeserializeOwned>(
    http: &reqwest::Client,
    api: &str,
    path: &str,
    params: &[(&str, &str)],
) -> Result<Option<T>> {
    let mut attempt = 0;
    let response = loop {
        let response = http
            .get(format!("{api}{path}"))
            .query(params)
            .header("Accept", "application/json")
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("cannot reach LRCLIB")?;
        let retries = if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            RATE_LIMIT_RETRIES
        } else if service_busy(response.status()) {
            SERVICE_RETRIES
        } else {
            0
        };
        let wait = retry_after(response.headers());
        if attempt >= retries || wait > MAX_RETRY_AFTER {
            break response;
        }
        attempt += 1;
        tokio::time::sleep(wait).await;
    };
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::BAD_REQUEST {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(RequestFailure {
            status,
            wait: retry_after(response.headers()),
        }
        .into());
    }
    Ok(Some(
        response
            .json()
            .await
            .context("unexpected answer from LRCLIB")?,
    ))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Record {
    track_name: String,
    artist_name: String,
    duration: Option<f64>,
    instrumental: bool,
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}

impl Record {
    fn synced(&self) -> Option<&str> {
        self.synced_lyrics
            .as_deref()
            .filter(|text| !text.trim().is_empty())
    }

    fn plain(&self) -> Option<&str> {
        self.plain_lyrics
            .as_deref()
            .filter(|text| !text.trim().is_empty())
    }

    fn lyrics(&self) -> Option<Lyrics> {
        if self.instrumental {
            return Some(Lyrics {
                instrumental: true,
                attribution: None,
                ..Lyrics::default()
            });
        }
        if let Some(synced) = self.synced() {
            let lines = parse_lrc(synced);
            if !lines.is_empty() {
                return Some(Lyrics {
                    lines,
                    synced: true,
                    instrumental: false,
                    attribution: None,
                });
            }
        }
        let plain = self.plain()?;
        Some(Lyrics {
            lines: plain
                .lines()
                .map(|line| Line {
                    at_ms: None,
                    text: line.trim_end().to_string(),
                    ..Line::default()
                })
                .collect(),
            synced: false,
            instrumental: false,
            attribution: None,
        })
    }
}

/// How well a search result fits what is playing; `None` rules it out.
fn score(record: &Record, query: &Query) -> Option<i64> {
    if !loose_match(&record.track_name, &clean_title(&query.title)) {
        return None;
    }
    let mut score = 0;
    if loose_match(&record.artist_name, &clean_artist(&query.artist)) {
        score += 1000;
    }
    if query.duration_ms > 0
        && let Some(duration) = record.duration.filter(|duration| *duration > 0.0)
    {
        let drift = (duration - f64::from(query.duration_ms) / 1000.0).abs();
        if drift > MAX_DRIFT_SECS {
            return None;
        }
        score += ((MAX_DRIFT_SECS - drift) * 10.0) as i64;
    }
    // Timing is the point, so a synced upload wins a tie.
    if record.synced().is_some() {
        score += 200;
    } else if record.plain().is_some() {
        score += 50;
    }
    Some(score)
}

fn pick(records: Vec<Record>, query: &Query) -> Option<Record> {
    let mut best: Option<(i64, Record)> = None;
    for record in records {
        let Some(score) = score(&record, query) else {
            continue;
        };
        if best.as_ref().is_none_or(|(held, _)| score > *held) {
            best = Some((score, record));
        }
    }
    best.map(|(_, record)| record)
}

// ---- names ----------------------------------------------------------------

/// Words a title carries in brackets that a lyrics database does not.
const BRACKET_NOISE: &[&str] = &[
    "remaster",
    "remastered",
    "remix",
    "live",
    "acoustic",
    "version",
    "edit",
    "mix",
    "mono",
    "stereo",
    "deluxe",
    "bonus",
    "expanded",
    "explicit",
    "anniversary",
    "feat",
    "featuring",
    "with",
];

/// What a " - " suffix says when it is not part of the title.
const SUFFIX_NOISE: &[&str] = &[
    "remaster",
    "remastered",
    "radio edit",
    "single version",
    "album version",
    "live",
    "mono",
    "stereo",
    "rerecorded",
    "re-recorded",
];

/// Invisible characters that carry no meaning: a zero-width space, the word
/// joiner and the invisible operators, and a byte order mark. Not the
/// zero-width joiner or non-joiner, which hold emoji sequences and Indic
/// and Persian words together.
fn is_disposable(c: char) -> bool {
    matches!(c, '\u{200b}' | '\u{2060}'..='\u{2064}' | '\u{feff}')
}

/// Whether `text` contains `phrase` as whole words, case-insensitively.
fn has_phrase(text: &str, phrase: &str) -> bool {
    let words: Vec<String> = text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '-')
        .filter(|word| !word.is_empty())
        .map(|word| word.trim_matches('-').to_string())
        .collect();
    let wanted: Vec<&str> = phrase.split(' ').collect();
    words
        .windows(wanted.len())
        .any(|window| window.iter().zip(&wanted).all(|(a, b)| a == b))
}

/// Strips what a player adds to a title and a database leaves out:
/// "(Remastered 2011)", " - Live at Wembley", "feat. Someone".
pub fn clean_title(title: &str) -> String {
    let original: String = title.chars().filter(|c| !is_disposable(*c)).collect();
    let mut cleaned = String::with_capacity(original.len());
    let mut rest = original.as_str();
    while let Some(open) = rest.find(['(', '[']) {
        let closer = if rest[open..].starts_with('(') {
            ')'
        } else {
            ']'
        };
        let Some(close) = rest[open..].find(closer) else {
            break;
        };
        let inner = &rest[open + 1..open + close];
        let noisy = BRACKET_NOISE.iter().any(|word| has_phrase(inner, word));
        if noisy {
            cleaned.push_str(rest[..open].trim_end());
        } else {
            cleaned.push_str(&rest[..open + close + 1]);
        }
        rest = &rest[open + close + 1..];
    }
    cleaned.push_str(rest);
    let mut cleaned = cleaned.trim().to_string();
    // " - Remastered 2009" and friends, from the first dash whose tail is
    // noise; a dash inside a real title stays.
    let mut cut = None;
    for (index, _) in cleaned.match_indices(" - ") {
        let tail = &cleaned[index + 3..];
        if SUFFIX_NOISE.iter().any(|phrase| has_phrase(tail, phrase))
            || tail
                .split_whitespace()
                .next()
                .is_some_and(|word| word.len() == 4 && word.chars().all(|c| c.is_ascii_digit()))
                && has_phrase(tail, "version")
        {
            cut = Some(index);
            break;
        }
    }
    if let Some(index) = cut {
        cleaned.truncate(index);
    }
    let cleaned = strip_featuring(cleaned.trim());
    if cleaned.is_empty() {
        original.trim().to_string()
    } else {
        cleaned
    }
}

/// Everything from a standalone "feat", "ft", or "featuring" on.
fn strip_featuring(text: &str) -> String {
    // The marker is looked for in a lower-cased copy and the cut is made in
    // `text`, and lower casing does not preserve length: Turkish 'İ' becomes
    // two characters and one byte longer, capital sharp s one byte shorter.
    // An offset measured in the copy therefore means somewhere else in the
    // title -- letters kept that should have gone, or, when it lands inside a
    // character, a panic. `starts` carries every offset in the copy back to
    // the same place in `text`, so no offset is ever used in the string it
    // was not measured in.
    let mut lower = String::with_capacity(text.len());
    let mut starts: Vec<usize> = Vec::with_capacity(text.len() + 1);
    for (at, character) in text.char_indices() {
        for lowered in character.to_lowercase() {
            for _ in 0..lowered.len_utf8() {
                starts.push(at);
            }
            lower.push(lowered);
        }
    }
    starts.push(text.len());
    let mut cut = None;
    for marker in ["featuring", "feat", "ft"] {
        let mut from = 0;
        while let Some(found) = lower[from..].find(marker) {
            let start = from + found;
            let end = start + marker.len();
            let before = lower[..start].trim_end_matches(['-', '(']).trim_end();
            let preceded = before.len() < start && !before.is_empty();
            let followed = lower[end..]
                .strip_prefix('.')
                .unwrap_or(&lower[end..])
                .starts_with(' ');
            if preceded && followed {
                let at = before.len();
                cut = Some(cut.map_or(at, |held: usize| held.min(at)));
                break;
            }
            from = end;
        }
    }
    match cut {
        Some(at) => text[..starts[at]].trim().to_string(),
        None => text.trim().to_string(),
    }
}

/// Players report collaborations in ways a database does not file them, and
/// LRCLIB itself sometimes stores "TOOL;Tool" for one artist.
pub fn clean_artist(artist: &str) -> String {
    let original: String = artist.chars().filter(|c| !is_disposable(*c)).collect();
    let cleaned = strip_featuring(&original);
    let cleaned = cleaned.split(';').next().unwrap_or_default().trim();
    if cleaned.is_empty() {
        original.trim().to_string()
    } else {
        cleaned.to_string()
    }
}

/// Lowercase, accents folded, punctuation gone, one space between words.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        let folded = fold(c);
        if folded == '\'' || folded == '’' || folded == '`' {
            continue;
        }
        if folded == '&' {
            for word in [' ', 'a', 'n', 'd', ' '] {
                push_normalized(&mut out, &mut space, word);
            }
            continue;
        }
        push_normalized(&mut out, &mut space, folded);
    }
    out.trim().to_string()
}

fn push_normalized(out: &mut String, space: &mut bool, c: char) {
    if c.is_alphanumeric() {
        out.extend(c.to_lowercase());
        *space = false;
    } else if !*space {
        out.push(' ');
        *space = true;
    }
}

/// The plain letter behind the Latin accents titles most often carry.
fn fold(c: char) -> char {
    match c {
        'À'..='Å' | 'à'..='å' => 'a',
        'Ç' | 'ç' => 'c',
        'È'..='Ë' | 'è'..='ë' => 'e',
        'Ì'..='Ï' | 'ì'..='ï' => 'i',
        'Ñ' | 'ñ' => 'n',
        'Ò'..='Ö' | 'Ø' | 'ò'..='ö' | 'ø' => 'o',
        'Ù'..='Ü' | 'ù'..='ü' => 'u',
        'Ý' | 'ý' | 'ÿ' => 'y',
        'ß' => 's',
        _ => c,
    }
}

fn loose_match(left: &str, right: &str) -> bool {
    let a = normalize(left);
    let b = normalize(right);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || a.contains(&b) || b.contains(&a)
}

/// Lyrics from text a file holds: timed when its lines carry LRC stamps,
/// plain lines otherwise. `None` for text without words.
pub fn from_text(text: &str) -> Option<Lyrics> {
    if text.trim().is_empty() {
        return None;
    }
    let timed = parse_lrc(text);
    if !timed.is_empty() {
        return Some(Lyrics {
            lines: timed,
            synced: true,
            instrumental: false,
            attribution: None,
        });
    }
    Some(Lyrics {
        lines: text
            .trim()
            .lines()
            .map(|line| Line {
                at_ms: None,
                text: line.trim_end().to_string(),
                ..Line::default()
            })
            .collect(),
        synced: false,
        instrumental: false,
        attribution: None,
    })
}

// ---- LRC ------------------------------------------------------------------

/// Parses `[mm:ss.xx]` lines. A line may open with several stamps when the
/// same words repeat; tags such as `[ar:...]` carry no digits and are
/// skipped. The result is sorted by time.
pub fn parse_lrc(text: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim_start();
        let mut times = Vec::new();
        while let Some(stamp) = leading_stamp(rest) {
            times.push(stamp.0);
            rest = &rest[stamp.1..];
        }
        if times.is_empty() {
            continue;
        }
        let body = rest.trim();
        let (text, syllables) = word_stamps(body);
        for at_ms in times {
            lines.push(Line {
                at_ms: Some(at_ms),
                text: text.clone(),
                syllables: syllables.clone(),
            });
        }
    }
    lines.sort_by_key(|line| line.at_ms);
    lines
}

/// Enhanced LRC puts `<mm:ss.xx>` before each word. Returns the text with the
/// stamps removed and the words they time, or the text alone when it has none.
fn word_stamps(body: &str) -> (String, Vec<Syllable>) {
    // Each stamp: when it starts, where its text starts, and where the stamp
    // itself began (which ends the word before it).
    let mut stamps: Vec<(u32, usize, usize)> = Vec::new();
    let mut search = 0;
    while let Some(offset) = body[search..].find('<') {
        let at = search + offset;
        if let Some((at_ms, length)) = angle_stamp(&body[at..]) {
            stamps.push((at_ms, at, at + length));
            search = at + length;
        } else {
            search = at + 1;
        }
    }
    if stamps.is_empty() {
        return (body.to_string(), Vec::new());
    }
    let mut words = Vec::new();
    for (index, (at_ms, _, text_start)) in stamps.iter().enumerate() {
        let to = stamps
            .get(index + 1)
            .map(|(_, next_tag, _)| *next_tag)
            .unwrap_or(body.len());
        let piece = &body[*text_start..to];
        if !piece.trim().is_empty() {
            words.push(Syllable {
                at_ms: *at_ms,
                end_ms: None,
                text: piece.to_string(),
                continues: false,
            });
        }
    }
    let text: String = words.iter().map(|word| word.text.as_str()).collect();
    (text.trim().to_string(), words)
}

/// An enhanced LRC word stamp at the head of `text`: its milliseconds and its
/// length.
fn angle_stamp(text: &str) -> Option<(u32, usize)> {
    let inner = text.strip_prefix('<')?;
    let close = inner.find('>')?;
    let stamp = &inner[..close];
    let (minutes, seconds) = stamp.split_once(':')?;
    let (seconds, fraction) = match seconds.find(['.', ':']) {
        Some(dot) => (&seconds[..dot], Some(&seconds[dot + 1..])),
        None => (seconds, None),
    };
    let minutes: u32 = minutes.parse().ok()?;
    let seconds: u32 = seconds.parse().ok()?;
    let fraction_ms = match fraction {
        Some(digits) if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) => {
            let value: u32 = digits.chars().take(3).collect::<String>().parse().ok()?;
            value * 10u32.pow(3u32.saturating_sub(digits.len().min(3) as u32))
        }
        Some(_) => return None,
        None => 0,
    };
    Some((minutes * 60_000 + seconds * 1_000 + fraction_ms, close + 2))
}

/// A timestamp at the head of `text`: its milliseconds and its length.
fn leading_stamp(text: &str) -> Option<(u32, usize)> {
    let inner = text.strip_prefix('[')?;
    let close = inner.find(']')?;
    let stamp = &inner[..close];
    let (minutes, seconds) = stamp.split_once(':')?;
    let (seconds, fraction) = match seconds.find(['.', ':']) {
        Some(dot) => (&seconds[..dot], Some(&seconds[dot + 1..])),
        None => (seconds, None),
    };
    let minutes: u32 = minutes.parse().ok()?;
    let seconds: u32 = seconds.parse().ok()?;
    let fraction_ms = match fraction {
        Some(digits) if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) => {
            let value: u32 = digits.chars().take(3).collect::<String>().parse().ok()?;
            value * 10u32.pow(3u32.saturating_sub(digits.len().min(3) as u32))
        }
        Some(_) => return None,
        None => 0,
    };
    Some((minutes * 60_000 + seconds * 1_000 + fraction_ms, close + 2))
}

// ---- cache ------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct Cached {
    found: Option<Lyrics>,
}

fn cache_key(query: &Query) -> String {
    let digest = Sha1::digest(
        format!(
            "{}|{}|{}|{}",
            query.artist, query.title, query.album, query.duration_ms
        )
        .as_bytes(),
    );
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn read_cache(path: &PathBuf) -> Option<Option<Lyrics>> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    if modified.elapsed().unwrap_or(CACHE_LIFETIME) >= CACHE_LIFETIME {
        return None;
    }
    read_cache_contents(path)
}

fn read_cache_contents(path: &Path) -> Option<Option<Lyrics>> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Cached>(&text)
        .ok()
        .map(|cached| cached.found)
}

fn write_cache(path: &Path, found: &Option<Lyrics>) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(&Cached {
        found: found.clone(),
    }) {
        let _ = std::fs::write(path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A local LRCLIB that gives each request, in turn, one of `answers`.
    async fn lrclib(
        answers: Vec<impl Into<String>>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let answers: Vec<String> = answers.into_iter().map(Into::into).collect();
        let server = tokio::spawn(async move {
            let mut paths = Vec::new();
            for answer in answers {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0u8; 4096];
                let len = socket.read(&mut bytes).await.unwrap();
                assert!(len > 0);
                paths.push(
                    String::from_utf8_lossy(&bytes[..len])
                        .split_whitespace()
                        .nth(1)
                        .unwrap()
                        .split('?')
                        .next()
                        .unwrap()
                        .to_owned(),
                );
                socket.write_all(answer.as_bytes()).await.unwrap();
            }
            paths
        });
        (format!("http://{address}"), server)
    }

    const LIMITED: &str = "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    const UNAVAILABLE: &str = "HTTP/1.1 503 Service Unavailable\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

    fn answer(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn outage_query() -> Query {
        Query {
            artist: "Artist".into(),
            title: "Song".into(),
            album: "Album".into(),
            duration_ms: 120_000,
        }
    }

    #[tokio::test]
    async fn search_recovers_when_the_exact_lookup_stays_unavailable() {
        let found = answer(
            r#"[{"trackName":"Song","artistName":"Artist","duration":120,"syncedLyrics":"[00:01.00] A test line"}]"#,
        );
        let (api, server) =
            lrclib(vec![UNAVAILABLE.to_owned(), UNAVAILABLE.to_owned(), found]).await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let lyrics = lookup(&http, &api, &outage_query()).await.unwrap().unwrap();
        assert!(lyrics.synced);
        assert_eq!(lyrics.lines[0].text, "A test line");
        assert_eq!(server.await.unwrap(), ["/get", "/get", "/search"]);
    }

    #[tokio::test]
    async fn a_short_503_is_retried_before_search() {
        let found = answer(r#"{"plainLyrics":"A test line"}"#);
        let (api, server) = lrclib(vec![UNAVAILABLE.to_owned(), found]).await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        assert!(
            lookup(&http, &api, &outage_query())
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(server.await.unwrap(), ["/get", "/get"]);
    }

    #[tokio::test]
    async fn an_empty_search_after_a_failed_exact_lookup_is_not_a_cached_miss() {
        let (api, server) = lrclib(vec![
            UNAVAILABLE.to_owned(),
            UNAVAILABLE.to_owned(),
            answer("[]"),
        ])
        .await;
        let root =
            std::env::temp_dir().join(format!("spotifast-lyrics-miss-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        assert!(
            fetch_from(&http, &api, &root, &outage_query())
                .await
                .is_err()
        );
        assert!(
            !root
                .join(format!("{}.json", cache_key(&outage_query())))
                .exists()
        );
        assert_eq!(server.await.unwrap(), ["/get", "/get", "/search"]);
    }

    #[tokio::test]
    async fn outages_keep_old_lyrics_without_renewing_their_cache_or_reusing_old_misses() {
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let root =
            std::env::temp_dir().join(format!("spotifast-lyrics-stale-{}", std::process::id()));
        let path = root.join(format!("{}.json", cache_key(&outage_query())));
        for found in [from_text("[00:01.00] A test line"), None] {
            write_cache(&path, &found);
            let stale = std::time::SystemTime::now() - CACHE_LIFETIME - Duration::from_secs(60);
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(stale))
                .unwrap();
            assert!(read_cache(&path).is_none());
            let (api, server) = lrclib(vec![UNAVAILABLE; 4]).await;
            let result = fetch_from(&http, &api, &root, &outage_query()).await;
            if found.is_some() {
                assert_eq!(result.unwrap().unwrap().lines[0].text, "A test line");
            } else {
                assert!(result.is_err());
            }
            assert!(
                std::fs::metadata(&path)
                    .unwrap()
                    .modified()
                    .unwrap()
                    .elapsed()
                    .unwrap()
                    > CACHE_LIFETIME
            );
            assert_eq!(
                server.await.unwrap(),
                ["/get", "/get", "/search", "/search"]
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_long_retry_after_is_not_shortened_or_bypassed_by_search() {
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        for wait in ["60", "Thu, 29 Oct 2099 12:00:00 GMT"] {
            let response = format!(
                "HTTP/1.1 503 Service Unavailable\r\nRetry-After: {wait}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let (api, server) = lrclib(vec![response]).await;
            let result =
                tokio::time::timeout(Duration::from_secs(1), lookup(&http, &api, &outage_query()))
                    .await
                    .unwrap();
            assert!(result.unwrap_err().to_string().contains("503"));
            assert_eq!(server.await.unwrap(), ["/get"]);
        }
    }

    #[tokio::test]
    async fn a_busy_lrclib_is_asked_again_when_it_says() {
        let found = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]";
        let (api, server) = lrclib(vec![LIMITED, LIMITED, found]).await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let answer = get_from::<Vec<Record>>(&http, &api, "/search", &[]).await;
        assert!(matches!(answer, Ok(Some(records)) if records.is_empty()));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_lrclib_that_stays_busy_is_an_error() {
        let answers = vec![LIMITED; RATE_LIMIT_RETRIES as usize + 1];
        let (api, server) = lrclib(answers).await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let answer = get_from::<Vec<Record>>(&http, &api, "/search", &[]).await;
        let error = answer.expect_err("still limited");
        assert!(error.to_string().contains("429"), "{error}");
        server.await.unwrap();
    }

    #[test]
    fn a_featuring_is_cut_where_it_starts_whatever_the_case_costs() {
        // The cut is found in a lower-cased copy and applied to the original,
        // and lower casing is not always the same number of bytes. Turkish
        // 'İ' grows by one byte, so every offset past it points one byte too
        // far into the title, and the letters between are kept.
        assert_eq!(
            clean_title("\u{130}\u{130} feat. Someone"),
            "\u{130}\u{130}"
        );
        assert_eq!(clean_artist("\u{130}zel feat. Someone"), "\u{130}zel");

        // Capital sharp s shrinks by one, so the offset points one byte SHORT
        // -- and one byte short of the end of a character is a panic.
        assert_eq!(
            clean_title("\u{1e9e} caf\u{e9} feat. Someone"),
            "\u{1e9e} caf\u{e9}"
        );

        // The ASCII case, which already worked, still does.
        assert_eq!(clean_title("Song feat. Someone"), "Song");
        assert_eq!(clean_title("Song ft. Someone"), "Song");
        assert_eq!(clean_artist("Artist featuring Other"), "Artist");
    }

    #[test]
    fn titles_lose_what_a_database_leaves_out() {
        assert_eq!(clean_title("Song (Remastered 2011)"), "Song");
        assert_eq!(clean_title("Song - Live at Wembley"), "Song");
        assert_eq!(clean_title("Song - 2009 Remaster"), "Song");
        assert_eq!(clean_title("Song (feat. Someone)"), "Song");
        assert_eq!(clean_title("Song feat. Someone"), "Song");
        assert_eq!(clean_title("Song ft. Someone"), "Song");
        assert_eq!(clean_title("Song (Part One)"), "Song (Part One)");
        assert_eq!(clean_title("Hyphen - Ated"), "Hyphen - Ated");
        assert_eq!(clean_title("(Remastered)"), "(Remastered)", "never empty");
        assert_eq!(clean_title("Feat\u{200b}ure"), "Feature");
    }

    #[test]
    fn artists_keep_their_first_name() {
        assert_eq!(clean_artist("TOOL;Tool"), "TOOL");
        assert_eq!(clean_artist("Artist feat. Guest"), "Artist");
        assert_eq!(clean_artist("Artist (feat. Guest)"), "Artist");
        assert_eq!(clean_artist("Beyoncé"), "Beyoncé");
    }

    #[test]
    fn matching_is_loose_about_case_accents_and_punctuation() {
        assert!(loose_match("Beyoncé", "beyonce"));
        assert!(loose_match("Rock & Roll", "rock and roll"));
        assert!(loose_match("Don't Stop", "dont stop"));
        assert!(loose_match("Song (Live)", "Song"));
        assert!(!loose_match("Something", "Else"));
        assert!(!loose_match("", "Else"));
    }

    fn record(track: &str, artist: &str, duration: f64, synced: bool) -> Record {
        Record {
            track_name: track.into(),
            artist_name: artist.into(),
            duration: Some(duration),
            instrumental: false,
            synced_lyrics: synced.then(|| "[00:01.00] a".to_string()),
            plain_lyrics: Some("a".to_string()),
        }
    }

    #[test]
    fn the_closest_length_wins_and_synced_breaks_ties() {
        let query = Query {
            artist: "Artist".into(),
            title: "Song".into(),
            album: String::new(),
            duration_ms: 200_000,
        };
        let picked = pick(
            vec![
                record("Song", "Artist", 260.0, true),
                record("Song", "Artist", 201.0, false),
                record("Song", "Artist", 202.0, true),
                record("Other", "Artist", 200.0, true),
            ],
            &query,
        )
        .unwrap();
        assert_eq!(picked.duration, Some(202.0));
        assert!(pick(vec![record("Song", "Artist", 400.0, true)], &query).is_none());
    }

    #[test]
    fn lrc_lines_parse_and_sort() {
        let lines = parse_lrc(
            "[ar:Someone]\n[00:12.50]First\n[00:05]Early\n[01:00.1][02:00.123]Twice\n\nNo stamp\n",
        );
        let times: Vec<u32> = lines.iter().filter_map(|line| line.at_ms).collect();
        assert_eq!(times, vec![5_000, 12_500, 60_100, 120_123]);
        assert_eq!(lines[0].text, "Early");
        assert_eq!(lines[2].text, "Twice");
        assert_eq!(lines[3].text, "Twice");
    }

    #[test]
    fn the_active_line_is_the_last_one_started() {
        let lyrics = Lyrics {
            lines: parse_lrc("[00:05]a\n[00:10]b\n[00:15]c"),
            synced: true,
            instrumental: false,
            attribution: None,
        };
        assert_eq!(lyrics.active_line(0), None);
        assert_eq!(lyrics.active_line(5_000), Some(0));
        assert_eq!(lyrics.active_line(12_000), Some(1));
        assert_eq!(lyrics.active_line(99_000), Some(2));
        let plain = Lyrics {
            lines: vec![Line::default()],
            synced: false,
            instrumental: false,
            attribution: None,
        };
        assert_eq!(plain.active_line(5_000), None);
    }

    /// A line-synced source still lights its words one at a time: each word
    /// gets a moment between its line's start and the next line's.
    #[test]
    fn a_line_spreads_its_words_across_its_own_time() {
        let lyrics = Lyrics {
            lines: parse_lrc("[00:00.00]one two three\n[00:04.00]four\n[00:06.00]five"),
            synced: true,
            instrumental: false,
            attribution: None,
        };
        assert_eq!(lyrics.line_end(0, 10_000), Some(4_000));
        assert_eq!(lyrics.line_end(2, 12_000), Some(12_000));
        assert_eq!(lyrics.line_end(2, 0), Some(9_000), "no track length yet");
        let words = lyrics.words(0, 10_000);
        assert_eq!(words.len(), 3);
        assert_eq!(words[0], ("one", Some(0)));
        assert!((1..4_000).contains(&words[1].1.unwrap()));
        assert!((1..4_000).contains(&words[2].1.unwrap()));
        assert!(
            words.windows(2).all(|pair| pair[0].1 <= pair[1].1),
            "words keep their order: {words:?}"
        );
        let plain = Lyrics {
            lines: vec![Line {
                at_ms: None,
                text: "a b".into(),
                ..Line::default()
            }],
            synced: false,
            instrumental: false,
            attribution: None,
        };
        assert_eq!(plain.words(0, 1_000), [("a", None), ("b", None)]);
        assert!(lyrics.words(9, 1_000).is_empty());
    }

    #[test]
    fn spotify_syllables_keep_their_own_timings() {
        let json = serde_json::json!({
            "lyrics": {
                "syncType": "LINE_SYNCED",
                "lines": [{
                    "startTimeMs": "1000",
                    "words": "Hello world",
                    "syllables": [
                        {"startTimeMs": "1000", "endTimeMs": "2200", "words": "Hello "},
                        {"startTimeMs": "2200", "endTimeMs": "2600", "words": "world"}
                    ]
                }]
            }
        });
        let lyrics = from_spotify(&json).unwrap();
        assert_eq!(lyrics.lines[0].syllables.len(), 2);
        let units = lyrics.units(0, 5_000);
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].text, "Hello");
        assert_eq!(units[0].at_ms, 1_000);
        assert_eq!(units[0].end_ms, 2_200);
        assert!(units[0].letters, "a held word splits into letters");
        assert!(units[1].space_before);
        assert!(!units[1].letters);
    }

    #[test]
    fn other_word_sync_types_keep_their_timings() {
        let json = serde_json::json!({
            "lyrics": {
                "syncType": "SYLLABLE_SYNCED",
                "lines": [{
                    "startTimeMs": "1000",
                    "words": "Hello world",
                    "syllables": [
                        {"startTimeMs": "1000", "endTimeMs": "1600", "words": "Hello "},
                        {"startTimeMs": "1600", "endTimeMs": "2200", "words": "world"}
                    ]
                }]
            }
        });
        let lyrics = from_spotify(&json).unwrap();
        assert!(lyrics.synced, "a word-sync type is still synced");
        assert_eq!(lyrics.units(0, 4_000).len(), 2);
    }

    #[test]
    fn enhanced_lrc_keeps_word_timings() {
        let lines = parse_lrc("[00:01.00]<00:01.00>Hello <00:01.60>world");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Hello world");
        assert_eq!(lines[0].syllables.len(), 2);
        assert_eq!(lines[0].syllables[0].at_ms, 1_000);
        assert_eq!(lines[0].syllables[1].at_ms, 1_600);
        let lyrics = Lyrics {
            lines,
            synced: true,
            instrumental: false,
            attribution: None,
        };
        let units = lyrics.units(0, 4_000);
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].end_ms, 1_600, "the next stamp ends the word");
        assert!(units[1].space_before);
    }

    #[test]
    fn letters_get_an_equal_share_that_ends_with_the_voice() {
        let word = Word {
            at_ms: 0,
            end_ms: 2_000,
            text: "Hello".into(),
            space_before: true,
            letters: true,
            parts: Vec::new(),
        };
        let letters = Lyrics::letters(&word);
        assert_eq!(letters.len(), 5);
        assert_eq!(letters[0], ('H', 0, 350));
        assert_eq!(letters[4], ('o', 1_400, 1_750));
        assert!(letters[4].2 <= 2_000 - LETTER_TRIM_MS);
        let unsplit = Word {
            letters: false,
            ..word
        };
        assert!(
            Lyrics::letters(&unsplit)
                .iter()
                .all(|(_, at, end)| *at == 0 && *end == 2_000)
        );
    }

    #[test]
    fn syllable_parts_pace_the_letters_inside_their_own_spans() {
        let word = Word {
            at_ms: 1_000,
            end_ms: 2_000,
            text: "Hello".into(),
            space_before: false,
            letters: true,
            parts: vec![
                WordPart {
                    at_ms: 1_000,
                    end_ms: 1_800,
                    chars: 3,
                },
                WordPart {
                    at_ms: 1_800,
                    end_ms: 2_000,
                    chars: 2,
                },
            ],
        };
        let letters = Lyrics::letters(&word);
        assert_eq!(letters.len(), 5);
        assert_eq!(letters[0].1, 1_000);
        assert!(letters[2].1 < 1_800, "the first syllable owns its letters");
        assert_eq!(
            letters[3].1, 1_800,
            "the second syllable starts its own span"
        );
        assert_eq!(
            letters[4].2, 1_900,
            "the final letter is trimmed to the voice"
        );
    }

    #[test]
    fn a_line_synced_source_keeps_one_span_for_its_words() {
        let lyrics = Lyrics {
            lines: parse_lrc("[00:00.00]one two\n[00:06.00]three"),
            synced: true,
            instrumental: false,
            attribution: None,
        };
        let units = lyrics.units(0, 10_000);
        assert_eq!(units.len(), 2);
        assert!(
            units
                .iter()
                .all(|unit| unit.at_ms == 0 && unit.end_ms == 6_000),
            "the words share the line's own span"
        );
        assert!(
            units.iter().all(|unit| !unit.letters),
            "no invented letter timings"
        );
        assert!(!units[0].space_before);
        assert!(units[1].space_before);
    }

    #[test]
    fn old_cached_lyrics_load_without_word_timings() {
        let cached: Lyrics = serde_json::from_str(
            r#"{"lines":[{"at_ms":0,"text":"a line"}],"synced":true,"instrumental":false}"#,
        )
        .unwrap();
        assert!(cached.lines[0].syllables.is_empty());
        let units = cached.units(0, 2_000);
        assert_eq!(units.len(), 2);
        assert!(units.iter().all(|unit| !unit.letters));
    }

    #[test]
    fn records_turn_into_lyrics() {
        let mut instrumental = record("Song", "Artist", 100.0, false);
        instrumental.instrumental = true;
        assert!(instrumental.lyrics().unwrap().instrumental);
        let synced = record("Song", "Artist", 100.0, true).lyrics().unwrap();
        assert!(synced.synced);
        let plain = record("Song", "Artist", 100.0, false).lyrics().unwrap();
        assert!(!plain.synced);
        assert_eq!(plain.lines.len(), 1);
        let nothing = Record::default();
        assert!(nothing.lyrics().is_none());
    }
}
