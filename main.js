'use strict';
const { app, BrowserWindow, screen, Tray, Menu, nativeImage, globalShortcut, ipcMain } = require('electron');
const path = require('node:path');
const fs = require('node:fs');
const crypto = require('node:crypto');
const { spawn } = require('node:child_process');
const readline = require('node:readline');
const { findLyrics, findVersions, sameTiming } = require('./lyrics');
const { findCover } = require('./cover');

const DEFAULTS = {
  enabled: true,
  showCard: true,
  showCardLyrics: true, // the lyric lines under the card
  showBig: true,
  // Big lyrics style: a look, a colour and a motion, picked separately so they combine.
  look: 'depth',
  // 'accent': the colour from the picker. 'rainbow': cycles through colours. 'fade': flows between fadeFrom and
  // fadeTo. 'cover': the main colour of the playing song's cover.
  colourMode: 'accent',
  motion: 'none',
  font: 'clean',
  accent: '#ffb648',
  history: [], // the last two colours picked before this one
  fadeFrom: '#ff6fae',
  fadeTo: '#6cc8ff',
  bigPosition: 'center',
  offsets: {},
  picks: {}, // song -> the lyrics version chosen with "Try other lyrics"
  card: { x: 24, y: 24 },
};
const LOOKS = [['depth', 'Depth (3D)'], ['glow', 'Glow'], ['outline', 'Outline'], ['plain', 'Plain']];
const MOTIONS = [['none', 'None'], ['wave', 'Wave'], ['bounce', 'Bounce'], ['pulse', 'Pulse']];
const COLOUR_MODES = [['accent', 'Colour'], ['rainbow', 'Rainbow'], ['fade', 'Fade'], ['cover', 'From the cover']];
// The single "effect" setting from before, translated into look, colour and motion.
const OLD_EFFECTS = {
  depth: { look: 'depth', colourMode: 'accent', motion: 'none' },
  glow: { look: 'glow', colourMode: 'accent', motion: 'none' },
  rainbow: { look: 'depth', colourMode: 'rainbow', motion: 'none' },
  wave: { look: 'glow', colourMode: 'accent', motion: 'wave' },
  plain: { look: 'plain', colourMode: 'accent', motion: 'none' },
};
const FONTS = [['clean', 'Clean'], ['serif', 'Serif'], ['pixel', 'Pixel']];
const POSITIONS = [['center', 'Middle of the screen'], ['bottom', 'Lower third']];

let win = null, panel = null, tray = null, helper = null, quitting = false, editing = false;
let settings = { ...DEFAULTS };
let lastState = null, lastCover = null, lastLyrics = null, lookingFor = null, coverTimer = null;
const lyricsCache = new Map(), versionsCache = new Map();
let searchingVersions = false;
const failedHotkeys = [];

// Same settings folder whether run from the project or installed (the installed app's name has a space).
app.setPath('userData', path.join(app.getPath('appData'), 'lyrics-overlay'));

if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on('second-instance', () => send('toast', 'Lyrics overlay is already running. Use the tray icon to change it.'));
  app.whenReady().then(start);
}

/* ---------- Settings ---------- */
const settingsFile = () => path.join(app.getPath('userData'), 'settings.json');

function loadSettings() {
  try {
    const saved = JSON.parse(fs.readFileSync(settingsFile(), 'utf8'));
    delete saved.lead; // Timing used to be one setting for every song; it is per song now.
    if (saved.effect) Object.assign(saved, saved.look ? {} : OLD_EFFECTS[saved.effect] || {});
    delete saved.effect;
    return { ...DEFAULTS, ...saved, offsets: saved.offsets || {}, picks: saved.picks || {} };
  } catch { return { ...DEFAULTS }; }
}

function saveSettings() {
  try {
    fs.mkdirSync(path.dirname(settingsFile()), { recursive: true });
    fs.writeFileSync(settingsFile(), JSON.stringify(settings, null, 2));
  } catch (err) { console.error('Could not save settings:', err.message); }
}

