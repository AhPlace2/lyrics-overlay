'use strict';
const api = window.overlay;
const $ = (s) => document.querySelector(s);
const card = $('#card'), panel = $('#panel'), big = $('#big');

let settings = { enabled: true, showCard: true, showBig: true, effect: 'depth', font: 'clean', accent: '#ffb648', bigPosition: 'center', offsets: {}, card: { x: 24, y: 24 } };
let track = null, lyrics = { key: null, status: 'idle', lines: [] };
let anchorPos = 0, anchorMs = 0, playing = false, idleSince = Date.now();
let shownIdx = null, shownKey = null, panelSig = '', editing = false, lastCover = null;

const songKey = (s) => `${(s?.artist || '').toLowerCase()}|${(s?.title || '').toLowerCase()}`;
const songLead = () => (track && settings.offsets?.[songKey(track)]) || 0;

const fmt = (s) => {
  s = Math.max(0, Math.floor(s || 0));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
};

/* ---------- Playback clock ---------- */
function songTime() {
  const t = playing ? anchorPos + (Date.now() - anchorMs) / 1000 : anchorPos;
  const length = track ? track.end - track.start : 0;
  return length > 0 ? Math.min(t, length) : t;
}

function onState(s) {
  if (!s.active) {
    if (playing) idleSince = Date.now();
    track = null; playing = false;
    return;
  }
  const isNew = !track || track.key !== s.key;
  const now = Date.now();
  // Players stamp when they last reported the position, so the true position keeps moving from there.
  const stampOK = s.updated > now - 6 * 3600e3 && s.updated < now + 5000;
  const nowPlaying = s.status === 'Playing';
  const reported = Math.max(0, s.position - s.start + (nowPlaying && stampOK ? (now - s.updated) / 1000 : 0));
  const drift = Math.abs(reported - songTime());
  if (isNew || nowPlaying !== playing || drift > .35) { anchorPos = reported; anchorMs = now; }
  if (!nowPlaying && playing) idleSince = now;
  playing = nowPlaying;
  if (isNew) {
    // The cover can arrive just before the new song's details, so keep it rather than clearing it.
    if (lastCover?.key === s.key) $('#cover').src = lastCover.data;
    else $('#cover').removeAttribute('src');
    card.classList.remove('pop'); void card.offsetWidth; card.classList.add('pop');
  }
  track = s;
  $('#title').textContent = s.title || 'Unknown title';
  $('#artist').textContent = s.artist || '';
  $('#eq').classList.toggle('paused', !playing);
}

/* ---------- Lyrics ---------- */
function lineIndex(lines, t) {
  let lo = 0, hi = lines.length;
  while (lo < hi) { const m = (lo + hi) >> 1; if (lines[m].t <= t) lo = m + 1; else hi = m; }
  return lo - 1;
}

function nextTextIndex(lines, from) {
  for (let i = from; i < lines.length; i++) if (lines[i].text) return i;
  return -1;
}

/** `elapsed` is how far into the line we already are, so joining mid-line shows the words already sung. */
function showBigLine(text, duration, elapsed = 0) {
  for (const old of big.querySelectorAll('.line:not(.leaving)')) {
    old.classList.add('leaving');
    old.addEventListener('animationend', () => old.remove(), { once: true });
  }
  if (!text) return;
  const line = document.createElement('div');
  line.className = 'line';
  const words = text.split(' ');
  const step = Math.min(.25, Math.max(.05, (Math.min(duration, 8) * .45) / words.length));
  words.forEach((w, i) => {
    const word = document.createElement('span');
    word.className = 'word';
    word.style.animationDelay = `${Math.max(-.42, i * step - elapsed).toFixed(3)}s`;
    const inner = document.createElement('span');
    inner.className = 'inner';
    inner.style.setProperty('--i', i);
    inner.textContent = w;
    word.append(inner);
    line.append(word, document.createTextNode(' '));
  });
  big.append(line);
}

