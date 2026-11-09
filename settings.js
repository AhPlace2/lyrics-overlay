'use strict';
// Named "bridge" because a top-level "panel" would clash with the window.panel the preload adds.
const bridge = window.panel;
const $ = (s) => document.querySelector(s);
const $$ = (s) => [...document.querySelectorAll(s)];
let built = false;

function build(options) {
  const chips = (key, list, cls) => {
    const box = $(`[data-choose="${key}"]`);
    for (const [value, label] of list) {
      const b = document.createElement('button');
      b.type = 'button'; b.dataset.value = value; b.textContent = label;
      if (cls) b.className = cls(value);
      b.addEventListener('click', () => bridge.act('choose', { key, value }));
      box.append(b);
    }
  };
  chips('look', options.looks);
  chips('motion', options.motions);
  chips('entrance', options.entrances);
  chips('font', options.fonts, (v) => `font-${v}`);
  chips('cardLyricsSide', options.sides);
  chips('bigAlign', options.aligns);
  chips('peekKey', options.peekKeys);
  built = true;
}

/* ---------- Sliders ----------
   Percent sliders hold the setting × 100. While one is dragged it is sent at most every 80 ms (each change is
   saved to disk), and changes coming back from main don't move it under the pointer. */
const sliding = new Set();
const isPct = (input) => input.dataset.unit === '%';
const sliderValue = (input) => Number(input.value) / (isPct(input) ? 100 : 1);
function showOut(input, v) {
  const out = $(`[data-out="${input.dataset.set}"]`);
  if (out) out.textContent = isPct(input) ? `${Math.round(v * 100)}%` : String(v);
}
for (const input of $$('input[data-set]')) {
  const key = input.dataset.set;
  let sentAt = 0, timer = null;
  const send = (final) => {
    clearTimeout(timer);
    const go = () => { sentAt = Date.now(); bridge.act('set', { key, value: sliderValue(input) }); };
    if (final || Date.now() - sentAt > 80) go(); else timer = setTimeout(go, 80);
  };
  input.addEventListener('pointerdown', () => sliding.add(key));
  input.addEventListener('input', () => { showOut(input, sliderValue(input)); send(false); });
  input.addEventListener('change', () => { sliding.delete(key); send(true); });
  input.addEventListener('pointerup', () => sliding.delete(key));
}

// Quick places for the big lyrics (centred across, at that height).
for (const b of $$('#presets button')) {
  b.addEventListener('click', () => bridge.act('set', { key: 'bigPos', value: { x: 0.5, y: Number(b.dataset.y) } }));
}

/* ---------- Colour ----------
   Swatches: your colour (opens the picker), rainbow, the last two colours you picked, a fade between two
   colours, and the cover's colour. The picker edits your colour, or one end of the fade. */
let current = null; // the last state from main
let target = null; // what the picker is editing: 'accent', 'fadeFrom', 'fadeTo', or null when closed
let hsv = { h: 0, s: 0, v: 1 }, dragging = false, sentAt = 0, sendTimer = null, shown = null;
const choose = (key, value, extra) => bridge.act('choose', { key, value, ...extra });
const mode = () => current?.settings.colourMode;

const hexToHsv = (hex) => {
  const n = parseInt(hex.slice(1), 16), r = (n >> 16) / 255, g = ((n >> 8) & 255) / 255, b = (n & 255) / 255;
  const max = Math.max(r, g, b), d = max - Math.min(r, g, b);
  let h = 0;
  if (d) h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return { h: (h * 60 + 360) % 360, s: max ? d / max : 0, v: max };
};
const hsvToHex = ({ h, s, v }) => {
  const f = (n) => { const k = (n + h / 60) % 6; return Math.round((v - v * s * Math.max(0, Math.min(k, 4 - k, 1))) * 255); };
  return '#' + [f(5), f(3), f(1)].map((x) => x.toString(16).padStart(2, '0')).join('');
};

function drawPicker(updateHex = true) {
  const hex = hsvToHex(hsv);
  $('#sv').style.setProperty('--h', hsv.h);
  $('#sv-knob').style.left = `${hsv.s * 100}%`;
  $('#sv-knob').style.top = `${(1 - hsv.v) * 100}%`;
  $('#sv-knob').style.background = hex;
  $('#hue-knob').style.left = `${(hsv.h / 360) * 100}%`;
  $('#hue-knob').style.background = `hsl(${hsv.h} 100% 50%)`;
  $('#preview').style.background = hex;
  if (updateHex) { $('#hex').value = hex.toUpperCase(); $('#hex').classList.remove('bad'); }
}