function update(patch) {
  settings = { ...settings, ...patch };
  saveSettings();
  send('settings', settings);
  buildMenu();
}

function send(channel, data) {
  if (win && !win.isDestroyed()) win.webContents.send(channel, data);
}

/* ---------- Overlay window ---------- */
function fitToScreen() {
  // The work area leaves the taskbar out, so Windows doesn't treat the overlay as a fullscreen app.
  const { x, y, width, height } = screen.getPrimaryDisplay().workArea;
  win.setBounds({ x, y, width, height });
}

function createWindow() {
  win = new BrowserWindow({
    transparent: true,
    frame: false,
    resizable: false,
    movable: false,
    minimizable: false,
    maximizable: false,
    fullscreenable: false,
    focusable: false,
    skipTaskbar: true,
    hasShadow: false,
    alwaysOnTop: true,
    backgroundColor: '#00000000',
    show: false,
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      backgroundThrottling: false,
    },
  });
  fitToScreen();
  win.setAlwaysOnTop(true, 'screen-saver');
  // Clicks pass through to the desktop, except over the card (see "Click-through" below).
  win.setIgnoreMouseEvents(true);
  setInterval(updateCatching, 33);
  win.loadFile(path.join(__dirname, 'overlay.html'));
  win.webContents.on('did-finish-load', () => {
    send('settings', settings);
    if (lastState) send('state', lastState);
    if (lastCover) send('cover', lastCover);
    if (coverColour) send('cover-colour', coverColour);
    if (lastLyrics) send('lyrics', lastLyrics);
    if (failedHotkeys.length) send('toast', `These shortcuts are taken by another app: ${failedHotkeys.join(', ')}`);
    win.showInactive();
  });
  screen.on('display-metrics-changed', fitToScreen);
  screen.on('display-added', fitToScreen);
  screen.on('display-removed', fitToScreen);
}

/* ---------- Click-through ----------
   The overlay lets clicks through to the desktop, except over the card. Electron can forward every mouse move to
   the page so it can tell when the pointer is on the card, but with a fast gaming mouse that is thousands of
   messages a second, which kept the app busy. Instead main looks where the pointer is 30 times a second and
   catches the mouse only while it is over the card (the page reports where the card is, or null when the card
   can't show). While caught, the page gets the mouse as usual; it holds on during drags and seeks. */
let cardRect = null, pageHold = false, catching = false;

function pointerOverCard() {
  if (!cardRect || !win || win.isDestroyed()) return false;
  const p = screen.getCursorScreenPoint(), b = win.getBounds();
  const x = p.x - b.x, y = p.y - b.y;
  return x >= cardRect.x && x < cardRect.x + cardRect.width && y >= cardRect.y && y < cardRect.y + cardRect.height;
}

function updateCatching() {
  if (!win || win.isDestroyed()) return;
  const want = editing || pageHold || pointerOverCard();
  if (want === catching) return;
  catching = want;
  win.setIgnoreMouseEvents(!want);
  if (!want) send('pointer-left');
}

function setEditing(on) {
  editing = on;
  updateCatching();
  send('edit', on);
  buildMenu();
}

/* ---------- Now playing ---------- */
// In the installed app the code sits inside app.asar, which PowerShell can't read, so the script is kept beside it.
const helperScript = path.join(__dirname.replace(/app\.asar$/, 'app.asar.unpacked'), 'nowplaying.ps1');

