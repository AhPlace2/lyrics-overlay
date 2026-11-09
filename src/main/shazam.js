'use strict';
/* Recognises a song from a few seconds of audio by asking Shazam's servers, the way open-source apps like
   SongRec do. Shazam has no public API: this uses the same private endpoint as its phone app, so it can break
   or be blocked at any time, and Shazam's terms don't allow it. Fine for your own use; don't ship it to others.

   How it works: the audio (16 kHz, mono) is cut into overlapping slices, each turned into a spectrum (which
   frequencies are loud). The strongest, most isolated peaks in those spectra, sorted into four frequency
   bands, make up the "signature". Shazam matches it against the signatures of every song it knows. */
const crypto = require('node:crypto');
const zlib = require('node:zlib');

const RATE = 16000;
const N = 2048; // samples per spectrum
const BINS = N / 2 + 1;

/* ---------- Spectra ---------- */
// Hann window, as numpy's hanning(2050)[1:-1].
const HANN = Float64Array.from({ length: N }, (_, n) => 0.5 - 0.5 * Math.cos((2 * Math.PI * (n + 1)) / (N + 1)));
const REV = new Uint16Array(N);
for (let i = 0; i < N; i++) {
  let r = 0;
  for (let b = 0; b < 11; b++) if (i & (1 << b)) r |= 1 << (10 - b);
  REV[i] = r;
}
const COS = Float64Array.from({ length: N / 2 }, (_, k) => Math.cos((2 * Math.PI * k) / N));
const SIN = Float64Array.from({ length: N / 2 }, (_, k) => Math.sin((2 * Math.PI * k) / N));
const re = new Float64Array(N), im = new Float64Array(N);

/** Power spectrum of 2048 windowed samples, scaled like Shazam's (bins 0..1024). */
function spectrum(input, out) {
  for (let i = 0; i < N; i++) { re[REV[i]] = input[i]; im[REV[i]] = 0; }
  for (let size = 2; size <= N; size <<= 1) {
    const half = size >> 1, step = N / size;
    for (let start = 0; start < N; start += size) {
      for (let k = 0; k < half; k++) {
        const wr = COS[k * step], wi = -SIN[k * step];
        const a = start + k, b = a + half;
        const tr = re[b] * wr - im[b] * wi, ti = re[b] * wi + im[b] * wr;
        re[b] = re[a] - tr; im[b] = im[a] - ti;
        re[a] += tr; im[a] += ti;
      }
    }
  }
  for (let k = 0; k < BINS; k++) out[k] = Math.max((re[k] * re[k] + im[k] * im[k]) / (1 << 17), 1e-10);
}

/* ---------- Peaks ---------- */
const NEAR = [-10, -7, -4, -3, 1, 2, 5, 8];
const OTHER_SLICES = [-53, -45, 165, 172, 179, 186, 193, 200, 214, 221, 228, 235, 242, 249];
const loudness = (v) => Math.log(Math.max(1 / 64, v)) * 1477.3 + 6144;

/** The peaks of 16 kHz mono 16-bit samples, by band: [{ fft, mag, bin }]. */
function findPeaks(samples) {
  const ring = new Int16Array(N);
  let ringAt = 0;
  const ffts = Array.from({ length: 256 }, () => new Float64Array(BINS));
  const spread = Array.from({ length: 256 }, () => new Float64Array(BINS));
  let fftAt = 0, spreadAt = 0, done = 0;
  const windowed = new Float64Array(N);
  const bands = [[], [], [], []];

  for (let chunk = 0; chunk + 128 <= samples.length; chunk += 128) {
    for (let i = 0; i < 128; i++) { ring[ringAt] = samples[chunk + i]; ringAt = (ringAt + 1) & (N - 1); }
    for (let i = 0; i < N; i++) windowed[i] = ring[(ringAt + i) & (N - 1)] * HANN[i];
    spectrum(windowed, ffts[fftAt]);
    const last = ffts[fftAt];
    fftAt = (fftAt + 1) & 255;

    // Spread each peak over its neighbouring bins and over the slices just before it.
    const s = spread[spreadAt];
    s.set(last);
    for (let k = 0; k <= BINS - 3; k++) s[k] = Math.max(s[k], s[k + 1], s[k + 2]);
    for (let k = 0; k < BINS; k++) {
      let m = s[k];
      for (const back of [1, 3, 6]) {
        const before = spread[(spreadAt - back) & 255];
        if (before[k] > m) m = before[k];
        before[k] = m;
      }
    }
    spreadAt = (spreadAt + 1) & 255;
    done++;
    if (done < 46) continue;

    // A peak in the slice 46 back counts when it beats everything around it, nearby and in other slices.
    const f46 = ffts[(fftAt - 46) & 255], s49 = spread[(spreadAt - 49) & 255];
    for (let bin = 10; bin < 1015; bin++) {
      const v = f46[bin];
      if (v < 1 / 64 || v < s49[bin - 1]) continue;
      let near = 0;
      for (const o of NEAR) near = Math.max(near, s49[bin + o]);
      if (v <= near) continue;
      let other = near;
      for (const o of OTHER_SLICES) other = Math.max(other, spread[(spreadAt + o) & 255][bin - 1]);
      if (v <= other) continue;

      const mag = loudness(v), before = loudness(f46[bin - 1]), after = loudness(f46[bin + 1]);
      const curve = mag * 2 - before - after;
      if (curve <= 0) continue;
      const corrected = bin * 64 + ((after - before) * 32) / curve;
      const hz = corrected * (RATE / 2 / 1024 / 64);
      const band = hz > 250 && hz < 520 ? 0 : hz > 520 && hz < 1450 ? 1 : hz > 1450 && hz < 3500 ? 2 : hz > 3500 && hz <= 5500 ? 3 : -1;
      if (band >= 0) bands[band].push({ fft: done - 46, mag: Math.trunc(mag), bin: Math.trunc(corrected) });
    }
  }
  return bands;
}