// Sends the colour while dragging, but at most every 60 ms, since each change is saved to disk.
// "commit" marks the colour as really picked (for the history), which happens when you let go.
function pickColour(hex, commit = false) {
  if (!target) return;
  clearTimeout(sendTimer);
  const key = target;
  const go = () => { sentAt = Date.now(); choose(key, hex, { commit }); };
  if (commit || Date.now() - sentAt > 60) go();
  else sendTimer = setTimeout(go, 60);
}

/** Opens the picker on a colour (or closes it with null), loaded with that colour. */
function openPicker(what) {
  target = what;
  $('#picker').classList.toggle('hidden', !what);
  if (what && current) { shown = current.settings[what]; hsv = hexToHsv(shown); drawPicker(); }
  render(current);
}

function dragOn(el, onMove) {
  const at = (e) => {
    const r = el.getBoundingClientRect();
    onMove(Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)), Math.min(1, Math.max(0, (e.clientY - r.top) / r.height)));
    drawPicker();
    pickColour(hsvToHex(hsv));
  };
  el.addEventListener('pointerdown', (e) => { dragging = true; el.setPointerCapture(e.pointerId); at(e); });
  el.addEventListener('pointermove', (e) => { if (dragging && el.hasPointerCapture(e.pointerId)) at(e); });
  const end = () => { if (dragging) { dragging = false; pickColour(hsvToHex(hsv), true); } };
  el.addEventListener('pointerup', end);
  el.addEventListener('pointercancel', end);
}
dragOn($('#sv'), (x, y) => { hsv = { ...hsv, s: x, v: 1 - y }; });
dragOn($('#hue'), (x) => { hsv = { ...hsv, h: Math.min(359.9, x * 360) }; });

