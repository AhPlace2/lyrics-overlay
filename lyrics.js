'use strict';
// Finds time-synced lyrics on LRCLIB (https://lrclib.net), a free lyrics database.

const BASE = 'https://lrclib.net/api';
const USER_AGENT = 'lyrics-overlay/0.1 (personal desktop prototype)';

const JUNK = /\s*[([][^)\]]*\b(official|video|audio|lyrics?|visuali[sz]er|hd|4k|mv|m\/v|remaster(ed)?|explicit|clean|color coded)\b[^)\]]*[)\]]/gi;

function cleanTitle(title = '') {
  return title
    .replace(JUNK, '')
    .replace(/\s*[([](feat|ft|with)\.?\s[^)\]]*[)\]]/gi, '')
    .replace(/\s+[-–—]\s+(radio edit|radio version|single version|original mix|extended mix|remaster(ed)?(\s+\d{4})?|\d{4}\s+remaster(ed)?|mono|stereo)\s*$/i, '')
    .replace(/\s+(feat|ft)\.?\s+.*$/i, '')
    .trim();
}

function cleanArtist(artist = '') {
  return artist
    .replace(/\s*-\s*Topic$/i, '')
    .replace(/VEVO$/i, '')
    .replace(/\s*Official$/i, '')
    .split(/\s*(?:,|&|\bx\b|\bfeat\.?|\bft\.?|\bwith\b)\s*/i)[0]
    .trim();
}

function looksLikeChannel(artist = '') {
  return !artist || /vevo$|- topic$|records|official|music$|tv$/i.test(artist);
}

/** Ways to read a now-playing title. Browsers often report "Artist - Song" with the channel as artist. */
function candidates({ title = '', artist = '' }) {
  const list = [];
  const t = cleanTitle(title), a = cleanArtist(artist);
  const split = t.match(/^(.+?)\s+[-–—]\s+(.+)$/);
  const fromSplit = split ? { artist: cleanArtist(split[1]), title: cleanTitle(split[2]) } : null;
  if (fromSplit && looksLikeChannel(artist)) list.push(fromSplit);
  if (t) list.push({ artist: a, title: t });
  if (fromSplit && !looksLikeChannel(artist)) list.push(fromSplit);
  const seen = new Set();
  return list.filter((c) => {
    const k = `${c.artist}|${c.title}`.toLowerCase();
    if (seen.has(k) || !c.title) return false;
    seen.add(k);
    return true;
  });
}

function parseLRC(lrc = '') {
  const lines = [];
  for (const raw of lrc.split(/\r?\n/)) {
    const tags = [...raw.matchAll(/\[(\d{1,3}):(\d{1,2}(?:[.:]\d{1,3})?)\]/g)];
    if (!tags.length) continue;
    const text = raw.replace(/\[[^\]]*\]/g, '').replace(/<\d+:\d+(?:[.:]\d+)?>/g, '').replace(/\s+/g, ' ').trim();
    for (const m of tags) lines.push({ t: Number(m[1]) * 60 + parseFloat(m[2].replace(':', '.')), text });
  }
  return lines.sort((a, b) => a.t - b.t);
}

const norm = (s = '') => s.normalize('NFKD').replace(/[̀-ͯ]/g, '').toLowerCase().replace(/[^a-z0-9Ѐ-ӿ؀-ۿ぀-ヿ一-鿿]+/g, ' ').trim();

function similarity(a, b) {
  const x = new Set(norm(a).split(' ').filter(Boolean)), y = new Set(norm(b).split(' ').filter(Boolean));
  if (!x.size || !y.size) return 0;
  let shared = 0;
  for (const w of x) if (y.has(w)) shared++;
  return shared / Math.max(x.size, y.size);
}

function score(result, want, duration) {
  if (!result || !result.syncedLyrics) return -Infinity;
  let s = similarity(result.trackName, want.title) * 10 + similarity(result.artistName, want.artist) * 6;
  if (duration > 0 && result.duration) {
    const off = Math.abs(result.duration - duration);
    if (off > 8) return -Infinity;
    s -= off * .5;
  }
  return s;
}

