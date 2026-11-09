'use strict';
const api = window.overlay;
const $ = (s) => document.querySelector(s);
const card = $('#card'), panel = $('#panel'), big = $('#big');

let settings = {
  enabled: true, showCard: true, showCardLyrics: true, showBig: true, look: 'depth', colourMode: 'accent', motion: 'none',
  font: 'clean', accent: '#ffb648', entrance: 'rise', bigPos: { x: 0.5, y: 0.44 }, bigWidth: 0.84, bigSize: 1, bigAlign: 'center',
  offsets: {}, card: { x: 24, y: 24 }, cardScale: 1, cardOpacity: 0.84, cardLyricsSide: 'bottom', cardNext: 2,
  showCover: true, showArtist: true, showProgress: true, showControls: true,
};
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
let seekHold = null;
function songTime() {
  if (seeking && track?.key === seeking.key) return seeking.to;
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
  // Just after a jump, the player may still report the old time; wait until it catches up (or gives up).
  const held = seekHold && seekHold.key === s.key && now < seekHold.until && Math.abs(reported - seekHold.to) > 2;
  if (seekHold && !held) seekHold = null;
  if (!held && !seeking && (isNew || nowPlaying !== playing || drift > .35)) { anchorPos = reported; anchorMs = now; anchorVersion++; }
  if (held && nowPlaying !== playing) { anchorPos = songTime(); anchorMs = now; anchorVersion++; }
  if (!nowPlaying && playing) idleSince = now;
  playing = nowPlaying;
  if (isNew) {
    // The cover can arrive just before the new song's details, so keep it rather than clearing it.
    if (lastCover?.key === s.key) $('#cover').src = lastCover.data;
    else $('#cover').removeAttribute('src');
    card.classList.remove('pop'); void card.offsetWidth; card.classList.add('pop');
  }
  track = s;
  setText($('#title'), s.title || 'Unknown title');
  setText($('#artist'), s.artist || '');
  $('#eq').classList.toggle('paused', !playing);
  showControls(s);
  update();
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
    // Whole 60 fps frames, so every word's steps land on the same frames.
    word.style.animationDelay = `${(Math.round(Math.max(-.42, i * step - elapsed) * 60) / 60).toFixed(4)}s`;
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
    return false;
  }

  const cur = lines[idx];
  const nextI = nextTextIndex(lines, idx + 1);
  const gap = nextI >= 0 ? lines[nextI].t - t : Infinity;
  const instrumental = (!cur || !cur.text) && gap > 2.5 && gap !== Infinity;
  sig = instrumental ? `gap:${nextI}:${Math.min(3, Math.floor(3 - gap / Math.max(1, (lines[nextI].t - (cur ? cur.t : 0)) / 3)))}` : `line:${idx}:${anchorVersion}:${playing}`;
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
      // The underline runs as a CSS animation over the line's length, starting from where we are in it,
      // in 60 steps a second (see the note on stepped animations in overlay.css).
      const el = add('cur', cur.text), end = lines[idx + 1] ? lines[idx + 1].t : cur.t + 4;
      const dur = Math.max(.1, end - cur.t);
      el.style.setProperty('--dur', `${dur.toFixed(2)}s`);
      el.style.setProperty('--steps', Math.max(1, Math.round(dur * 60)));
      el.style.setProperty('--delay', `${(cur.t - t).toFixed(2)}s`);
      let shown = 0;
      const coming = settings.cardNext ?? 2;
      for (let i = idx + 1; i < lines.length && shown < coming; i++) if (lines[i].text) { add('next', lines[i].text); shown++; }
    } else if (nextI >= 0) {
      add('next', lines[nextI].text);
    }
  }
  if (instrumental) {
    const span = lines[nextI].t - (cur ? cur.t : 0);
    const lit = Math.min(3, Math.floor(((span - gap) / span) * 4));
    panel.querySelectorAll('.dots i').forEach((d, i) => d.classList.toggle('on', i < lit));
  }
  return instrumental;
}

/* ---------- Catching the mouse on the card ----------
   The overlay lets clicks through to the desktop. Main catches the mouse while the pointer is over the card (we
   tell it where the card is), so the buttons work and the card can be dragged out of the way. While caught we
   see the mouse as usual; "ui-hover" tells main to hold on (during a drag of the card or on the progress bar,
   even when the pointer runs ahead of the card) or that the pointer has left. */