const typedHex = () => {
  let v = $('#hex').value.trim();
  if (!v.startsWith('#')) v = '#' + v;
  if (/^#[0-9a-f]{3}$/i.test(v)) v = '#' + [...v.slice(1)].map((c) => c + c).join('');
  return /^#[0-9a-f]{6}$/i.test(v) ? v.toLowerCase() : null;
};
$('#hex').addEventListener('input', () => {
  const v = typedHex();
  $('#hex').classList.toggle('bad', !v);
  if (!v) return;
  hsv = hexToHsv(v);
  drawPicker(false);
  pickColour(v);
});
$('#hex').addEventListener('change', () => { const v = typedHex(); if (v) pickColour(v, true); });
// Esc in the colour code field closes the panel like everywhere else; Enter just confirms.
$('#hex').addEventListener('keydown', (e) => { if (e.key === 'Enter') $('#hex').blur(); });

// Your colour: the first click uses it straight away (and opens the picker), the next ones open or close it.
$('[data-colour="accent"]').addEventListener('click', () => {
  if (mode() !== 'accent') { choose('colourMode', 'accent'); openPicker('accent'); }
  else openPicker(target === 'accent' ? null : 'accent');
});
$('[data-colour="rainbow"]').addEventListener('click', () => { choose('colourMode', 'rainbow'); openPicker(null); });
$('[data-colour="cover"]').addEventListener('click', () => { choose('colourMode', 'cover'); openPicker(null); });
$('[data-colour="fade"]').addEventListener('click', () => {
  if (mode() !== 'fade') choose('colourMode', 'fade');
  if (target === 'accent') openPicker(null);
});
for (const b of $$('[data-end]')) {
  b.addEventListener('click', () => openPicker(target === b.dataset.end ? null : b.dataset.end));
}
for (const b of $$('[data-history]')) {
  b.addEventListener('click', () => {
    const c = current?.settings.history?.[b.dataset.history];
    if (!c) return;
    choose('accent', c, { commit: true });
    if (target && target !== 'accent') openPicker(null);
  });
}

const COLOUR_NAMES = { accent: 'your colour', rainbow: 'rainbow', fade: 'fade', cover: 'from the cover' };

function renderColour({ settings: s, coverColour }) {
  const m = s.colourMode;
  // The panel's own highlights use the colour too (the start colour while fading).
  document.documentElement.style.setProperty('--accent', m === 'fade' ? s.fadeFrom : m === 'cover' && coverColour ? coverColour : s.accent);
  $('#colour-name').textContent = COLOUR_NAMES[m] || '';
  for (const b of $$('[data-colour]')) b.classList.toggle('on', b.dataset.colour === m);
  const custom = $('[data-colour="accent"]');
  custom.style.setProperty('--col', s.accent);
  custom.classList.toggle('open', target === 'accent');
  for (const b of $$('[data-history]')) {
    const c = s.history?.[b.dataset.history];
    b.classList.toggle('empty', !c);
    b.style.background = c || '';
    b.title = c ? `Earlier colour ${c.toUpperCase()}` : 'Colours you pick show up here';
  }
  $('[data-colour="fade"]').style.background = `linear-gradient(135deg, ${s.fadeFrom} 15%, ${s.fadeTo} 85%)`;
  const cover = $('[data-colour="cover"]');
  cover.style.setProperty('--col', coverColour || '');
  cover.classList.toggle('none', !coverColour);
  $('#fade-ends').classList.toggle('hidden', m !== 'fade');
  for (const b of $$('[data-end]')) {
    b.querySelector('i').style.background = s[b.dataset.end];
    b.classList.toggle('open', target === b.dataset.end);
  }
  $('#cover-hint').classList.toggle('hidden', m !== 'cover');
  $('#cover-hint').textContent = coverColour ? 'Follows the cover art of whatever is playing.' : 'Shows once something with cover art plays.';

  // Keep the picker on what it edits: closed when that colour is no longer in use, and in step with
  // changes made elsewhere, but never fighting a drag in progress.
  if ((target === 'accent' && m !== 'accent') || ((target === 'fadeFrom' || target === 'fadeTo') && m !== 'fade')) {
    target = null; $('#picker').classList.add('hidden');
  }
  const value = target && s[target];
  if (value && value !== shown) {
    shown = value;
    if (!dragging && document.activeElement !== $('#hex')) { hsv = hexToHsv(value); drawPicker(); }
  }
}

function render(state) {
  if (!state) return;
  current = state;
  const { settings, options, song } = state;
  if (!built) build(options);
  for (const box of $$('input[data-toggle]')) box.checked = !!settings[box.dataset.toggle];
  for (const row of $$('.row.sub')) row.classList.toggle('off', !settings.showCard); // card options only matter with the card on
  for (const box of $$('[data-choose]')) {
    for (const b of box.children) b.classList.toggle('on', settings[box.dataset.choose] === b.dataset.value);
  }
  for (const input of $$('input[data-set]')) {
    const v = settings[input.dataset.set];
    if (!Number.isFinite(v)) continue;
    if (!sliding.has(input.dataset.set)) input.value = Math.round(v * (isPct(input) ? 100 : 1));
    showOut(input, v);
  }
  const pos = settings.bigPos || { x: 0.5, y: 0.44 };
  for (const b of $$('#presets button')) b.classList.toggle('on', Math.abs(pos.x - 0.5) < 0.01 && Math.abs(pos.y - Number(b.dataset.y)) < 0.01);
  renderColour(state);
  $('#song').classList.toggle('hidden', !song);
  $('#nothing').classList.toggle('hidden', !!song);
  if (song) {
    $('#song-name').textContent = song.artist ? `${song.title} · ${song.artist}` : song.title;
    $('#lead').textContent = song.lead === 0 ? 'in sync' : `${Math.abs(song.lead).toFixed(2)} s ${song.lead > 0 ? 'earlier' : 'later'}`;
    $('#source').textContent = song.listening ? 'listening…' : song.provider ? `from ${song.provider}` : song.status === 'loading' ? 'looking…' : 'none found';
    const r = song.recognisedAs;
    $('#recognised').classList.toggle('hidden', !r);
    if (r) $('#recognised').textContent = `Recognised as ${r.artist ? `${r.artist} – ${r.title}` : r.title}`;
    $('[data-act="recognise"]').disabled = song.listening;
    const synced = song.status === 'found';
    $('[data-act="reset"]').disabled = song.lead === 0;
    $('[data-act="sync"]').disabled = !synced;
    $('[data-act="first"]').disabled = !song.picked;
  }
}

for (const box of $$('input[data-toggle]')) box.addEventListener('change', () => bridge.act('toggle', box.dataset.toggle));
for (const b of $$('[data-act]')) b.addEventListener('click', () => bridge.act(b.dataset.act));
$('#close').addEventListener('click', () => bridge.act('close'));

// Reset all asks once: the first click turns the button red, a second click within 3 s resets.
{
  const b = $('#reset-all');
  let timer = 0;
  const calm = () => { clearTimeout(timer); b.classList.remove('confirm'); b.textContent = 'Reset all'; };
  b.addEventListener('click', () => {
    if (!b.classList.contains('confirm')) {
      b.classList.add('confirm');
      b.textContent = 'Click again to reset';
      timer = setTimeout(calm, 3000);
      return;
    }
    calm();
    bridge.act('reset-all');
  });
}
addEventListener('keydown', (e) => { if (e.key === 'Escape') bridge.act('close'); });

/* ---------- Window size ----------
   The window grows or shrinks with what is shown. While a tab change animates the height, the window is
   sized once for the taller of the two tabs instead of on every frame, then fitted when it is done. */
let sizeLock = 0;
const bodyHeight = () => Math.ceil(document.body.getBoundingClientRect().height);
new ResizeObserver(() => { if (!sizeLock) bridge.act('size', bodyHeight()); }).observe(document.body);

/* ---------- Tabs ---------- */
const TABS = ['show', 'layout', 'look', 'song'];
const stage = $('#stage');
let tab = null, tabDone = null;

function showTab(name, animate = true) {
  if (!TABS.includes(name) || name === tab) return;
  const from = tab, oldPane = from && $(`[data-pane="${from}"]`), newPane = $(`[data-pane="${name}"]`);
  tab = name;
  try { localStorage.setItem('tab', name); } catch { /* the tab just isn't remembered */ }
  const i = TABS.indexOf(name);
  $('.tabs').style.setProperty('--tab', i);
  for (const b of $$('[data-tab]')) { b.classList.toggle('on', b.dataset.tab === name); b.setAttribute('aria-selected', b.dataset.tab === name); }

  clearTimeout(tabDone);
  for (const p of $$('.pane')) p.classList.remove('on', 'leaving', 'in-left', 'in-right');
  if (!animate || !oldPane) { newPane.classList.add('on'); stage.style.height = ''; return; }

  const h0 = stage.offsetHeight;
  oldPane.classList.add('leaving');
  newPane.classList.add('on', i > TABS.indexOf(from) ? 'in-right' : 'in-left');
  const h1 = newPane.offsetHeight;
  sizeLock++;
  bridge.act('size', bodyHeight() - stage.offsetHeight + Math.max(h0, h1));
  stage.style.height = `${h0}px`;
  void stage.offsetHeight; // start the height animation from the old tab's height
  stage.style.height = `${h1}px`;
  tabDone = setTimeout(() => {
    oldPane.classList.remove('leaving');
    stage.style.height = '';
    sizeLock = Math.max(0, sizeLock - 1);
    bridge.act('size', bodyHeight());
  }, 280);
}
for (const b of $$('[data-tab]')) b.addEventListener('click', () => showTab(b.dataset.tab));
addEventListener('keydown', (e) => { // Ctrl+Tab / Ctrl+Shift+Tab move between tabs
  if (e.key === 'Tab' && e.ctrlKey) { e.preventDefault(); showTab(TABS[(TABS.indexOf(tab) + (e.shiftKey ? TABS.length - 1 : 1)) % TABS.length]); }
});
let lastTab = 'show';
try { lastTab = localStorage.getItem('tab') || 'show'; } catch { /* first tab then */ }
showTab(lastTab, false);

/* ---------- Opening and closing ----------
   The panel grows out of the card's side; main says which side. Closing plays the reverse first. */
document.documentElement.dataset.side = new URLSearchParams(location.search).get('side') || 'right';
const opening = () => $('#root').classList.add('open');
bridge.onOpening(opening);
setTimeout(opening, 400); // in case the "now on screen" message never comes, show the panel anyway
bridge.onClosing(() => $('#root').classList.add('closing'));

bridge.onState(render);