function startHelper() {
  helper = spawn('powershell.exe', ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', helperScript, '-ParentPid', String(process.pid)], { windowsHide: true });
  readline.createInterface({ input: helper.stdout }).on('line', onHelperLine);
  helper.stderr.on('data', (d) => console.error('[now playing]', String(d).trim()));
  helper.on('exit', (code) => {
    helper = null;
    if (!quitting) {
      console.error(`[now playing] stopped (code ${code}), restarting`);
      setTimeout(startHelper, 2000);
    }
  });
}

/* Playback controls from the card go to the helper, which passes them to the player. */
const CONTROL_REFUSED = {
  toggle: "The player didn't pause or play",
  next: 'The player has no next song here',
  prev: 'The player has no previous song here',
  seek: "This player doesn't allow jumping to a time",
};

function control(msg) {
  if (!CONTROL_REFUSED[msg?.cmd] || typeof msg.key !== 'string') return;
  if (msg.cmd === 'seek' && !(Number.isFinite(msg.to) && msg.to >= 0)) return;
  if (!helper?.stdin?.writable) return send('toast', "Can't reach the player right now");
  const out = { cmd: msg.cmd, key: msg.key };
  if (msg.cmd === 'seek') out.to = Math.round(msg.to * 1000) / 1000;
  helper.stdin.write(JSON.stringify(out) + '\n');
}

function onHelperLine(line) {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  if (msg.type === 'cover') {
    setCover(msg);
    return;
  }
  if (msg.type === 'control') {
    if (!msg.ok) send('toast', CONTROL_REFUSED[msg.cmd] || "The player didn't respond");
    return;
  }
  if (msg.type !== 'state') return;
  const changed = msg.active && msg.key !== lastState?.key;
  lastState = msg;
  send('state', msg);
  if (changed) { lookUp(msg); buildMenu(); scheduleCoverLookup(msg); }
}

function setCover(cover) {
  lastCover = cover;
  send('cover', cover);
  let colour = null;
  try { colour = colourOfCover(cover.data); } catch (err) { console.error('Could not read the cover colour:', err.message); }
  coverColour = colour ? { key: cover.key, colour } : null;
  if (coverColour) send('cover-colour', coverColour);
  refreshPanel();
}

/* ---------- The cover's colour, for the "From the cover" colour ----------
   An average of the cover that counts its lively pixels most, so a bright patch on a dark cover beats the murky
   mean, then made light and saturated enough to read well as text over anything. */
let coverColour = null; // { key, colour }
function colourOfCover(dataUrl) {
  const img = nativeImage.createFromDataURL(dataUrl);
  if (img.isEmpty()) return null;
  const px = img.resize({ width: 24, height: 24, quality: 'good' }).toBitmap(); // BGRA
  let r = 0, g = 0, b = 0, total = 0;
  for (let i = 0; i + 3 < px.length; i += 4) {
    const B = px[i], G = px[i + 1], R = px[i + 2];
    const max = Math.max(R, G, B), sat = max ? (max - Math.min(R, G, B)) / max : 0;
    const weight = .04 + sat * sat * (max / 255); // grey and dark pixels count for little
    r += R * weight; g += G * weight; b += B * weight; total += weight;
  }
  return total ? readableColour(r / total, g / total, b / total) : null;
}

function readableColour(r, g, b) {
  r /= 255; g /= 255; b /= 255;
  const max = Math.max(r, g, b), min = Math.min(r, g, b), d = max - min;
  let h = 0, s = 0, l = (max + min) / 2;
  if (d) {
    s = d / (1 - Math.abs(2 * l - 1));
    h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
    h = (h * 60 + 360) % 360;
  }
  if (s > .12) s = Math.max(s, .65); // a real colour: make it vivid; a grey cover stays a light grey
  l = Math.min(.8, Math.max(.64, l));
  const k = (n) => (n + h / 30) % 12, a = s * Math.min(l, 1 - l);
  const f = (n) => Math.round((l - a * Math.max(-1, Math.min(k(n) - 3, 9 - k(n), 1))) * 255);
  return '#' + [f(0), f(8), f(4)].map((x) => x.toString(16).padStart(2, '0')).join('');
}

/* Picker colours. A colour counts as picked (and the one before it goes into the history) when the picker
   is let go of; the many colours sent while dragging only show it. */
let committedAccent = null;
function chooseAccent(hex, commit) {
  const patch = { accent: hex, colourMode: 'accent' };
  if (commit) {
    const before = committedAccent;
    const rest = (settings.history || []).filter((c) => c !== hex && c !== before);
    patch.history = (before && before !== hex ? [before, ...rest] : rest).slice(0, 2);
    committedAccent = hex;
  }
  update(patch);
}

/** If the player shares no cover within a few seconds, look one up online. */
function scheduleCoverLookup(state) {
  clearTimeout(coverTimer);
  const missing = () => lastState?.key === state.key && lastCover?.key !== state.key;
  coverTimer = setTimeout(async () => {
    if (!missing()) return;
    const data = await findCover(state).catch((err) => { console.error('Cover lookup failed:', err.message); return null; });
    if (data && missing()) setCover({ type: 'cover', key: state.key, data });
  }, 5000);
}

/* Lyrics found once are saved on disk, so the song loads instantly next time, even offline. */
const SAVED_LYRICS_VERSION = 1;
const savedLyricsFile = (state) =>
  path.join(app.getPath('userData'), 'lyrics-cache', `${crypto.createHash('sha1').update(songKey(state)).digest('hex')}.json`);

function loadSavedLyrics(state) {
  try {
    const saved = JSON.parse(fs.readFileSync(savedLyricsFile(state), 'utf8'));
    const pick = settings.picks[songKey(state)];
    // Only use it when it is the version this song should show now: the picked one, or the first one.
    if (saved.v !== SAVED_LYRICS_VERSION || saved.key !== songKey(state)) return null;
    if (pick ? saved.value.pick !== pick : saved.chosen) return null;
    return saved.value;
  } catch {
    return null;
  }
}

function saveLyrics(state, value) {
  if (value.status !== 'found') return;
  const key = songKey(state), file = savedLyricsFile(state);
  const data = JSON.stringify({ v: SAVED_LYRICS_VERSION, key, chosen: !!settings.picks[key], value });
  fs.promises.mkdir(path.dirname(file), { recursive: true })
    .then(() => fs.promises.writeFile(file, data))
    .catch((err) => console.error('Saving lyrics failed:', err.message));
}

async function lookUp(state, force = false) {
  const key = state.key;
  lookingFor = key;
  const publish = (value) => {
    lastLyrics = { key, ...value };
    if (lookingFor === key) send('lyrics', lastLyrics);
    refreshPanel();
  };
  if (!state.title) return publish({ status: 'none', lines: [] });
  if (!force && lyricsCache.has(key)) return publish(lyricsCache.get(key));
  const stored = !force && loadSavedLyrics(state);
  if (stored) {
    lyricsCache.set(key, stored);
    return publish(stored);
  }
  publish({ status: 'loading', lines: [] });
  try {
    let result = await findLyrics(trackOf(state));
    // A version picked earlier with "Try other lyrics" wins over the default one.
    const saved = settings.picks[songKey(state)];
    if (saved && result?.pick !== saved) result = (await versionsOf(state)).find((v) => v.pick === saved) || result;
    const value = result?.lines?.length
      ? { status: 'found', lines: result.lines, source: result.source, pick: result.pick, provider: result.provider }
      : { status: result?.plainOnly ? 'unsynced' : 'none', lines: [] };
    lyricsCache.set(key, value);
    saveLyrics(state, value);
    publish(value);
  } catch (err) {
    console.error('Lyrics lookup failed:', err.message);
    publish({ status: 'error', lines: [] });
  }
}

const trackOf = (state) => ({ title: state.title, artist: state.artist, duration: state.end - state.start });

async function versionsOf(state) {
  if (!versionsCache.has(state.key)) versionsCache.set(state.key, findVersions(trackOf(state)).catch((err) => { versionsCache.delete(state.key); throw err; }));
  return versionsCache.get(state.key);
}

/** Switches to the next (or, with step -1, the previous) differently timed version of this song's lyrics. */
async function tryOtherLyrics(step = 1) {
  const state = lastState;
  if (!state?.active) return send('toast', 'Nothing is playing');
  if (searchingVersions) return;
  searchingVersions = true;
  if (!versionsCache.has(state.key)) send('toast', 'Looking for other lyrics…');
  try {
    const versions = await versionsOf(state);
    if (lastState?.key !== state.key) return;
    const current = lastLyrics?.key === state.key && lastLyrics.status === 'found' ? lastLyrics : null;
    const at = current ? versions.findIndex((v) => v.pick === current.pick) : -1;
    const differs = (v) => !current || !sameTiming(v.lines, current.lines);
    const next = step > 0 ? versions.slice(at + 1).find(differs) : (at > 0 ? versions.slice(0, at).reverse().find(differs) : null);
    if (!next) return send('toast', step > 0 ? 'Sorry, no other lyrics found for this song' : 'Already on the first lyrics version');
    const value = { status: 'found', lines: next.lines, source: next.source, pick: next.pick, provider: next.provider };
    lyricsCache.set(state.key, value);
    lastLyrics = { key: state.key, ...value };
    send('lyrics', lastLyrics);
    // A timing fix made for the old version would be wrong for this one.
    const key = songKey(state), offsets = { ...settings.offsets };
    delete offsets[key];
    update({ picks: { ...settings.picks, [key]: next.pick }, offsets });
    saveLyrics(state, value); // after the pick is stored, so the file is marked as the chosen version
    send('toast', `Lyrics version ${versions.indexOf(next) + 1} of ${versions.length} (${next.provider})`);
  } catch (err) {
    console.error('Other lyrics failed:', err.message);
    send('toast', "Couldn't reach the lyrics sites");
  } finally {
    searchingVersions = false;
  }
}

function useFirstLyrics() {
  if (!lastState?.active) return;
  const picks = { ...settings.picks };
  delete picks[songKey(lastState)];
  update({ picks });
  lyricsCache.delete(lastState.key);
  lookUp(lastState, true);
  send('toast', 'Back to the first lyrics for this song');
}

/* ---------- Tray and shortcuts ---------- */
function trayIcon() {
  const size = 32, buf = Buffer.alloc(size * size * 4);
  const bars = [[9, 13, 8, 24], [15, 19, 8, 21], [21, 25, 8, 16]];
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const i = (y * size + x) * 4;
      const dx = Math.max(7 - x, x - (size - 8), 0), dy = Math.max(7 - y, y - (size - 8), 0);
      if (dx * dx + dy * dy > 49) continue;
      const white = bars.some(([y0, y1, x0, x1]) => y >= y0 && y < y1 && x >= x0 && x < x1);
      buf[i] = white ? 255 : 0x48; buf[i + 1] = white ? 255 : 0xb6; buf[i + 2] = 255; buf[i + 3] = 255;
    }
  }
  return nativeImage.createFromBitmap(buf, { width: size, height: size, scaleFactor: 2 });
}

