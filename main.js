'use strict';
const { app, BrowserWindow, screen, Tray, Menu, nativeImage, globalShortcut, ipcMain, desktopCapturer, session } = require('electron');
const path = require('node:path');
const fs = require('node:fs');
const crypto = require('node:crypto');
const { spawn } = require('node:child_process');
const readline = require('node:readline');
const { findLyrics, findVersions, sameTiming } = require('./lyrics');
const { findCover } = require('./cover');
const shazam = require('./shazam');

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
  entrance: 'rise', // how each word of the big lyrics comes in
  // Big lyrics: where the middle of the lyrics sits (fractions of the screen), how wide the lines may run (a
  // fraction of the screen's width), the text size (1 = normal) and how lines line up.
  bigPos: { x: 0.5, y: 0.44 },
  bigWidth: 0.84,
  bigSize: 1,
  bigAlign: 'center',
  offsets: {},
  picks: {}, // song -> the lyrics version chosen with "Try other lyrics"
  // The card: where it is, how big (1 = normal), how solid its background is, which side of the song its
  // lyrics go, how many coming lines they show, and which of its parts show.
  card: { x: 24, y: 24 },
  cardScale: 1,
  cardOpacity: 0.84,
  cardLyricsSide: 'bottom',
  cardNext: 2,
  showCover: true,
  showArtist: true,
  showProgress: true,
  showControls: true,
  peekKey: 'ctrl', // hold this key alone to hide the card and click what's under it ('off' to never)
  recognise: true, // when no lyrics are found, listen to the song and ask Shazam what it really is
};
const PEEK_KEYS = [['off', 'Off'], ['ctrl', 'Ctrl'], ['shift', 'Shift'], ['alt', 'Alt']];
const LOOKS = [
  ['depth', 'Depth (3D)'], ['glow', 'Glow'], ['outline', 'Outline'], ['plain', 'Plain'],
];
const MOTIONS = [
  ['none', 'None'], ['wave', 'Wave'], ['bounce', 'Bounce'], ['pulse', 'Pulse'], ['float', 'Float'], ['flicker', 'Flicker'],
];
const ENTRANCES = [['rise', 'Rise'], ['pop', 'Pop'], ['slide', 'Slide'], ['drop', 'Drop'], ['fade', 'Fade']];
const SIDES = [['bottom', 'Below'], ['top', 'Above'], ['left', 'Left'], ['right', 'Right']];
const ALIGNS = [['left', 'Left'], ['center', 'Centre'], ['right', 'Right']];
// Number settings and the range each may take. Whole numbers where the step is 1.
const RANGES = {
  cardScale: [0.6, 2, 0.01], cardOpacity: [0, 1, 0.01], cardNext: [0, 4, 1],
  bigWidth: [0.25, 0.98, 0.01], bigSize: [0.4, 2.5, 0.01],
};
const TOGGLES = ['enabled', 'showCard', 'showCardLyrics', 'showBig', 'showCover', 'showArtist', 'showProgress', 'showControls', 'recognise'];
const COLOUR_MODES = [['accent', 'Colour'], ['rainbow', 'Rainbow'], ['fade', 'Fade'], ['cover', 'From the cover']];
// The single "effect" setting from before, translated into look, colour and motion.
const OLD_EFFECTS = {
  depth: { look: 'depth', colourMode: 'accent', motion: 'none' },
  glow: { look: 'glow', colourMode: 'accent', motion: 'none' },
  rainbow: { look: 'depth', colourMode: 'rainbow', motion: 'none' },
  wave: { look: 'glow', colourMode: 'accent', motion: 'wave' },
  plain: { look: 'plain', colourMode: 'accent', motion: 'none' },
};
const FONTS = [['clean', 'Clean'], ['serif', 'Serif'], ['pixel', 'Pixel'], ['bold', 'Poster'], ['round', 'Round'], ['hand', 'Hand']];

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
    // The big lyrics used to have two places to be; now they go anywhere.
    if (saved.bigPosition && !saved.bigPos) saved.bigPos = { x: 0.5, y: saved.bigPosition === 'bottom' ? 0.76 : 0.44 };
    delete saved.bigPosition;
    // A look or motion that has since been removed goes back to the default.
    if (!LOOKS.some(([v]) => v === saved.look)) delete saved.look;
    if (!MOTIONS.some(([v]) => v === saved.motion)) delete saved.motion;
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