const gear = $('#gear'), bar = $('#bar'), playBtn = $('#play');
let overUi = false, seeking = null, drag = null;
// Sends false, true (the pointer is on the card) or 'busy' (dragging the card or seeking: hold on no matter what).
function setOverUi(on) {
  if (seeking || drag) on = 'busy';
  if (on === overUi) return;
  overUi = on;
  api.send('ui-hover', on);
}
const clickable = (el) => !!el?.closest?.('#card') && !card.classList.contains('hidden');
gear.addEventListener('click', () => {
  const r = card.getBoundingClientRect();
  api.send('open-settings', { x: Math.round(r.left), y: Math.round(r.top), width: Math.round(r.width) });
});

// Also true while the mouse is on the card, so a card hidden after a pause comes back to be played again.
let mouseOnCard = false;
addEventListener('mousemove', (e) => {
  setOverUi(!editing && clickable(e.target));
  const r = card.getBoundingClientRect();
  const on = e.clientX >= r.left && e.clientX <= r.right && e.clientY >= r.top && e.clientY <= r.bottom;
  if (on !== mouseOnCard) { mouseOnCard = on; update(); }
});
document.addEventListener('mouseleave', () => { mouseOnCard = false; setOverUi(false); update(); });
// Main stopped catching the mouse (the pointer left the card without us seeing it go).
api.on('pointer-left', () => { setOverUi(false); if (mouseOnCard) { mouseOnCard = false; update(); } });

/** Tells main where the card is, or null while it can't show, so the mouse is only caught over a card that
    can appear. Sent only when it changes. */
let sentRect = '';
function sendCardRect() {
  const can = editing || (settings.enabled && settings.showCard && !!track);
  const r = can ? { x: card.offsetLeft, y: card.offsetTop, width: card.offsetWidth, height: card.offsetHeight } : null;
  const key = JSON.stringify(r);
  if (key !== sentRect) { sentRect = key; api.send('card-rect', r); }
}
new ResizeObserver(() => sendCardRect()).observe(card);

/* ---------- Playback controls ---------- */
const sendControl = (cmd, extra = {}) => track && api.send('control', { cmd, key: track.key, ...extra });
$('#prev').addEventListener('click', () => sendControl('prev'));
$('#next').addEventListener('click', () => sendControl('next'));
playBtn.addEventListener('click', () => {
  sendControl('toggle');
  playBtn.classList.toggle('paused'); // show the change now; the player's answer follows within a moment
  setTimeout(() => track && showControls(track), 1500); // and if the player ignored it, show the truth again
});

function showControls(s) {
  const c = s.controls || {};
  $('#prev').disabled = !c.prev;
  $('#next').disabled = !c.next;
  playBtn.disabled = !c.toggle;
  playBtn.classList.toggle('paused', s.status !== 'Playing');
  playBtn.title = s.status === 'Playing' ? 'Pause' : 'Play';
  const length = s.end - s.start;
  bar.classList.toggle('off', !c.seek || !(length > 0));
  bar.title = bar.classList.contains('off') ? "This player can't jump" : 'Drag to jump';
}

// Jumping: drag (or click) on the bar. While dragging, the card and lyrics preview that time.
const barTime = (e) => {
  const r = bar.getBoundingClientRect(), length = track.end - track.start;
  return Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)) * length;
};
bar.addEventListener('pointerdown', (e) => {
  if (!track || bar.classList.contains('off') || editing) return;
  seeking = { key: track.key, to: barTime(e) };
  bar.setPointerCapture(e.pointerId);
  bar.classList.add('dragging');
  setOverUi(true);
  update();
});
bar.addEventListener('pointermove', (e) => { if (seeking && track?.key === seeking.key) { seeking.to = barTime(e); update(); } });
const endSeek = (e, cancelled) => {
  if (!seeking) return;
  const { key, to } = seeking;
  seeking = null;
  bar.classList.remove('dragging');
  setOverUi(clickable(document.elementFromPoint(e.clientX, e.clientY)));
  if (cancelled || track?.key !== key) return update();
  sendControl('seek', { to });
  // Move there now; the player's own report can lag behind for a moment, so don't let it pull us back.
  anchorPos = to; anchorMs = Date.now(); anchorVersion++;
  update();
  seekHold = { key, to, until: Date.now() + 2500 };
};
bar.addEventListener('pointerup', (e) => endSeek(e, false));
bar.addEventListener('pointercancel', (e) => endSeek(e, true));
bar.addEventListener('lostpointercapture', (e) => endSeek(e, true));