function renderPanel(lines, idx, t) {
  const status = lyrics.key === track?.key ? lyrics.status : 'loading';
  let sig, html;
  if (status === 'loading') { sig = 'loading'; html = '<div class="note">Looking for lyrics…</div>'; }
  else if (status === 'none') { sig = 'none'; html = '<div class="note">No synced lyrics found for this song</div>'; }
  else if (status === 'unsynced') { sig = 'unsynced'; html = '<div class="note">Only unsynced lyrics exist for this song</div>'; }
  else if (status === 'error') { sig = 'error'; html = '<div class="note">Couldn\'t reach the lyrics service. Use “Search lyrics again” in the tray menu.</div>'; }
  else if (status !== 'found') { sig = 'idle'; html = ''; }

  if (sig) {
    if (sig !== panelSig) { panel.innerHTML = html; panelSig = sig; }
    return;
  }

  const cur = lines[idx];
  const nextI = nextTextIndex(lines, idx + 1);
  const gap = nextI >= 0 ? lines[nextI].t - t : Infinity;
  const instrumental = (!cur || !cur.text) && gap > 2.5 && gap !== Infinity;
  sig = instrumental ? `gap:${nextI}:${Math.min(3, Math.floor(3 - gap / Math.max(1, (lines[nextI].t - (cur ? cur.t : 0)) / 3)))}` : `line:${idx}`;
  if (sig !== panelSig) {
    panelSig = sig;
    panel.replaceChildren();
    const add = (cls, text) => { const d = document.createElement('div'); d.className = `l ${cls}`; d.textContent = text; panel.append(d); return d; };
    const prevI = (() => { for (let i = idx - 1; i >= 0; i--) if (lines[i].text) return i; return -1; })();
    if (instrumental) {
      if (prevI >= 0) add('prev', lines[prevI].text);
      const dots = document.createElement('div');
      dots.className = 'dots';
      dots.innerHTML = '<i></i><i></i><i></i>';
      panel.append(dots);
      add('next', lines[nextI].text);
    } else if (cur && cur.text) {
      if (prevI >= 0) add('prev', lines[prevI].text);
      add('cur', cur.text);
      let shown = 0;
      for (let i = idx + 1; i < lines.length && shown < 2; i++) if (lines[i].text) { add('next', lines[i].text); shown++; }
    } else if (nextI >= 0) {
      add('next', lines[nextI].text);
    }
  }
  if (instrumental) {
    const span = lines[nextI].t - (cur ? cur.t : 0);
    const lit = Math.min(3, Math.floor(((span - gap) / span) * 4));
    panel.querySelectorAll('.dots i').forEach((d, i) => d.classList.toggle('on', i < lit));
  }
  const curEl = panel.querySelector('.cur');
  if (curEl && cur) {
    const end = lines[idx + 1] ? lines[idx + 1].t : cur.t + 4;
    curEl.style.setProperty('--p', Math.min(1, Math.max(0, (t - cur.t) / Math.max(.1, end - cur.t))).toFixed(3));
  }
}

/* ---------- Settings gear ----------
   The overlay lets clicks through to the desktop. While the pointer is on the gear, it asks to catch
   them instead, so the gear can be clicked. */
const gear = $('#gear');
let overGear = false;
function setOverGear(on) {
  if (on === overGear) return;
  overGear = on;
  api.send('ui-hover', on);
}
gear.addEventListener('mouseenter', () => setOverGear(true));
gear.addEventListener('mouseleave', () => setOverGear(false));
gear.addEventListener('click', () => api.send('open-settings'));

