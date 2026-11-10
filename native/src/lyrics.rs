// Finds time-synced lyrics on LRCLIB (https://lrclib.net), a free lyrics database, and on NetEase Cloud Music when
// LRCLIB has none. A line-for-line port of src/main/lyrics.js.

use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Duration;
use unicode_normalization::UnicodeNormalization;

const BASE: &str = "https://lrclib.net/api";
const USER_AGENT: &str = "lyrics-overlay/0.1 (personal desktop prototype)";

/// One HTTP client for every lookup: 4xx/5xx come back as answers (not errors), each request gives up after 10 s.
pub static HTTP: LazyLock<ureq::Agent> = LazyLock::new(|| agent(10));

pub fn agent(seconds: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(seconds)))
        .build()
        .into()
}

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).unwrap());
    };
}

re!(JUNK, r"(?i)\s*[(\[][^)\]]*\b(official|video|audio|lyrics?|visuali[sz]er|hd|4k|mv|m/v|remaster(ed)?|explicit|clean|color coded)\b[^)\]]*[)\]]");
re!(FEAT_BRACKET, r"(?i)\s*[(\[](feat|ft|with)\.?\s[^)\]]*[)\]]");
re!(EDIT_SUFFIX, r"(?i)\s+[-–—]\s+(radio edit|radio version|single version|original mix|extended mix|remaster(ed)?(\s+\d{4})?|\d{4}\s+remaster(ed)?|mono|stereo)\s*$");
re!(FEAT_TAIL, r"(?i)\s+(feat|ft)\.?\s+.*$");
re!(TOPIC, r"(?i)\s*-\s*Topic$");
re!(VEVO, r"(?i)VEVO$");
re!(OFFICIAL, r"(?i)\s*Official$");
re!(ARTIST_SPLIT, r"(?i)\s*(?:,|&|\bx\b|\bfeat\.?|\bft\.?|\bwith\b)\s*");
re!(CHANNEL, r"(?i)vevo$|- topic$|records|official|music$|tv$");
re!(DASH_SPLIT, r"^(.+?)\s+[-–—]\s+(.+)$");
re!(DASH_INSIDE, r"\s[-–—]\s");
re!(LRC_TAG, r"\[(\d{1,3}):(\d{1,2}(?:[.:]\d{1,3})?)\]");
re!(ANY_TAG, r"\[[^\]]*\]");
re!(WORD_TIME, r"<\d+:\d+(?:[.:]\d+)?>");
re!(SPACES, r"\s+");
re!(MARKS, "[\u{0300}-\u{036f}]");
re!(NOT_WORD, "[^a-z0-9\u{0400}-\u{04ff}\u{0600}-\u{06ff}\u{3040}-\u{30ff}\u{4e00}-\u{9fff}]+");
re!(BRACKETS, r"\s*[(\[][^)\]]*[)\]]");
re!(CREDIT, r"(?i)^(作词|作曲|编曲|制作人|制作|混音|母带|录音|和声|监制|出品|发行|词|曲|lyricist|lyrics by|composer|arranger|producer)\s*[:：]");

pub fn clean_title(title: &str) -> String {
    let s = JUNK.replace_all(title, "");
    let s = FEAT_BRACKET.replace_all(&s, "");
    let s = EDIT_SUFFIX.replace(&s, "");
    let s = FEAT_TAIL.replace(&s, "");
    s.trim().to_string()
}

pub fn clean_artist(artist: &str) -> String {
    let s = TOPIC.replace(artist, "");
    let s = VEVO.replace(&s, "");
    let s = OFFICIAL.replace(&s, "");
    ARTIST_SPLIT.split(&s).next().unwrap_or("").trim().to_string()
}

fn looks_like_channel(artist: &str) -> bool {
    artist.is_empty() || CHANNEL.is_match(artist)
}

#[derive(Clone, Debug)]
pub struct Want {
    pub artist: String,
    pub title: String,
}

