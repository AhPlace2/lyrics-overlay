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
  built = true;
}

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
  for (const b of $('[data-choose="accent"]').children) {
    b.classList.toggle('on', rainbowOn ? b.dataset.value === 'rainbow' : b.dataset.value === settings.accent);
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

// Let the window grow or shrink with what is shown.
new ResizeObserver(() => bridge.act('size', Math.ceil(document.body.getBoundingClientRect().height))).observe(document.body);
bridge.onState(render);