/* Timing fixes are saved per song, keyed the same way the overlay keys them. */
const songKey = (s) => `${(s?.artist || '').toLowerCase()}|${(s?.title || '').toLowerCase()}`;
const songLead = () => (lastState?.active ? settings.offsets[songKey(lastState)] || 0 : 0);

function setLead(key, lead, message) {
  lead = Math.max(-120, Math.min(120, Math.round(lead * 100) / 100));
  const offsets = { ...settings.offsets };
  delete offsets[key];
  if (lead !== 0) offsets[key] = lead;
  const keys = Object.keys(offsets);
  if (keys.length > 300) delete offsets[keys[0]]; // Keep the file small; the oldest fix goes first.
  update({ offsets });
  send('toast', message || (lead === 0 ? 'Lyrics timing reset for this song' : `Lyrics ${lead > 0 ? 'earlier' : 'later'} by ${Math.abs(lead).toFixed(2)} s for this song`));
}

function nudge(delta) {
  if (!lastState?.active) return send('toast', 'Nothing is playing');
  setLead(songKey(lastState), songLead() + delta);
}

function toggleEnabled() {
  update({ enabled: !settings.enabled });
  send('toast', settings.enabled ? 'Lyrics on' : 'Lyrics hidden');
}

/* ---------- Settings panel (opened from the gear on the card) ---------- */
const PANEL_WIDTH = 368; // 340 px panel plus room for its shadow