/// Ways to read a now-playing title. Browsers often report "Artist - Song" with the channel as artist.
pub fn candidates(title: &str, artist: &str) -> Vec<Want> {
    let mut list = Vec::new();
    let t = clean_title(title);
    let a = clean_artist(artist);
    let from_split = DASH_SPLIT.captures(&t).map(|m| Want { artist: clean_artist(&m[1]), title: clean_title(&m[2]) });
    if let Some(f) = &from_split {
        if looks_like_channel(artist) {
            list.push(f.clone());
        }
    }
    if !t.is_empty() {
        list.push(Want { artist: a, title: t.clone() });
    }
    if let Some(f) = from_split {
        if !looks_like_channel(artist) {
            list.push(f);
        }
    }
    let mut seen = HashSet::new();
    list.into_iter()
        .filter(|c| {
            let k = format!("{}|{}", c.artist, c.title).to_lowercase();
            if c.title.is_empty() || seen.contains(&k) {
                return false;
            }
            seen.insert(k);
            true
        })
        .collect()
}

pub fn parse_lrc(lrc: &str) -> Vec<Value> {
    let mut lines: Vec<(f64, String)> = Vec::new();
    for raw in lrc.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let tags: Vec<_> = LRC_TAG.captures_iter(raw).collect();
        if tags.is_empty() {
            continue;
        }
        let text = ANY_TAG.replace_all(raw, "");
        let text = WORD_TIME.replace_all(&text, "");
        let text = SPACES.replace_all(&text, " ").trim().to_string();
        for m in tags {
            let min: f64 = m[1].parse().unwrap_or(0.0);
            let sec: f64 = m[2].replacen(':', ".", 1).parse().unwrap_or(0.0);
            lines.push((min * 60.0 + sec, text.clone()));
        }
    }
    lines.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    lines.into_iter().map(|(t, text)| json!({ "t": t, "text": text })).collect()
}

fn norm(s: &str) -> String {
    let s: String = s.nfkd().collect();
    let s = MARKS.replace_all(&s, "").to_lowercase();
    NOT_WORD.replace_all(&s, " ").trim().to_string()
}

pub fn similarity(a: &str, b: &str) -> f64 {
    let x: HashSet<String> = norm(a).split(' ').filter(|w| !w.is_empty()).map(String::from).collect();
    let y: HashSet<String> = norm(b).split(' ').filter(|w| !w.is_empty()).map(String::from).collect();
    if x.is_empty() || y.is_empty() {
        return 0.0;
    }
    let shared = x.iter().filter(|w| y.contains(*w)).count();
    shared as f64 / x.len().max(y.len()) as f64
}

pub fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}
fn num_of(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}
/// A JSON id (a number or a string) as text.
pub fn id_of(v: &Value) -> String {
    match v.get("id") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

fn score(r: &Value, want: &Want, duration: f64) -> f64 {
    if str_of(r, "syncedLyrics").is_empty() {
        return f64::NEG_INFINITY;
    }
    let mut s = similarity(str_of(r, "trackName"), &want.title) * 10.0 + similarity(str_of(r, "artistName"), &want.artist) * 6.0;
    let rd = num_of(r, "duration");
    if duration > 0.0 && rd != 0.0 {
        let off = (rd - duration).abs();
        if off > 8.0 {
            return f64::NEG_INFINITY;
        }
        s -= off * 0.5;
    }
    s
}

fn request(path: &str, params: &[(&str, String)]) -> Result<Value, String> {
    let mut res = HTTP
        .get(format!("{BASE}{path}"))
        .query_pairs(params.iter().map(|(k, v)| (*k, v.as_str())))
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| e.to_string())?;
    let status = res.status().as_u16();
    if status == 404 {
        return Ok(Value::Null);
    }
    if !(200..300).contains(&status) {
        return Err(format!("LRCLIB answered {status}"));
    }
    res.body_mut().read_json().map_err(|e| e.to_string())
}