/* ---------- Updating the screen ----------
   Nothing is redrawn every frame. update() puts on screen what belongs to this moment, works out when something
   will next change (the next lyric line, the clock's next second), and sleeps until then. Anything that changes
   what should show (the player, lyrics, settings, the mouse) calls update() straight away. Movement in between
   (the progress bar's glide, the current line's underline) is left to CSS. */
let updateTimer = null, anchorVersion = 0, lastBarTime = null;
const setText = (el, text) => { if (el.textContent !== text) el.textContent = text; };

function update() {
  clearTimeout(updateTimer);
  updateTimer = setTimeout(update, Math.max(16, Math.min(1000, render())));
}

/** Shows the progress bar at `pos` (0..1). It glides there over the next second, unless it jumped. */
function showBarPos(pos, t) {
  const jumped = lastBarTime === null || !!seeking || Math.abs(t - lastBarTime) > 1.6;
  bar.classList.toggle('jump', jumped);
  if (jumped) void bar.offsetWidth; // take the jump without the glide
  const value = pos.toFixed(4);
  if (bar.style.getPropertyValue('--pos') !== value) bar.style.setProperty('--pos', value);
  lastBarTime = t;
}

/** Puts this moment on screen; returns how many ms until something next needs changing. */
function render() {
  const now = Date.now();
  const recent = playing || now - idleSince < 20000;
  const showCard = editing || (settings.enabled && settings.showCard && track && (recent || mouseOnCard || seeking));
  card.classList.toggle('hidden', !showCard);
  card.classList.toggle('paused', !playing || !!seeking);
  if (!showCard && !seeking && !drag) setOverUi(false);
  let wait = 1000;

  if (track) {
    const t = songTime(), length = track.end - track.start;
    setText($('#elapsed'), fmt(t));
    setText($('#length'), length > 0 ? fmt(length) : '–:––');
    showBarPos(length > 0 ? Math.min(1, t / length) : 0, t);
    if (playing && !seeking) wait = (1 - (t % 1)) * 1000 + 5; // the clock's next second

    const ready = lyrics.key === track.key && lyrics.status === 'found';
    const lines = ready ? lyrics.lines : [];
    const lt = t + songLead();
    const idx = lineIndex(lines, lt);
    if (playing && !seeking && lines[idx + 1]) wait = Math.min(wait, (lines[idx + 1].t - lt) * 1000 + 5); // the next line
    if (renderPanel(lines, idx, lt) && playing) wait = Math.min(wait, 250); // instrumental dots light up in steps

    const wantBig = settings.enabled && settings.showBig && ready;
    const fadeBig = !wantBig || (!playing && now - idleSince > 4000);
    big.classList.toggle('fade', fadeBig);
    if (wantBig && (idx !== shownIdx || track.key !== shownKey)) {
      const cur = lines[idx];
      const end = lines[idx + 1] ? lines[idx + 1].t : (cur ? cur.t + 4 : 0);
      const joinedLate = shownKey !== track.key || shownIdx === null || Math.abs(idx - shownIdx) > 1;
      showBigLine(cur ? cur.text : '', cur ? end - cur.t : 0, cur && joinedLate ? lt - cur.t : 0);
      shownIdx = idx; shownKey = track.key;
    }
    if (!wantBig && shownIdx !== null) { showBigLine('', 0); shownIdx = null; }
  } else {
    big.classList.add('fade');
    if (shownIdx !== null) { showBigLine('', 0); shownIdx = null; }
    if (editing) {
      setText($('#title'), 'Nothing playing');
      setText($('#artist'), 'The card shows up here');
      if (panelSig !== 'edit') { panel.innerHTML = '<div class="note">Lyrics appear under the card</div>'; panelSig = 'edit'; }
    }
  }
  sendCardRect();
  // Nothing on screen: stop every animation, so an idle overlay costs nothing.
  document.documentElement.classList.toggle('idle', card.classList.contains('hidden') && big.classList.contains('fade') && !big.querySelector('.line.leaving'));
  return wait;
}