function panelState() {
  const s = lastState?.active ? lastState : null;
  const lyrics = s && lastLyrics?.key === s.key ? lastLyrics : null;
  return {
    settings,
    options: { looks: LOOKS, motions: MOTIONS, fonts: FONTS, positions: POSITIONS },
    coverColour: coverColour?.colour || null,
    song: s && {
      title: s.title || 'Unknown title', artist: s.artist || '', lead: songLead(),
      picked: !!settings.picks[songKey(s)],
      status: lyrics?.status || 'loading',
      provider: lyrics?.status === 'found' ? lyrics.provider || 'LRCLIB' : null,
    },
  };
}

function refreshPanel() {
  if (panel && !panel.isDestroyed()) panel.webContents.send('panel-state', panelState());
}

/** Plays the panel's closing animation, then closes it. */
function closePanel(w = panel) {
  if (!w || w.isDestroyed() || w.closingSoon) return;
  w.closingSoon = true;
  w.webContents.send('panel-closing');
  setTimeout(() => { if (!w.isDestroyed()) w.close(); }, 170);
}

/** `at` is where the card is on screen right now. */
function togglePanel(at) {
  if (panel && !panel.isDestroyed()) return closePanel();
  const area = screen.getPrimaryDisplay().workArea;
  const card = Number.isFinite(at?.x) && Number.isFinite(at?.y) ? at : settings.card;
  // Next to the card, or on its left when there is no room on the right.
  let x = area.x + card.x + 340, side = 'right';
  if (x + PANEL_WIDTH > area.x + area.width) { x = area.x + card.x - PANEL_WIDTH; side = 'left'; }
  const w = new BrowserWindow({
    x: Math.max(area.x, Math.round(x)), y: area.y + Math.max(0, Math.round(card.y) - 10), width: PANEL_WIDTH, height: 600,
    transparent: true, frame: false, resizable: false, minimizable: false, maximizable: false, fullscreenable: false,
    skipTaskbar: true, hasShadow: false, show: false, backgroundColor: '#00000000',
    webPreferences: { preload: path.join(__dirname, 'preload-settings.js'), contextIsolation: true, nodeIntegration: false },
  });
  panel = w;
  w.setAlwaysOnTop(true, 'screen-saver');
  w.loadFile(path.join(__dirname, 'settings.html'), { query: { side } });
  // The opening animation starts once the window is on screen, so it isn't over before anyone sees it.
  w.webContents.on('did-finish-load', () => { refreshPanel(); w.show(); w.webContents.send('panel-opening'); });
  w.on('blur', () => closePanel(w)); // closes like a menu when you click elsewhere
  // Only forget this window if it is still the current one; a panel closed late must not orphan a newer one.
  w.on('closed', () => { if (panel === w) { panel = null; send('panel-open', false); } });
  send('panel-open', true);
}