fn results(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

fn best(results: &[Value], want: &Want, duration: f64) -> Option<Value> {
    let mut top = None;
    let mut top_score = 4.0;
    for r in results {
        let s = score(r, want, duration);
        if s > top_score {
            top = Some(r.clone());
            top_score = s;
        }
    }
    top
}

/// Community timings often stamp a song's first line far too early, so it sits on screen through the intro. When
/// that line waits much longer than the song's usual line length, show it only shortly before the next line instead.
fn delay_early_starts(lines: Vec<Value>) -> Vec<Value> {
    let t = |l: &Value| num_of(l, "t");
    let mut spans = Vec::new();
    for i in 0..lines.len().saturating_sub(1) {
        if !str_of(&lines[i], "text").is_empty() {
            spans.push(t(&lines[i + 1]) - t(&lines[i]));
        }
    }
    if spans.len() < 4 {
        return lines;
    }
    let mut sorted = spans.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() >> 1];
    let Some(first) = lines.iter().position(|l| !str_of(l, "text").is_empty()) else { return lines };
    let Some(next) = lines.get(first + 1) else { return lines };
    let line = &lines[first];
    let gap = t(next) - t(line);
    if gap <= 8.0 || gap <= median * 2.5 {
        return lines;
    }
    let words = str_of(line, "text").split(' ').count() as f64;
    let needed = (words * 0.42 + 1.4).max(2.5).min(6.0);
    let mut out = lines.clone();
    out[first] = json!({ "t": t(line).max(t(next) - needed), "text": str_of(line, "text"), "stamped": t(line) });
    out
}