/* ---------- Colour ----------
   Everything coloured (big lyrics, the card's bar and current line) follows --accent: the picked colour, the
   cover's colour, or in fade mode a colour that flows between the two fade colours (done in CSS). */
let coverColour = null; // the cover's main colour, worked out by main
function applyAccent() {
  const root = document.documentElement, s = settings;
  root.classList.toggle('colour-fade', s.colourMode === 'fade');
  root.style.setProperty('--fade-a', s.fadeFrom || '#ff6fae');
  root.style.setProperty('--fade-b', s.fadeTo || '#6cc8ff');
  if (s.colourMode === 'fade') root.style.removeProperty('--accent');
  else root.style.setProperty('--accent', s.colourMode === 'cover' && coverColour ? coverColour : s.accent);
}

/* ---------- Settings, moving the card, messages ---------- */
function applySettings(s) {
  settings = s;
  applyAccent();
  for (const c of [...document.body.classList]) if (c.startsWith('font-')) document.body.classList.remove(c);
  document.body.classList.add(`font-${s.font}`);
  const bigOff = !(s.enabled && s.showBig);
  big.className = `big look-${s.look} colour-${s.colourMode} motion-${s.motion} entrance-${s.entrance || 'rise'} align-${s.bigAlign || 'center'}`
    + `${big.classList.contains('fade') ? ' fade' : ''}${bigOff ? ' off' : ''}`;
  // The card: its parts, where its lyrics go, how solid and how big.
  card.classList.toggle('no-lyrics', s.showCardLyrics === false);
  for (const [cls, on] of [['no-cover', s.showCover], ['no-artist', s.showArtist], ['no-progress', s.showProgress], ['no-controls', s.showControls]]) {
    card.classList.toggle(cls, on === false);
  }
  for (const side of ['bottom', 'top', 'left', 'right']) card.classList.toggle(`side-${side}`, (s.cardLyricsSide || 'bottom') === side);
  const alpha = s.cardOpacity ?? 0.84;
  card.style.setProperty('--card-alpha', alpha);
  card.classList.toggle('clear', alpha < 0.45);
  if (!sizeDrag) card.style.setProperty('--card-scale', s.cardScale || 1);
  placeCard(s.card.x, s.card.y);
  if (!bigDrag) placeBig(bigLayout());
  panelSig = ''; shownIdx = null; // timing or style may have changed
  update();
}

/* ---------- Where the big lyrics go ---------- */
const bigLayout = () => ({
  x: settings.bigPos?.x ?? 0.5, y: settings.bigPos?.y ?? 0.44, w: settings.bigWidth ?? 0.84, size: settings.bigSize ?? 1,
});

/** Puts the big lyrics there, keeping the whole box on screen. */
function placeBig(l) {
  big.style.setProperty('--big-w', l.w);
  big.style.setProperty('--big-size', l.size);
  const halfW = Math.min(0.5, l.w / 2), halfH = Math.min(0.5, big.offsetHeight / innerHeight / 2);
  big.style.setProperty('--big-x', Math.min(1 - halfW, Math.max(halfW, l.x)).toFixed(4));
  big.style.setProperty('--big-y', Math.min(1 - halfH, Math.max(halfH, l.y)).toFixed(4));
}

function placeCard(x, y) {
  const maxX = Math.max(0, innerWidth - card.offsetWidth), maxY = Math.max(0, innerHeight - 120);
  card.style.left = `${Math.min(maxX, Math.max(0, x))}px`;
  card.style.top = `${Math.min(maxY, Math.max(0, y))}px`;
  // The grid only shows near the card, centred on its main part (not the hint under it).
  grid.style.setProperty('--gx', `${card.offsetLeft + card.offsetWidth / 2}px`);
  grid.style.setProperty('--gy', `${card.offsetTop + card.querySelector('.now').getBoundingClientRect().height / 2 + 40}px`);
  sendCardRect();
}
const grid = $('#grid');

/* ---------- Moving the card: grid, guides and snapping ---------- */
const GRID = 24, MARGIN = 24, SNAP = 12;
const guideX = $('#guide-x'), guideY = $('#guide-y');