async function request(path, params) {
  const res = await fetch(`${BASE}${path}?${new URLSearchParams(params)}`, {
    headers: { 'User-Agent': USER_AGENT },
    signal: AbortSignal.timeout(10000),
  });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`LRCLIB answered ${res.status}`);
  return res.json();
}

function best(results, want, duration) {
  let top = null, topScore = 4;
  for (const r of results || []) {
    const s = score(r, want, duration);
    if (s > topScore) { top = r; topScore = s; }
  }
  return top;
}

/**
 * Community timings often stamp a song's first line far too early, so it sits on screen through
 * the intro. When that line waits much longer than the song's usual line length, show it only
 * shortly before the next line instead. Later lines are left alone: long gaps there are usually real.
 */
function delayEarlyStarts(lines) {
  const spans = [];
  for (let i = 0; i < lines.length - 1; i++) if (lines[i].text) spans.push(lines[i + 1].t - lines[i].t);
  if (spans.length < 4) return lines;
  const median = [...spans].sort((a, b) => a - b)[spans.length >> 1];
  const first = lines.findIndex((l) => l.text);
  const line = lines[first], next = lines[first + 1];
  if (!next || next.t - line.t <= 8 || next.t - line.t <= median * 2.5) return lines;
  const needed = Math.min(6, Math.max(2.5, line.text.split(' ').length * .42 + 1.4));
  const out = [...lines];
  out[first] = { ...line, t: Math.max(line.t, next.t - needed), stamped: line.t };
  return out;
}

/**
 * Uploads often name the uploader instead of the artist (a channel, or whoever made the mashup), so
 * nothing matches by artist. As a last try, match on the title alone, but only when the length agrees
 * to within 3 seconds, which keeps common titles like "Intro" from matching the wrong song.
 */
function titleOnlyMatches(results, title, duration) {
  if (!(duration > 0)) return [];
  return (results || [])
    .filter((r) => r.syncedLyrics && r.duration && similarity(r.trackName, title) >= .8 && Math.abs(r.duration - duration) <= 3)
    .sort((a, b) => Math.abs(a.duration - duration) - Math.abs(b.duration - duration));
}

const found = (r) => ({ lines: delayEarlyStarts(parseLRC(r.syncedLyrics)), source: `${r.artistName} – ${r.trackName}`, pick: `lrclib:${r.id}`, provider: 'LRCLIB' });

/**
 * Returns { lines, source } with synced lyrics, { plainOnly: true } when only unsynced text exists,
 * or null when nothing was found.
 */
async function findLyrics(track) {
  const duration = track.duration > 0 ? track.duration : 0;
  let plainOnly = false;
  const list = candidates(track);
  for (const c of list) {
    if (c.artist && duration) {
      const exact = await request('/get', { track_name: c.title, artist_name: c.artist, duration: Math.round(duration) });
      if (exact?.syncedLyrics) return found(exact);
      if (exact?.plainLyrics) plainOnly = true;
    }
    const params = c.artist ? { track_name: c.title, artist_name: c.artist } : { q: c.title };
    const pick = best(await request('/search', params), c, duration);
    if (pick) return found(pick);
  }
  if (list[0]) {
    const pick = best(await request('/search', { q: `${list[0].artist} ${list[0].title}`.trim() }), list[0], duration);
    if (pick) return found(pick);
  }
  if (duration) {
    for (const title of new Set(list.map((c) => c.title))) {
      const [pick] = titleOnlyMatches(await request('/search', { q: title }), title, duration);
      if (pick) return found(pick);
    }
  }
  return plainOnly ? { plainOnly: true } : null;
}

/* ---------- Other versions, for when the first pick is badly timed ---------- */