/// Uploads often name the uploader instead of the artist, so nothing matches by artist. As a last try, match on the
/// title alone, but only when the length agrees to within 3 seconds.
fn title_only_matches(results: &[Value], title: &str, duration: f64) -> Vec<Value> {
    if !(duration > 0.0) {
        return Vec::new();
    }
    let bare = |s: &str| BRACKETS.replace_all(s, "").trim().to_string();
    let full = |r: &Value| similarity(str_of(r, "trackName"), title).max(similarity(str_of(r, "albumName"), title));
    let mut out: Vec<Value> = results
        .iter()
        .filter(|r| {
            let d = num_of(r, "duration");
            !str_of(r, "syncedLyrics").is_empty()
                && d != 0.0
                && (d - duration).abs() <= 3.0
                && (full(r) >= 0.8 || similarity(&bare(str_of(r, "trackName")), &bare(title)) >= 0.8)
        })
        .cloned()
        .collect();
    out.sort_by(|a, b| {
        let by_full = full(b) - full(a);
        let v = if by_full != 0.0 { by_full } else { (num_of(a, "duration") - duration).abs() - (num_of(b, "duration") - duration).abs() };
        v.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

/// The same readings with artist and title swapped, for titles written "Song - Artist" instead of "Artist - Song".
fn swapped(list: &[Want]) -> Vec<Want> {
    let have: HashSet<String> = list.iter().map(|c| format!("{}|{}", c.artist, c.title).to_lowercase()).collect();
    list.iter()
        .filter(|c| !c.artist.is_empty() && !DASH_INSIDE.is_match(&c.title))
        .map(|c| Want { artist: c.title.clone(), title: c.artist.clone() })
        .filter(|c| !have.contains(&format!("{}|{}", c.artist, c.title).to_lowercase()))
        .collect()
}

fn found(r: &Value) -> Value {
    json!({
        "lines": delay_early_starts(parse_lrc(str_of(r, "syncedLyrics"))),
        "source": format!("{} – {}", str_of(r, "artistName"), str_of(r, "trackName")),
        "pick": format!("lrclib:{}", id_of(r)),
        "provider": "LRCLIB",
    })
}

pub struct Track {
    pub title: String,
    pub artist: String,
    pub duration: f64,
}

/// Synced lyrics as { lines, source, pick, provider }; `plain_only` when only unsynced text exists.
pub struct Found {
    pub version: Option<Value>,
    pub plain_only: bool,
}

/// LRCLIB first; when it has no timed lyrics (or can't be reached), NetEase's best match.
pub fn find_lyrics(track: &Track) -> Result<Found, String> {
    let duration = if track.duration > 0.0 { track.duration } else { 0.0 };
    let (from_lrclib, lrclib_error) = match lrclib_lyrics(track, duration) {
        Ok(f) => (Some(f), None),
        Err(e) => (None, Some(e)),
    };
    if let Some(Found { version: Some(v), .. }) = &from_lrclib {
        return Ok(Found { version: Some(v.clone()), plain_only: false });
    }
    match netease_versions(track, duration) {
        Ok(list) => {
            if let Some(first) = list.into_iter().next() {
                return Ok(Found { version: Some(first), plain_only: false });
            }
        }
        Err(e) => eprintln!("NetEase lookup failed: {e}"),
    }
    if let Some(e) = lrclib_error {
        return Err(e);
    }
    Ok(from_lrclib.unwrap_or(Found { version: None, plain_only: false }))
}

fn lrclib_lyrics(track: &Track, duration: f64) -> Result<Found, String> {
    let mut plain_only = false;
    let list = candidates(&track.title, &track.artist);
    let dur = || duration.round().to_string();
    for c in &list {
        if !c.artist.is_empty() && duration != 0.0 {
            let exact = request("/get", &[("track_name", c.title.clone()), ("artist_name", c.artist.clone()), ("duration", dur())])?;
            if !str_of(&exact, "syncedLyrics").is_empty() {
                return Ok(Found { version: Some(found(&exact)), plain_only: false });
            }
            if !str_of(&exact, "plainLyrics").is_empty() {
                plain_only = true;
            }
        }
        let params: Vec<(&str, String)> = if c.artist.is_empty() {
            vec![("q", c.title.clone())]
        } else {
            vec![("track_name", c.title.clone()), ("artist_name", c.artist.clone())]
        };
        if let Some(pick) = best(&results(&request("/search", &params)?), c, duration) {
            return Ok(Found { version: Some(found(&pick)), plain_only: false });
        }
    }
    if let Some(first) = list.first() {
        let q = format!("{} {}", first.artist, first.title).trim().to_string();
        if let Some(pick) = best(&results(&request("/search", &[("q", q)])?), first, duration) {
            return Ok(Found { version: Some(found(&pick)), plain_only: false });
        }
    }
    for c in swapped(&list) {
        if duration != 0.0 {
            let exact = request("/get", &[("track_name", c.title.clone()), ("artist_name", c.artist.clone()), ("duration", dur())])?;
            if !str_of(&exact, "syncedLyrics").is_empty() {
                return Ok(Found { version: Some(found(&exact)), plain_only: false });
            }
        }
        let res = request("/search", &[("track_name", c.title.clone()), ("artist_name", c.artist.clone())])?;
        if let Some(pick) = best(&results(&res), &c, duration) {
            return Ok(Found { version: Some(found(&pick)), plain_only: false });
        }
    }
    if duration != 0.0 {
        let mut titles: Vec<String> = Vec::new();
        for c in &list {
            if !titles.contains(&c.title) {
                titles.push(c.title.clone());
            }
        }
        for title in titles {
            let res = request("/search", &[("q", title.clone())])?;
            if let Some(pick) = title_only_matches(&results(&res), &title, duration).into_iter().next() {
                return Ok(Found { version: Some(found(&pick)), plain_only: false });
            }
        }
    }
    Ok(Found { version: None, plain_only })
}

/* ---------- Other versions, for when the first pick is badly timed ---------- */

/// True when two lyric versions are timed the same, so switching between them would change nothing.
pub fn same_timing(a: &[Value], b: &[Value]) -> bool {
    let x: Vec<&Value> = a.iter().filter(|l| !str_of(l, "text").is_empty()).collect();
    let y: Vec<&Value> = b.iter().filter(|l| !str_of(l, "text").is_empty()).collect();
    if (x.len() as i64 - y.len() as i64).abs() > 2 {
        return false;
    }
    for i in 0..6.min(x.len()).min(y.len()) {
        if (num_of(x[i], "t") - num_of(y[i], "t")).abs() > 0.4 {
            return false;
        }
    }
    true
}

pub fn lines_of(v: &Value) -> Vec<Value> {
    v.get("lines").and_then(Value::as_array).cloned().unwrap_or_default()
}

fn lrclib_versions(track: &Track, duration: f64) -> Result<Vec<Value>, String> {
    let mut out: Vec<(Value, f64)> = Vec::new();
    let list = candidates(&track.title, &track.artist);
    for c in swapped(&list) {
        let res = results(&request("/search", &[("track_name", c.title.clone()), ("artist_name", c.artist.clone())])?);
        for r in res {
            let s = score(&r, &c, duration);
            if s > 4.0 {
                out.push((r, s - 1.0));
            }
        }
    }
    for c in &list {
        let params: Vec<(&str, String)> = if c.artist.is_empty() {
            vec![("q", c.title.clone())]
        } else {
            vec![("track_name", c.title.clone()), ("artist_name", c.artist.clone())]
        };
        for r in results(&request("/search", &params)?) {
            let s = score(&r, c, duration);
            if s > 4.0 {
                out.push((r, s));
            }
        }
        // Title-and-length matches come last, after any version whose artist matched too.
        let res = results(&request("/search", &[("q", c.title.clone())])?);
        for r in title_only_matches(&res, &c.title, duration) {
            let s = 2.0 - (num_of(&r, "duration") - duration).abs() * 0.1;
            out.push((r, s));
        }
    }
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out.iter().map(|(r, _)| found(r)).collect())
}

// NetEase Cloud Music has a large catalogue of timed lyrics, made separately from LRCLIB's.
const NETEASE: &str = "https://music.163.com/api";

fn netease(path: &str, params: &[(&str, String)]) -> Result<Value, String> {
    let mut res = HTTP
        .get(format!("{NETEASE}{path}"))
        .query_pairs(params.iter().map(|(k, v)| (*k, v.as_str())))
        .header("User-Agent", "Mozilla/5.0")
        .header("Referer", "https://music.163.com/")
        .call()
        .map_err(|e| e.to_string())?;
    let status = res.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(format!("NetEase answered {status}"));
    }
    // NetEase sometimes labels JSON as text/plain, so parse the text.
    let text = res.body_mut().read_to_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn artists_of(song: &Value) -> Vec<String> {
    song.get("artists")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|x| str_of(x, "name").to_string()).collect())
        .unwrap_or_default()
}

