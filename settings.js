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
  chips('font', options.fonts, (v) => `font-${v}`);
  chips('bigPosition', options.positions.map(([v, l]) => [v, l.replace('Middle of the screen', 'Middle')]));
  const swatches = $('[data-choose="accent"]');
  for (const [value, label] of options.accents) {
    const b = document.createElement('button');
    b.type = 'button'; b.dataset.value = value; b.title = label; b.style.background = value;
    b.addEventListener('click', () => bridge.act('choose', { key: 'accent', value }));
    swatches.append(b);
  }
  const rainbow = document.createElement('button');
  rainbow.type = 'button'; rainbow.dataset.value = 'rainbow'; rainbow.title = 'Rainbow'; rainbow.className = 'rainbow';
  rainbow.addEventListener('click', () => bridge.act('choose', { key: 'colourMode', value: 'rainbow' }));
  swatches.append(rainbow);
  const custom = document.createElement('button');
  custom.type = 'button'; custom.dataset.value = 'custom'; custom.title = 'Your own colour'; custom.className = 'custom';
  custom.addEventListener('click', () => togglePicker());
  swatches.append(custom);
  presets = options.accents.map(([v]) => v.toLowerCase());
  built = true;
}

/* ---------- Colour picker ---------- */
let presets = [], hsv = { h: 0, s: 0, v: 1 }, dragging = false, sentAt = 0, sendTimer = null, lastAccent = null;

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
function pickColour(hex, now = false) {
  clearTimeout(sendTimer);
  const go = () => { sentAt = Date.now(); bridge.act('choose', { key: 'accent', value: hex }); };
  if (now || Date.now() - sentAt > 60) go();
  else sendTimer = setTimeout(go, 60);
}

function togglePicker(open = $('#picker').classList.contains('hidden')) {
  $('#picker').classList.toggle('hidden', !open);
  $('[data-value="custom"]').classList.toggle('open', open);
  if (open && lastAccent) { hsv = hexToHsv(lastAccent); drawPicker(); }
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

$('#hex').addEventListener('input', () => {
  let v = $('#hex').value.trim();
  if (!v.startsWith('#')) v = '#' + v;
  if (/^#[0-9a-f]{3}$/i.test(v)) v = '#' + [...v.slice(1)].map((c) => c + c).join('');
  const ok = /^#[0-9a-f]{6}$/i.test(v);
  $('#hex').classList.toggle('bad', !ok);
  if (!ok) return;
  hsv = hexToHsv(v.toLowerCase());
  drawPicker(false);
  pickColour(v.toLowerCase(), true);
});
// Esc in the colour code field closes the panel like everywhere else; Enter just confirms.
$('#hex').addEventListener('keydown', (e) => { if (e.key === 'Enter') $('#hex').blur(); });

function render({ settings, options, song }) {
  if (!built) build(options);
  document.documentElement.style.setProperty('--accent', settings.accent);
  for (const box of $$('input[data-toggle]')) box.checked = !!settings[box.dataset.toggle];
  for (const row of $$('.row.sub')) row.classList.toggle('off', !settings.showCard); // card options only matter with the card on
  for (const box of $$('[data-choose]')) {
    for (const b of box.children) b.classList.toggle('on', settings[box.dataset.choose] === b.dataset.value);
  }
  // Colour row: the rainbow swatch, or the picked colour when not in rainbow mode.
  const rainbowOn = settings.colourMode === 'rainbow';
  const accent = String(settings.accent).toLowerCase(), own = !presets.includes(accent);
  for (const b of $('[data-choose="accent"]').children) {
    const v = b.dataset.value;
    b.classList.toggle('on', rainbowOn ? v === 'rainbow' : v === 'custom' ? own : v.toLowerCase() === accent);
  }
  const custom = $('[data-value="custom"]');
  custom.classList.toggle('filled', own);
  custom.style.background = own ? accent : '';
  // Keep the picker in step with colours chosen elsewhere, but never fight a drag in progress.
  if (accent !== lastAccent) {
    lastAccent = accent;
    if (!dragging && document.activeElement !== $('#hex')) { hsv = hexToHsv(accent); drawPicker(); }
  }
  $('#song').classList.toggle('hidden', !song);
  $('#nothing').classList.toggle('hidden', !!song);
  if (song) {
    $('#song-name').textContent = song.artist ? `${song.title} · ${song.artist}` : song.title;
    $('#lead').textContent = song.lead === 0 ? 'in sync' : `${Math.abs(song.lead).toFixed(2)} s ${song.lead > 0 ? 'earlier' : 'later'}`;
    $('#source').textContent = song.provider ? `from ${song.provider}` : song.status === 'loading' ? 'looking…' : 'none found';
    const synced = song.status === 'found';
    $('[data-act="reset"]').disabled = song.lead === 0;
    $('[data-act="sync"]').disabled = !synced;
    $('[data-act="first"]').disabled = !song.picked;
  }
}

for (const box of $$('input[data-toggle]')) box.addEventListener('change', () => bridge.act('toggle', box.dataset.toggle));
for (const b of $$('[data-act]')) b.addEventListener('click', () => bridge.act(b.dataset.act));
$('#close').addEventListener('click', () => bridge.act('close'));
addEventListener('keydown', (e) => { if (e.key === 'Escape') bridge.act('close'); });

/* ---------- Window size ----------
   The window grows or shrinks with what is shown. While a tab change animates the height, the window is
   sized once for the taller of the two tabs instead of on every frame, then fitted when it is done. */
let sizeLock = 0;
const bodyHeight = () => Math.ceil(document.body.getBoundingClientRect().height);
new ResizeObserver(() => { if (!sizeLock) bridge.act('size', bodyHeight()); }).observe(document.body);

/* ---------- Tabs ---------- */
const TABS = ['show', 'look', 'song'];
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
  if (e.key === 'Tab' && e.ctrlKey) { e.preventDefault(); showTab(TABS[(TABS.indexOf(tab) + (e.shiftKey ? 2 : 1)) % 3]); }
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