/** Sets a number setting (kept in its range) or where the big lyrics are. Ignores anything else. */
function setValue(key, value) {
  if (key === 'bigPos') {
    if (!Number.isFinite(value?.x) || !Number.isFinite(value?.y)) return;
    const clamp = (v) => Math.round(Math.min(1, Math.max(0, v)) * 1000) / 1000;
    return update({ bigPos: { x: clamp(value.x), y: clamp(value.y) } });
  }
  const range = RANGES[key];
  if (!range || !Number.isFinite(value)) return;
  const [min, max, step] = range;
  update({ [key]: Number((Math.round(Math.min(max, Math.max(min, value)) / step) * step).toFixed(3)) });
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
    // A tool window. Chrome pauses video when it thinks a window fully covers it, and this window spans the
    // screen. Chrome ignores click-through windows, but over the card the overlay stops being click-through,
    // which froze YouTube while hovering or dragging the card. Chrome always ignores tool windows.
    type: 'toolbar',
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
// pageHold: the page has the pointer on the card. pageBusy: it is dragging the card or seeking, and must keep
// the mouse even if the pointer runs off the card or the card's hide key is held.
let cardRect = null, pageHold = false, pageBusy = false, catching = false;

function pointerOverCard() {
  if (!cardRect || !win || win.isDestroyed()) return false;
  const p = screen.getCursorScreenPoint(), b = win.getBounds();
  const x = p.x - b.x, y = p.y - b.y;
  return x >= cardRect.x && x < cardRect.x + cardRect.width && y >= cardRect.y && y < cardRect.y + cardRect.height;
}

function updateCatching() {
  if (!win || win.isDestroyed()) return;
  peekHeld = peekKeyHeld(Date.now());
  if (peekHeld && !editing && !pageBusy) setPeeking(true);
  const over = pointerOverCard();
  // A hidden card comes back once its key is let go and the pointer is off the card, so you can let go of the
  // key and still click (a plain click, not Ctrl+click) where the card was.
  if (peeking && !peekHeld && !over) setPeeking(false);
  const want = editing || pageBusy || (!peeking && (pageHold || over));
  if (want === catching) return;
  catching = want;
  win.setIgnoreMouseEvents(!want);
  if (!want) send('pointer-left');
}

/* ---------- Hide the card while a key is held ----------
   While the chosen modifier is held on its own, the card fades out and lets clicks through to what's under it.
   The overlay never has the keyboard, so on each check above it asks Windows which keys are down. */
let GetAsyncKeyState = null;
try {
  GetAsyncKeyState = require('koffi').load('user32.dll').func('short __stdcall GetAsyncKeyState(int vKey)');
} catch (e) {
  console.error('[keys] Hiding the card with a key is unavailable:', e.message);
}
const MODIFIER_KEYS = { ctrl: 0x11, shift: 0x10, alt: 0x12 };
const isDown = (vk) => (GetAsyncKeyState(vk) & 0x8000) !== 0;

/** 'ctrl', 'shift' or 'alt' when just that one is down, 'up' when none is, 'busy' when it's part of a shortcut. */
function modifierAlone() {
  const down = Object.keys(MODIFIER_KEYS).filter((k) => isDown(MODIFIER_KEYS[k]));
  if (!down.length) return 'up';
  if (down.length > 1) return 'busy'; // Ctrl+Alt for the overlay's own shortcuts, AltGr
  // Any other key down means a shortcut like Ctrl+C. 0x01-0x06 are the mouse buttons, which may be down: you
  // click while holding the key.
  for (let vk = 0x08; vk <= 0xfe; vk++) {
    if ((vk >= 0x10 && vk <= 0x12) || (vk >= 0xa0 && vk <= 0xa5)) continue; // the modifiers themselves
    if (isDown(vk)) return 'busy';
  }
  return down[0];
}

// Some keyboards (or their software) report a held key as quickly going up and down again, many times a
// second. So a key only counts as let go once it has stayed up for 250 ms, and it counts as held (not tapped)
// once it is still down 150 ms after it was first pressed, so quick shortcuts don't make the card blink.
let heldKey = 'none', heldSince = 0, lastDown = 0, peekHeld = false, peeking = false;

function peekKeyHeld(now) {
  if (!GetAsyncKeyState || !MODIFIER_KEYS[settings.peekKey]) { heldKey = 'none'; return false; }
  const key = modifierAlone();
  if (key === 'busy') heldKey = 'none';
  else if (key !== 'up') {
    if (key !== heldKey) { heldKey = key; heldSince = now; }
    lastDown = now;
  } else if (heldKey !== 'none' && now - lastDown >= 250) heldKey = 'none';
  return heldKey === settings.peekKey && lastDown - heldSince >= 150;
}

function setPeeking(on) {
  if (on === peeking) return;
  peeking = on;
  send('peek', on);
}

function setEditing(on) {
  editing = on;
  if (on) setPeeking(false);
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
    if (value.recognised) recognised.set(key, value.recognised);
    if (lookingFor === key) send('lyrics', lastLyrics);
    refreshPanel();
    if (value.status === 'none' || value.status === 'unsynced') recogniseSong(state);
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

// The song as the player names it, or as Shazam recognised it.
const trackOf = (state) => {
  const r = recognised.get(state.key);
  return { title: r?.title ?? state.title, artist: r?.artist ?? state.artist, duration: state.end - state.start };
};

/* ---------- Recognising songs without lyrics ----------
   When no lyrics are found, the name the player gives is often the problem (a YouTube video title, a re-upload,
   a mix). Then the app records ~11 s of what the computer plays and asks Shazam (shazam.js) what the song really
   is, and looks the lyrics up again under that name. Recording happens in a hidden page (listen.html), the only
   page allowed to capture the computer's sound. */
const LISTEN_MS = 11000, AUTO_TRIES = 3, RETRY_MS = 15000;
const recognised = new Map(); // player's song key -> { title, artist } from Shazam
const recogniseTries = new Map(); // player's song key -> automatic attempts so far
let listenWin = null, recognising = null; // recognising: the song key being listened to
const wait = (ms) => new Promise((r) => setTimeout(r, ms));

async function listenWindow() {
  if (listenWin && !listenWin.isDestroyed()) return listenWin;
  const w = new BrowserWindow({
    show: false, width: 200, height: 100, skipTaskbar: true,
    webPreferences: { contextIsolation: true, nodeIntegration: false, backgroundThrottling: false },
  });
  listenWin = w;
  w.on('closed', () => { if (listenWin === w) listenWin = null; });
  await w.loadFile(path.join(__dirname, 'listen.html'));
  return w;
}

/** Lets only the hidden listening page record, and only the computer's sound ("loopback"). */
function allowListening() {
  session.defaultSession.setDisplayMediaRequestHandler((request, callback) => {
    const wc = listenWin && !listenWin.isDestroyed() ? listenWin.webContents : null;
    const fromListen = wc && request.frame && request.frame.processId === wc.getProcessId() && request.frame.routingId === wc.mainFrame.routingId;
    if (!fromListen) return callback({});
    desktopCapturer.getSources({ types: ['screen'], thumbnailSize: { width: 0, height: 0 } })
      .then((sources) => callback(sources[0] ? { video: sources[0], audio: 'loopback' } : {}))
      .catch(() => callback({}));
  });
}

/**
 * Listens to the playing song and, when Shazam knows it, looks its lyrics up under the recognised name.
 * Automatic (no lyrics found): up to three tries per song, 15 s apart, as one clip may catch an intro, a quiet
 * part or a bit an altered upload changed. Manual (the "Recognise song" button): always, and it replaces lyrics
 * already shown.
 */
async function recogniseSong(state, manual = false) {
  const key = state?.key;
  if (!key || !state.title) return;
  if (recognising) { if (manual) send('toast', 'Already listening…'); return; }
  const tries = recogniseTries.get(key) || 0;
  if (!manual && (!settings.recognise || tries >= AUTO_TRIES)) return;
  if (!manual) recogniseTries.set(key, tries + 1);
  const still = () => lastState?.key === key;
  const lastTry = manual || tries + 1 >= AUTO_TRIES;
  const again = () => { if (!lastTry) setTimeout(() => { if (still() && lastLyrics?.key === key && lastLyrics.status !== 'found') recogniseSong(lastState); }, RETRY_MS); };
  recognising = key;
  refreshPanel();
  try {
    while (still() && lastState.status !== 'Playing') await wait(1000); // a paused song is only silence
    if (!still()) return;
    if (manual || tries === 0) send('toast', 'Listening to recognise this song…');
    const w = await listenWindow();
    const { samples, peak } = await w.webContents.executeJavaScript(`record(${LISTEN_MS})`, true);
    if (!still()) return;
    if (peak < 0.01) {
      if (lastTry) send('toast', "Couldn't hear anything playing");
      return again();
    }
    const match = await shazam.recognise(Int16Array.from(samples));
    console.log(`[recognise] "${state.title}": ${match ? `${match.artist} – ${match.title}` : 'no match'} (loudest ${peak.toFixed(2)})`);
    if (!still()) return;
    if (!match) {
      if (lastTry) send('toast', "Couldn't recognise this song");
      return again();
    }
    const name = match.artist ? `${match.artist} – ${match.title}` : match.title;
    recognised.set(key, { title: match.title, artist: match.artist });
    versionsCache.delete(key); // "Other lyrics" should search under the new name too
    const result = await findLyrics(trackOf(state));
    if (!still()) return;
    if (!result?.lines?.length) return send('toast', `Recognised “${name}”, but found no synced lyrics for it`);
    const value = {
      status: 'found', lines: result.lines, source: result.source, pick: result.pick, provider: result.provider,
      recognised: { title: match.title, artist: match.artist },
    };
    lyricsCache.set(key, value);
    saveLyrics(state, value);
    lastLyrics = { key, ...value };
    if (lookingFor === key) send('lyrics', lastLyrics);
    send('toast', `Recognised: ${name}`);
  } catch (err) {
    console.error('Recognising the song failed:', err.message);
    if (manual) send('toast', "Couldn't recognise this song right now");
  } finally {
    recognising = null;
    refreshPanel();
  }
}

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
    options: { looks: LOOKS, motions: MOTIONS, fonts: FONTS, entrances: ENTRANCES, sides: SIDES, aligns: ALIGNS, peekKeys: PEEK_KEYS },
    coverColour: coverColour?.colour || null,
    song: s && {
      title: s.title || 'Unknown title', artist: s.artist || '', lead: songLead(),
      picked: !!settings.picks[songKey(s)],
      status: lyrics?.status || 'loading',
      provider: lyrics?.status === 'found' ? lyrics.provider || 'LRCLIB' : null,
      listening: recognising === s.key,
      recognisedAs: recognised.get(s.key) || null,
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
  const cardWidth = Number.isFinite(at?.width) ? at.width : 340 * settings.cardScale;
  // Next to the card, or on its left when there is no room on the right.
  let x = area.x + card.x + cardWidth, side = 'right';
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
  const choices = {
    look: LOOKS, motion: MOTIONS, colourMode: COLOUR_MODES, font: FONTS, entrance: ENTRANCES,
    cardLyricsSide: SIDES, bigAlign: ALIGNS, peekKey: PEEK_KEYS,
  };
  const isHex = (v) => typeof v === 'string' && /^#[0-9a-f]{6}$/i.test(v);
  switch (name) {
    case 'toggle':
      if (value === 'enabled') toggleEnabled();
      else if (TOGGLES.includes(value)) update({ [value]: !settings[value] });
      break;
    case 'set': setValue(value?.key, value?.value); break;
    case 'reset-layout': {
      const keys = ['card', 'cardScale', 'cardOpacity', 'cardLyricsSide', 'cardNext', 'bigPos', 'bigWidth', 'bigSize', 'bigAlign'];
      update(Object.fromEntries(keys.map((k) => [k, DEFAULTS[k]])));
      send('toast', 'Layout back to how it started');
      break;
    }
    case 'reset-all': {
      // Every setting back to its default. Per-song timing fixes, lyrics picks and recent colours stay.
      const { offsets, picks, history, ...rest } = DEFAULTS;
      update(structuredClone(rest));
      send('toast', 'All settings back to the defaults');
      break;
    }
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
    case 'recognise': if (lastState?.active) recogniseSong(lastState, true); else send('toast', 'Nothing is playing'); break;
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
  allowListening();

  ipcMain.on('card-moved', (_e, pos) => {
    if (Number.isFinite(pos?.x) && Number.isFinite(pos?.y)) update({ card: { x: Math.round(pos.x), y: Math.round(pos.y) } });
  });
  // Sizes and the big lyrics' place, changed by dragging on the overlay while arranging.
  ipcMain.on('set', (e, msg) => { if (win && e.sender === win.webContents) setValue(msg?.key, msg?.value); });
  ipcMain.on('edit-done', () => setEditing(false));
  ipcMain.on('open-settings', (_e, at) => togglePanel(at));
  ipcMain.on('control', (e, msg) => { if (win && e.sender === win.webContents) control(msg); });
  ipcMain.on('ui-hover', (_e, on) => { pageHold = !!on; pageBusy = on === 'busy'; updateCatching(); });
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