/** Lines the card snaps to along one axis, and which part of the card (start, centre, end) may sit on each. */
const snapLines = (length) => [
  [MARGIN, ['start']],
  [length - MARGIN, ['end']],
  [length / 2, ['center', 'start', 'end']],
  [length / 3, ['start', 'center', 'end']],
  [(length * 2) / 3, ['start', 'center', 'end']],
];

/** Snaps to the nearest line within reach, otherwise to the grid. `line` is where to draw the guide, or null. */
function snapAxis(pos, size, length) {
  let best = null;
  for (const [line, parts] of snapLines(length)) {
    for (const part of parts) {
      const offset = part === 'start' ? 0 : part === 'center' ? size / 2 : size;
      const d = Math.abs(pos + offset - line);
      if (d <= SNAP && (!best || d < best.d)) best = { d, pos: line - offset, line };
    }
  }
  return best || { pos: Math.round(pos / GRID) * GRID, line: null };
}

function showGuide(el, at, side) {
  el.classList.toggle('on', at !== null);
  if (at !== null) el.style[side] = `${Math.round(at)}px`;
}

/* Right-click the card to hide it and click what's under it. Main fades it out and brings it back once the
   mouse has moved off where it was. */
card.addEventListener('contextmenu', (e) => {
  e.preventDefault();
  if (!editing && !card.classList.contains('hidden')) api.send('peek-now');
});

/* Drag the card from anywhere but its buttons and progress bar, any time (not only in "Move card" mode),
   so it can be pushed aside when it is over something you want to click. It only starts moving after a
   few pixels, so a plain click on the card does nothing. */
card.addEventListener('pointerdown', (e) => {
  if (e.button !== 0 || card.classList.contains('hidden')) return;
  if (e.target.closest('button') || e.target.closest('.bar:not(.off)') || e.target.closest('.handle')) return;
  drag = { dx: e.clientX - card.offsetLeft, dy: e.clientY - card.offsetTop, x0: e.clientX, y0: e.clientY, moved: false };
  card.setPointerCapture(e.pointerId);
  setOverUi(true);
});
card.addEventListener('pointermove', (e) => {
  if (!drag) return;
  if (!drag.moved) {
    if (Math.hypot(e.clientX - drag.x0, e.clientY - drag.y0) < 4) return;
    drag.moved = true;
    document.body.classList.add('dragging');
  }
  let x = e.clientX - drag.dx, y = e.clientY - drag.dy, lineX = null, lineY = null;
  if (!e.altKey) { // hold Alt to move freely
    const hint = card.querySelector('.edit-hint'); // only shows (under the card) in "Move card" mode
    const height = card.offsetHeight - (editing ? hint.offsetHeight + 8 : 0);
    ({ pos: x, line: lineX } = snapAxis(x, card.offsetWidth, innerWidth));
    ({ pos: y, line: lineY } = snapAxis(y, height, innerHeight));
  }
  placeCard(x, y);
  showGuide(guideX, lineX, 'left');
  showGuide(guideY, lineY, 'top');
});
function endDrag(e) {
  if (!drag) return;
  const { moved } = drag;
  drag = null;
  document.body.classList.remove('dragging');
  showGuide(guideX, null);
  showGuide(guideY, null);
  if (!editing) setOverUi(clickable(document.elementFromPoint(e.clientX, e.clientY)));
  if (moved) api.send('card-moved', { x: card.offsetLeft, y: card.offsetTop });
}
card.addEventListener('pointerup', endDrag);
card.addEventListener('pointercancel', endDrag);
card.addEventListener('lostpointercapture', endDrag);
$('#done').addEventListener('click', () => api.send('edit-done'));

/* ---------- Resizing while arranging ----------
   The card's corner sets its size. The big lyrics' frame moves by dragging it (snapping like the card), its
   sides set how wide the lines run, and its corner sets the text size. What you see changes as you drag; the
   new value is saved when you let go. */
const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
let sizeDrag = null, bigDrag = null;