function sizePanel(height) {
  if (!panel || panel.isDestroyed() || !Number.isFinite(height)) return;
  const area = screen.getPrimaryDisplay().workArea;
  const b = panel.getBounds();
  const h = Math.min(Math.max(120, Math.round(height)), area.height);
  panel.setBounds({ ...b, height: h, y: Math.min(b.y, area.y + area.height - h) });
}

function onPanelAction(name, value) {
  const choices = { look: LOOKS, motion: MOTIONS, colourMode: COLOUR_MODES, font: FONTS, bigPosition: POSITIONS };
  const isHex = (v) => typeof v === 'string' && /^#[0-9a-f]{6}$/i.test(v);
  switch (name) {
    case 'toggle':
      if (value === 'enabled') toggleEnabled();
      else if (['showCard', 'showCardLyrics', 'showBig'].includes(value)) update({ [value]: !settings[value] });
      break;
    case 'choose':
      // Colours from the picker come as #rrggbb, and switch to the mode they belong to so they show straight away.
      if (value?.key === 'accent' && isHex(value.value)) chooseAccent(value.value.toLowerCase(), !!value.commit);
      else if ((value?.key === 'fadeFrom' || value?.key === 'fadeTo') && isHex(value.value)) update({ [value.key]: value.value.toLowerCase(), colourMode: 'fade' });
      else if (choices[value?.key]?.some(([v]) => v === value.value)) update({ [value.key]: value.value });
      break;
    case 'earlier': nudge(.25); break;
    case 'later': nudge(-.25); break;
    case 'sync': send('sync-now'); break;
    case 'reset': nudge(-songLead()); break;
    case 'next': tryOtherLyrics(1); break;
    case 'prev': tryOtherLyrics(-1); break;
    case 'first': useFirstLyrics(); break;
    case 'search': if (lastState?.active) lookUp(lastState, true); break;
    case 'move': panel?.close(); setEditing(true); break;
    case 'quit': app.quit(); break;
    case 'close': closePanel(); break;
    case 'size': sizePanel(value); break;
  }
}