/** True when two lyric versions are timed the same, so switching between them would change nothing. */
function sameTiming(a, b) {
  const x = a.filter((l) => l.text), y = b.filter((l) => l.text);
  if (Math.abs(x.length - y.length) > 2) return false;
  for (let i = 0; i < Math.min(6, x.length, y.length); i++) if (Math.abs(x[i].t - y[i].t) > .4) return false;
  return true;
}

async function lrclibVersions(track, duration) {
  const out = [];
  for (const c of candidates(track)) {
    const params = c.artist ? { track_name: c.title, artist_name: c.artist } : { q: c.title };
    const results = (await request('/search', params)) || [];
    for (const r of results) if (score(r, c, duration) > 4) out.push({ r, s: score(r, c, duration) });
    // Title-and-length matches come last, after any version whose artist matched too.
    for (const r of titleOnlyMatches(await request('/search', { q: c.title }), c.title, duration)) {
      out.push({ r, s: 2 - Math.abs(r.duration - duration) * .1 });
    }
  }
  return out.sort((a, b) => b.s - a.s).map(({ r }) => found(r));
}

// NetEase Cloud Music has a large catalogue of timed lyrics, made separately from LRCLIB's.
const NETEASE = 'https://music.163.com/api';
const CREDIT = /^(作词|作曲|编曲|制作人|制作|混音|母带|录音|和声|监制|出品|发行|词|曲|lyricist|lyrics by|composer|arranger|producer)\s*[:：]/i;

async function netease(path, params) {
  const res = await fetch(`${NETEASE}${path}?${new URLSearchParams(params)}`, {
    headers: { 'User-Agent': 'Mozilla/5.0', Referer: 'https://music.163.com/' },
    signal: AbortSignal.timeout(10000),
  });
  if (!res.ok) throw new Error(`NetEase answered ${res.status}`);
  return res.json();
}

async function neteaseVersions(track, duration) {
  const songs = [];
  for (const c of candidates(track).slice(0, 2)) {
    const data = await netease('/search/get', { s: `${c.artist} ${c.title}`.trim(), type: 1, limit: 10 });
    for (const song of data?.result?.songs || []) {
      if (duration > 0 && song.duration && Math.abs(song.duration / 1000 - duration) > 8) continue;
      const titleMatch = similarity(song.name, c.title);
      const s = titleMatch * 10 + similarity((song.artists || []).map((a) => a.name).join(' '), c.artist) * 6;
      if (titleMatch >= .5 && s > 4 && !songs.some((x) => x.song.id === song.id)) songs.push({ song, s });
    }
  }
  const out = [];
  for (const { song } of songs.sort((a, b) => b.s - a.s).slice(0, 3)) {
    const lrc = (await netease('/song/lyric', { id: song.id, lv: 1 }))?.lrc?.lyric;
    const lines = parseLRC(lrc).filter((l) => !CREDIT.test(l.text));
    if (lines.filter((l) => l.text).length < 4 || lines.every((l) => l.t === 0)) continue;
    out.push({ lines: delayEarlyStarts(lines), source: `${(song.artists || []).map((a) => a.name).join(', ')} – ${song.name}`, pick: `netease:${song.id}`, provider: 'NetEase' });
  }
  return out;
}

/** Every distinct timed version of a song we can find, LRCLIB's first. Same-timed duplicates are dropped. */
async function findVersions(track) {
  const duration = track.duration > 0 ? track.duration : 0;
  const all = [];
  for (const source of [lrclibVersions, neteaseVersions]) {
    try { all.push(...(await source(track, duration))); }
    catch (err) { console.error('Lyrics source failed:', err.message); }
  }
  const out = [];
  for (const v of all) if (!out.some((o) => o.pick === v.pick || sameTiming(o.lines, v.lines))) out.push(v);
  return out;
}

module.exports = { findLyrics, findVersions, sameTiming, parseLRC, delayEarlyStarts, candidates, cleanTitle, cleanArtist };