/* ---------- Frame loop ---------- */
function frame() {
  const now = Date.now();
  const recent = playing || now - idleSince < 20000;
  const showCard = editing || (settings.enabled && settings.showCard && track && recent);
  card.classList.toggle('hidden', !showCard);
  if (!showCard) setOverGear(false);

  if (track) {
    const t = songTime(), length = track.end - track.start;
    $('#elapsed').textContent = fmt(t);
    $('#length').textContent = length > 0 ? fmt(length) : '–:––';
    $('#fill').style.width = length > 0 ? `${Math.min(100, (t / length) * 100)}%` : '0%';

    const ready = lyrics.key === track.key && lyrics.status === 'found';
    const lines = ready ? lyrics.lines : [];
    const lt = t + songLead();
    const idx = lineIndex(lines, lt);
    renderPanel(lines, idx, lt);

    const wantBig = settings.enabled && settings.showBig && ready;
    big.classList.toggle('fade', !wantBig || (!playing && now - idleSince > 4000));
    if (wantBig && (idx !== shownIdx || track.key !== shownKey)) {
      const cur = lines[idx];
      const end = lines[idx + 1] ? lines[idx + 1].t : (cur ? cur.t + 4 : 0);
      const joinedLate = shownKey !== track.key || shownIdx === null || Math.abs(idx - shownIdx) > 1;
      showBigLine(cur ? cur.text : '', cur ? end - cur.t : 0, cur && joinedLate ? lt - cur.t : 0);
      shownIdx = idx; shownKey = track.key;
    }
    if (!wantBig && shownIdx !== null) { showBigLine('', 0); shownIdx = null; }
  } else {
    if (shownIdx !== null) { showBigLine('', 0); shownIdx = null; }
    if (editing) {
      $('#title').textContent = 'Nothing playing';
      $('#artist').textContent = 'The card shows up here';
      panel.innerHTML = '<div class="note">Lyrics appear under the card</div>';
      panelSig = 'edit';
    }
  }
  requestAnimationFrame(frame);
}

/* ---------- Settings, moving the card, messages ---------- */
function applySettings(s) {
  settings = s;
  document.documentElement.style.setProperty('--accent', s.accent);
  document.documentElement.style.setProperty('--big-top', s.bigPosition === 'bottom' ? '76%' : '44%');
  document.body.className = `font-${s.font}`;
  big.className = `big fx-${s.effect}${big.classList.contains('fade') ? ' fade' : ''}`;
  placeCard(s.card.x, s.card.y);
}

function placeCard(x, y) {
  const maxX = Math.max(0, innerWidth - card.offsetWidth), maxY = Math.max(0, innerHeight - 120);
  card.style.left = `${Math.min(maxX, Math.max(0, x))}px`;
  card.style.top = `${Math.min(maxY, Math.max(0, y))}px`;
}

let drag = null;
card.addEventListener('pointerdown', (e) => {
  if (!editing || e.target.closest('button')) return;
  drag = { dx: e.clientX - card.offsetLeft, dy: e.clientY - card.offsetTop };
  card.setPointerCapture(e.pointerId);
});
card.addEventListener('pointermove', (e) => { if (drag) placeCard(e.clientX - drag.dx, e.clientY - drag.dy); });
card.addEventListener('pointerup', () => {
  if (!drag) return;
  drag = null;
  api.send('card-moved', { x: card.offsetLeft, y: card.offsetTop });
});
$('#done').addEventListener('click', () => api.send('edit-done'));

let toastTimer;
function toast(msg) {
  const el = $('#toast');
  el.textContent = msg;
  el.classList.add('on');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.remove('on'), 2400);
}

api.on('settings', applySettings);
api.on('state', onState);
api.on('cover', (c) => { lastCover = c; if (track && c.key === track.key) $('#cover').src = c.data; });
api.on('lyrics', (l) => { lyrics = l; panelSig = ''; shownIdx = null; });
api.on('edit', (on) => { editing = on; card.classList.toggle('editing', on); panelSig = ''; if (!on && !track) panel.replaceChildren(); });
api.on('toast', toast);
api.on('panel-open', (open) => gear.classList.toggle('open', open));
api.on('sync-now', () => {
  if (!track || lyrics.key !== track.key || lyrics.status !== 'found') return toast('No synced lyrics to line up');
  // Line up the line on screen (or, during an intro, the first one coming) with this moment.
  const lines = lyrics.lines, t = songTime();
  const idx = lineIndex(lines, t + songLead());
  const target = lines[idx]?.text ? lines[idx] : lines[nextTextIndex(lines, idx + 1)];
  if (!target) return toast('No lyric line to line up');
  api.send('set-offset', { key: songKey(track), lead: Math.round((target.t - t) * 100) / 100 });
});
addEventListener('resize', () => placeCard(settings.card.x, settings.card.y));
requestAnimationFrame(frame);
