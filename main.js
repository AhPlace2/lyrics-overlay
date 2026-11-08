'use strict';
const { app, BrowserWindow, screen, Tray, Menu, nativeImage, globalShortcut, ipcMain } = require('electron');
const path = require('node:path');
const fs = require('node:fs');
const { spawn } = require('node:child_process');
const readline = require('node:readline');
const { findLyrics, findVersions, sameTiming } = require('./lyrics');
const { findCover } = require('./cover');

const DEFAULTS = {
  enabled: true,
  showCard: true,
  showCardLyrics: true, // the lyric lines under the card
  seeThrough: true, // card turns see-through while the mouse is over it
  showBig: true,
  // Big lyrics style: a look, a colour and a motion, picked separately so they combine.
  look: 'depth',
  colourMode: 'accent', // 'accent' uses the colour picked below, 'rainbow' cycles through colours
  motion: 'none',
  font: 'clean',
  accent: '#ffb648',
  bigPosition: 'center',
  offsets: {},
  picks: {}, // song -> the lyrics version chosen with "Try other lyrics"
  card: { x: 24, y: 24 },
};
const LOOKS = [['depth', 'Depth (3D)'], ['glow', 'Glow'], ['outline', 'Outline'], ['plain', 'Plain']];
const MOTIONS = [['none', 'None'], ['wave', 'Wave'], ['bounce', 'Bounce'], ['pulse', 'Pulse']];
const COLOUR_MODES = [['accent', 'Colour'], ['rainbow', 'Rainbow']];
// The single "effect" setting from before, translated into look, colour and motion.
const OLD_EFFECTS = {
  depth: { look: 'depth', colourMode: 'accent', motion: 'none' },
  glow: { look: 'glow', colourMode: 'accent', motion: 'none' },
  rainbow: { look: 'depth', colourMode: 'rainbow', motion: 'none' },
  wave: { look: 'glow', colourMode: 'accent', motion: 'wave' },
  plain: { look: 'plain', colourMode: 'accent', motion: 'none' },
};
const FONTS = [['clean', 'Clean'], ['serif', 'Serif'], ['pixel', 'Pixel']];
const ACCENTS = [['#ffb648', 'Amber'], ['#5cf2b0', 'Mint'], ['#ff6fae', 'Pink'], ['#6cc8ff', 'Sky'], ['#ff4d5e', 'Red'], ['#ffffff', 'White']];
const POSITIONS = [['center', 'Middle of the screen'], ['bottom', 'Lower third']];

let win = null, panel = null, tray = null, helper = null, quitting = false, editing = false;
let settings = { ...DEFAULTS };
let lastState = null, lastCover = null, lastLyrics = null, lookingFor = null, coverTimer = null;
const lyricsCache = new Map(), versionsCache = new Map();
let searchingVersions = false;
const failedHotkeys = [];

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
  // Clicks pass through to the desktop, but the page still sees the pointer, so it can tell when it is over the gear.
  win.setIgnoreMouseEvents(true, { forward: true });
  win.loadFile(path.join(__dirname, 'overlay.html'));
  win.webContents.on('did-finish-load', () => {
    send('settings', settings);
    if (lastState) send('state', lastState);
    if (lastCover) send('cover', lastCover);
    if (lastLyrics) send('lyrics', lastLyrics);
    if (failedHotkeys.length) send('toast', `These shortcuts are taken by another app: ${failedHotkeys.join(', ')}`);
    win.showInactive();
  });
  screen.on('display-metrics-changed', fitToScreen);
  screen.on('display-added', fitToScreen);
  screen.on('display-removed', fitToScreen);
}

function setEditing(on) {
  editing = on;
  win.setIgnoreMouseEvents(!on, { forward: true });
  send('edit', on);
  buildMenu();
}

/* ---------- Now playing ---------- */
function startHelper() {
  helper = spawn('powershell.exe', ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', path.join(__dirname, 'nowplaying.ps1'), '-ParentPid', String(process.pid)], { windowsHide: true });
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

function onHelperLine(line) {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  if (msg.type === 'cover') {
    lastCover = msg;
    send('cover', msg);
    return;
  }
  if (msg.type !== 'state') return;
  const changed = msg.active && msg.key !== lastState?.key;
  lastState = msg;
  send('state', msg);
  if (changed) { lookUp(msg); buildMenu(); scheduleCoverLookup(msg); }
}

/** If the player shares no cover within a few seconds, look one up online. */
function scheduleCoverLookup(state) {
  clearTimeout(coverTimer);
  const missing = () => lastState?.key === state.key && lastCover?.key !== state.key;
  coverTimer = setTimeout(async () => {
    if (!missing()) return;
    const data = await findCover(state).catch((err) => { console.error('Cover lookup failed:', err.message); return null; });
    if (data && missing()) { lastCover = { type: 'cover', key: state.key, data }; send('cover', lastCover); }
  }, 5000);
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
    options: { looks: LOOKS, motions: MOTIONS, fonts: FONTS, accents: ACCENTS, positions: POSITIONS },
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

function togglePanel() {
  if (panel && !panel.isDestroyed()) return panel.close();
  const area = screen.getPrimaryDisplay().workArea;
  // Next to the card, or on its left when there is no room on the right.
  let x = area.x + settings.card.x + 340;
  if (x + PANEL_WIDTH > area.x + area.width) x = area.x + settings.card.x - PANEL_WIDTH;
  const w = new BrowserWindow({
    x: Math.max(area.x, x), y: area.y + Math.max(0, settings.card.y - 10), width: PANEL_WIDTH, height: 600,
    transparent: true, frame: false, resizable: false, minimizable: false, maximizable: false, fullscreenable: false,
    skipTaskbar: true, hasShadow: false, show: false, backgroundColor: '#00000000',
    webPreferences: { preload: path.join(__dirname, 'preload-settings.js'), contextIsolation: true, nodeIntegration: false },
  });
  panel = w;
  w.setAlwaysOnTop(true, 'screen-saver');
  w.loadFile(path.join(__dirname, 'settings.html'));
  w.webContents.on('did-finish-load', () => { refreshPanel(); w.show(); });
  w.on('blur', () => { if (!w.isDestroyed()) w.close(); }); // closes like a menu when you click elsewhere
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
  const choices = { look: LOOKS, motion: MOTIONS, colourMode: COLOUR_MODES, font: FONTS, accent: ACCENTS, bigPosition: POSITIONS };
  switch (name) {
    case 'toggle':
      if (value === 'enabled') toggleEnabled();
      else if (['showCard', 'showCardLyrics', 'seeThrough', 'showBig'].includes(value)) update({ [value]: !settings[value] });
      break;
    case 'choose':
      if (!choices[value?.key]?.some(([v]) => v === value.value)) break;
      // Picking a colour swatch also leaves rainbow mode, so the pick shows straight away.
      update(value.key === 'accent' ? { accent: value.value, colourMode: 'accent' } : { [value.key]: value.value });
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
    case 'close': panel?.close(); break;
    case 'size': sizePanel(value); break;
  }
}

function buildMenu() {
  refreshPanel();
  if (!tray) return;
  // Just the essentials; everything else lives in the settings panel.
  tray.setContextMenu(Menu.buildFromTemplate([
    { label: 'Settings…', click: togglePanel },
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
  ipcMain.on('open-settings', togglePanel);
  ipcMain.on('ui-hover', (_e, on) => { if (!editing) win.setIgnoreMouseEvents(!on, { forward: true }); });
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