/* ---------- Signature ---------- */
function encode(bands, sampleCount) {
  const body = [];
  const u32 = (v) => { const b = Buffer.alloc(4); b.writeUInt32LE(v >>> 0); return b; };
  bands.forEach((peaks, band) => {
    if (!peaks.length) return;
    const bytes = [];
    let pass = 0;
    for (const p of peaks) {
      if (p.fft - pass >= 255) { bytes.push(0xff, ...u32(p.fft)); pass = p.fft; }
      bytes.push(p.fft - pass, p.mag & 255, p.mag >> 8, p.bin & 255, p.bin >> 8);
      pass = p.fft;
    }
    body.push(u32(0x60030040 + band), u32(bytes.length), Buffer.from(bytes), Buffer.alloc((4 - (bytes.length % 4)) % 4));
  });
  const content = Buffer.concat(body);
  const size = 8 + content.length; // what follows the 48-byte header
  const head = Buffer.concat([
    u32(0xcafe2580), u32(0), u32(size), u32(0x94119c00), u32(0), u32(0), u32(0),
    u32(3 << 27), // 16 kHz
    u32(0), u32(0), u32(Math.trunc(sampleCount + RATE * 0.24)), u32((15 << 19) + 0x40000),
    u32(0x40000000), u32(size),
  ]);
  const all = Buffer.concat([head, content]);
  all.writeUInt32LE(zlib.crc32(all.subarray(8)) >>> 0, 4);
  return all;
}

/* ---------- Asking Shazam ---------- */
const USER_AGENT = 'Dalvik/2.1.0 (Linux; U; Android 10; SM-G973F Build/QP1A.190711.020)';

/** Shazam's whole answer for 16 kHz mono 16-bit samples (3 to 12 seconds). */
async function recogniseRaw(samples) {
  const usable = samples.subarray(0, samples.length - (samples.length % 128));
  const signature = encode(findPeaks(usable), usable.length);
  const now = Date.now();
  const id = () => crypto.randomUUID().toUpperCase();
  const url = `https://amp.shazam.com/discovery/v5/en/US/android/-/tag/${id()}/${id()}` +
    '?sync=true&webv3=true&sampling=true&connected=&shazamapiversion=v3&sharehub=true&video=v3';
  const res = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'Content-Language': 'en_US', 'User-Agent': USER_AGENT },
    body: JSON.stringify({
      geolocation: { altitude: 300, latitude: 45, longitude: 2 },
      signature: { samplems: Math.round((usable.length / RATE) * 1000), timestamp: now, uri: `data:audio/vnd.shazam.sig;base64,${signature.toString('base64')}` },
      timestamp: now,
      timezone: 'Europe/Paris',
    }),
    signal: AbortSignal.timeout(15000),
  });
  if (!res.ok) throw new Error(`Shazam answered ${res.status}`);
  return res.json();
}

/**
 * Recognises 16 kHz mono 16-bit samples (3 to 12 seconds). Resolves to { title, artist, offset } (offset: where
 * in the song the clip started, in seconds) or null when Shazam doesn't know it.
 */
async function recognise(samples) {
  const data = await recogniseRaw(samples);
  const track = data?.track;
  if (!track?.title) return null;
  return { title: track.title, artist: track.subtitle || '', offset: Number(data.matches?.[0]?.offset) || null };
}

module.exports = { recognise, recogniseRaw, findPeaks, encode };
