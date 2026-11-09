'use strict';
// Album art for songs whose player shares no cover with Windows, looked up on the iTunes Search API.
const { candidates } = require('./lyrics');

const norm = (s = '') => s.toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim();
const words = (s) => new Set(norm(s).split(' ').filter(Boolean));
function overlap(a, b) {
  const x = words(a), y = words(b);
  if (!x.size || !y.size) return 0;
  let shared = 0;
  for (const w of x) if (y.has(w)) shared++;
  return shared / Math.min(x.size, y.size);
}

/** Returns a data: URL with the cover, or null. */
async function findCover(track) {
  for (const want of candidates(track)) {
    const found = await search(want);
    if (found) return found;
  }
  return null;
}

async function search(want) {
  const term = `${want.artist} ${want.title}`.trim();
  const res = await fetch(`https://itunes.apple.com/search?${new URLSearchParams({ term, entity: 'song', limit: '10' })}`, { signal: AbortSignal.timeout(8000) });
  if (!res.ok) return null;
  const { results = [] } = await res.json();
  let top = null, topScore = 0.99;
  for (const r of results) {
    const s = overlap(r.trackName, want.title) * 2 + (want.artist ? overlap(r.artistName, want.artist) : 1);
    if (s > topScore) { top = r; topScore = s; }
  }
  if (!top?.artworkUrl100) return null;
  const img = await fetch(top.artworkUrl100.replace(/100x100bb/, '300x300bb'), { signal: AbortSignal.timeout(8000) });
  if (!img.ok) return null;
  const type = img.headers.get('content-type') || 'image/jpeg';
  return `data:${type};base64,${Buffer.from(await img.arrayBuffer()).toString('base64')}`;
}

module.exports = { findCover };