/** Calls `move` with each pointer move and `end` once when let go, with the pointer captured on `el`. */
function follow(el, e, move, end) {
  el.setPointerCapture(e.pointerId);
  document.body.classList.add('dragging');
  const onMove = (ev) => move(ev);
  const onEnd = () => {
    el.removeEventListener('pointermove', onMove);
    el.removeEventListener('pointerup', onEnd);
    el.removeEventListener('lostpointercapture', onEnd);
    document.body.classList.remove('dragging');
    end();
  };
  el.addEventListener('pointermove', onMove);
  el.addEventListener('pointerup', onEnd);
  el.addEventListener('lostpointercapture', onEnd);
}

const cardSize = $('#card-size');
cardSize.addEventListener('pointerdown', (e) => {
  if (!editing || e.button !== 0) return;
  e.stopPropagation();
  const x0 = e.clientX, w0 = card.offsetWidth, s0 = settings.cardScale || 1;
  sizeDrag = { scale: s0 };
  follow(cardSize, e, (ev) => {
    sizeDrag.scale = Math.round(clamp(s0 * (w0 + ev.clientX - x0) / w0, 0.6, 2) * 100) / 100;
    card.style.setProperty('--card-scale', sizeDrag.scale);
    placeCard(card.offsetLeft, card.offsetTop);
  }, () => {
    const { scale } = sizeDrag;
    sizeDrag = null;
    if (scale !== s0) api.send('set', { key: 'cardScale', value: scale });
    placeCard(card.offsetLeft, card.offsetTop);
    api.send('card-moved', { x: card.offsetLeft, y: card.offsetTop });
  });
});

big.addEventListener('pointerdown', (e) => {
  if (!editing || e.button !== 0 || big.classList.contains('off')) return;
  const kind = e.target.dataset?.big || 'move';
  const r = big.getBoundingClientRect(), start = bigLayout();
  const cx = r.left + r.width / 2, cy = r.top + r.height / 2, x0 = e.clientX, y0 = e.clientY;
  const live = bigDrag = { ...start };
  follow(big, e, (ev) => {
    if (kind === 'move') {
      let left = r.left + ev.clientX - x0, top = r.top + ev.clientY - y0, lineX = null, lineY = null;
      if (!ev.altKey) {
        ({ pos: left, line: lineX } = snapAxis(left, r.width, innerWidth));
        ({ pos: top, line: lineY } = snapAxis(top, r.height, innerHeight));
      }
      live.x = (left + r.width / 2) / innerWidth;
      live.y = (top + r.height / 2) / innerHeight;
      showGuide(guideX, lineX, 'left');
      showGuide(guideY, lineY, 'top');
    } else if (kind === 'width') {
      live.w = Math.round(clamp((Math.abs(ev.clientX - cx) * 2) / innerWidth, 0.25, 0.98) * 100) / 100;
    } else {
      const from = Math.max(10, Math.abs(y0 - cy));
      live.size = Math.round(clamp(start.size * (Math.abs(ev.clientY - cy) / from), 0.4, 2.5) * 100) / 100;
    }
    placeBig(live);
  }, () => {
    bigDrag = null;
    showGuide(guideX, null);
    showGuide(guideY, null);
    // Save where it really ended up (kept on screen), not where the pointer pushed it.
    const at = { x: parseFloat(big.style.getPropertyValue('--big-x')), y: parseFloat(big.style.getPropertyValue('--big-y')) };
    if (kind === 'move' && (at.x !== start.x || at.y !== start.y)) api.send('set', { key: 'bigPos', value: at });
    if (live.w !== start.w) api.send('set', { key: 'bigWidth', value: live.w });
    if (live.size !== start.size) api.send('set', { key: 'bigSize', value: live.size });
  });
});

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
api.on('cover-colour', (c) => { coverColour = c.colour; applyAccent(); });
api.on('cover', (c) => { lastCover = c; if (track && c.key === track.key) $('#cover').src = c.data; });
api.on('lyrics', (l) => { lyrics = l; panelSig = ''; shownIdx = null; update(); });
api.on('edit', (on) => { editing = on; card.classList.toggle('editing', on); document.body.classList.toggle('moving', on); panelSig = ''; if (!on && !track) panel.replaceChildren(); update(); });
api.on('toast', toast);
api.on('peek', (on) => document.body.classList.toggle('peek', on));
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
addEventListener('resize', () => { placeCard(settings.card.x, settings.card.y); placeBig(bigLayout()); });
update();