fn netease_versions(track: &Track, duration: f64) -> Result<Vec<Value>, String> {
    let mut songs: Vec<(Value, f64)> = Vec::new();
    for c in candidates(&track.title, &track.artist).into_iter().take(2) {
        let s = format!("{} {}", c.artist, c.title).trim().to_string();
        let data = netease("/search/get", &[("s", s), ("type", "1".into()), ("limit", "10".into())])?;
        let list = data.pointer("/result/songs").and_then(Value::as_array).cloned().unwrap_or_default();
        for song in list {
            let d = num_of(&song, "duration");
            if duration > 0.0 && d != 0.0 && (d / 1000.0 - duration).abs() > 8.0 {
                continue;
            }
            let title_match = similarity(str_of(&song, "name"), &c.title);
            let s = title_match * 10.0 + similarity(&artists_of(&song).join(" "), &c.artist) * 6.0;
            if title_match >= 0.5 && s > 4.0 && !songs.iter().any(|(x, _)| id_of(x) == id_of(&song)) {
                songs.push((song, s));
            }
        }
    }
    songs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = Vec::new();
    for (song, _) in songs.into_iter().take(3) {
        let data = netease("/song/lyric", &[("id", id_of(&song)), ("lv", "1".into())])?;
        let lrc = data.pointer("/lrc/lyric").and_then(Value::as_str).unwrap_or("");
        let lines: Vec<Value> = parse_lrc(lrc).into_iter().filter(|l| !CREDIT.is_match(str_of(l, "text"))).collect();
        if lines.iter().filter(|l| !str_of(l, "text").is_empty()).count() < 4 || lines.iter().all(|l| num_of(l, "t") == 0.0) {
            continue;
        }
        out.push(json!({
            "lines": delay_early_starts(lines),
            "source": format!("{} – {}", artists_of(&song).join(", "), str_of(&song, "name")),
            "pick": format!("netease:{}", id_of(&song)),
            "provider": "NetEase",
        }));
    }
    Ok(out)
}

/// Every distinct timed version of a song we can find, LRCLIB's first. Same-timed duplicates are dropped.
pub fn find_versions(track: &Track) -> Vec<Value> {
    let duration = if track.duration > 0.0 { track.duration } else { 0.0 };
    let mut all = Vec::new();
    match lrclib_versions(track, duration) {
        Ok(v) => all.extend(v),
        Err(e) => eprintln!("Lyrics source failed: {e}"),
    }
    match netease_versions(track, duration) {
        Ok(v) => all.extend(v),
        Err(e) => eprintln!("Lyrics source failed: {e}"),
    }
    let mut out: Vec<Value> = Vec::new();
    for v in all {
        let dup = out.iter().any(|o| str_of(o, "pick") == str_of(&v, "pick") || same_timing(&lines_of(o), &lines_of(&v)));
        if !dup {
            out.push(v);
        }
    }
    out
}