function buildMenu() {
  refreshPanel();
  if (!tray) return;
  // Just the essentials; everything else lives in the settings panel.
  tray.setContextMenu(Menu.buildFromTemplate([
    { label: 'Settings…', click: () => togglePanel() },
    { label: 'Show lyrics', type: 'checkbox', checked: settings.enabled, accelerator: 'Control+Alt+L', click: toggleEnabled },
    { type: 'separator' },
    { label: 'Quit', click: () => app.quit() },
  ]));
}

function registerHotkeys() {
  const keys = {
    'Control+Alt+L': toggleEnabled,
    // Arrow keys, because brackets need AltGr on many non-US keyboards.
    'Control+Alt+Up': () => nudge(.25),
    'Control+Alt+Down': () => nudge(-.25),
    'Control+Alt+M': () => setEditing(!editing),
    'Control+Alt+S': () => send('sync-now'),
    'Control+Alt+O': () => tryOtherLyrics(1),
    'Control+Alt+P': () => tryOtherLyrics(-1),
  };
  for (const [accel, fn] of Object.entries(keys)) {
    try { if (!globalShortcut.register(accel, fn)) failedHotkeys.push(accel); }
    catch { failedHotkeys.push(accel); }
  }
}

/* ---------- Startup ---------- */
function start() {
  settings = loadSettings();
  committedAccent = settings.accent;
  app.setAppUserModelId('Lyrics Overlay');
  createWindow();
  tray = new Tray(trayIcon());
  tray.setToolTip('Lyrics Overlay');
  tray.on('click', () => tray.popUpContextMenu());
  registerHotkeys();
  buildMenu();
  startHelper();

  ipcMain.on('card-moved', (_e, pos) => {
    if (Number.isFinite(pos?.x) && Number.isFinite(pos?.y)) update({ card: { x: Math.round(pos.x), y: Math.round(pos.y) } });
  });
  ipcMain.on('edit-done', () => setEditing(false));
  ipcMain.on('open-settings', (_e, at) => togglePanel(at));
  ipcMain.on('control', (e, msg) => { if (win && e.sender === win.webContents) control(msg); });
  ipcMain.on('ui-hover', (_e, on) => { pageHold = !!on; updateCatching(); });
  ipcMain.on('card-rect', (_e, r) => {
    cardRect = r && [r.x, r.y, r.width, r.height].every(Number.isFinite) ? r : null;
    updateCatching();
  });
  ipcMain.on('panel-action', (e, msg) => { if (panel && e.sender === panel.webContents) onPanelAction(msg?.name, msg?.value); });
  ipcMain.on('set-offset', (_e, msg) => {
    if (typeof msg?.key === 'string' && Number.isFinite(msg.lead)) {
      setLead(msg.key, msg.lead, msg.lead === 0 ? 'Lyrics already line up here' : `Synced: lyrics ${msg.lead > 0 ? 'earlier' : 'later'} by ${Math.abs(msg.lead).toFixed(2)} s for this song`);
    }
  });
}

app.on('window-all-closed', (e) => e.preventDefault());
app.on('before-quit', () => {
  quitting = true;
  globalShortcut.unregisterAll();
  if (helper) helper.kill();
});
