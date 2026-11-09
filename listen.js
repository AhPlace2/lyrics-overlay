'use strict';
/* Records what the computer is playing (Windows "loopback" audio, which main grants to this page only) and
   returns it as 16 kHz mono 16-bit samples, the format song recognition wants. Main calls record(). */
async function record(ms) {
  const stream = await navigator.mediaDevices.getDisplayMedia({ audio: true, video: true });
  for (const t of stream.getVideoTracks()) t.stop(); // the screen isn't needed, only its sound
  if (!stream.getAudioTracks().length) throw new Error('No system audio to record');
  const ctx = new AudioContext({ sampleRate: 16000 });
  try {
    await ctx.audioWorklet.addModule('listen-worklet.js');
    const tap = new AudioWorkletNode(ctx, 'tap');
    const chunks = [];
    tap.port.onmessage = (e) => chunks.push(e.data);
    ctx.createMediaStreamSource(stream).connect(tap);
    await new Promise((r) => setTimeout(r, ms));
    const total = chunks.reduce((n, c) => n + c.length, 0);
    const out = new Int16Array(total);
    let at = 0, peak = 0;
    for (const c of chunks) {
      for (let i = 0; i < c.length; i++) {
        const v = Math.max(-1, Math.min(1, c[i]));
        out[at++] = Math.round(v * 32767);
        peak = Math.max(peak, Math.abs(v));
      }
    }
    return { samples: Array.from(out), peak };
  } finally {
    for (const t of stream.getTracks()) t.stop();
    ctx.close();
  }
}
