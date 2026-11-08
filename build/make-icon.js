'use strict';
// Draws build/icon.png (256x256): the tray icon's amber rounded square with three white bars, smoothed.
// Run with: node build/make-icon.js
const fs = require('node:fs');
const path = require('node:path');
const zlib = require('node:zlib');

const SIZE = 256, GRID = 32, SS = 4; // drawn on the tray icon's 32-unit grid, 4x4 samples per pixel
const bars = [[9, 13, 8, 24], [15, 19, 8, 21], [21, 25, 8, 16]]; // [y0, y1, x0, x1], as in main.js

function sample(x, y) { // x, y in grid units; returns [r, g, b, a]
  const dx = Math.max(7 - x, x - (GRID - 7), 0), dy = Math.max(7 - y, y - (GRID - 7), 0);
  if (dx * dx + dy * dy > 49) return [0, 0, 0, 0];
  const white = bars.some(([y0, y1, x0, x1]) => y >= y0 && y < y1 && x >= x0 && x < x1);
  return white ? [255, 255, 255, 255] : [255, 0xb6, 0x48, 255];
}

const raw = Buffer.alloc(SIZE * (SIZE * 4 + 1));
for (let py = 0; py < SIZE; py++) {
  raw[py * (SIZE * 4 + 1)] = 0; // PNG filter: none
  for (let px = 0; px < SIZE; px++) {
    const sum = [0, 0, 0, 0];
    for (let sy = 0; sy < SS; sy++) {
      for (let sx = 0; sx < SS; sx++) {
        const c = sample(((px + (sx + .5) / SS) / SIZE) * GRID, ((py + (sy + .5) / SS) / SIZE) * GRID);
        for (let k = 0; k < 3; k++) sum[k] += c[k] * c[3];
        sum[3] += c[3];
      }
    }
    const o = py * (SIZE * 4 + 1) + 1 + px * 4, a = sum[3] / (SS * SS);
    for (let k = 0; k < 3; k++) raw[o + k] = sum[3] ? Math.round(sum[k] / sum[3]) : 0;
    raw[o + 3] = Math.round(a);
  }
}

const crcTable = Array.from({ length: 256 }, (_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; return c >>> 0; });
const crc = (buf) => { let c = 0xffffffff; for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0; };
const chunk = (type, data) => {
  const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type), data]);
  const sum = Buffer.alloc(4); sum.writeUInt32BE(crc(body));
  return Buffer.concat([len, body, sum]);
};
const header = Buffer.alloc(13);
header.writeUInt32BE(SIZE, 0); header.writeUInt32BE(SIZE, 4); header[8] = 8; header[9] = 6; // 8-bit RGBA
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk('IHDR', header), chunk('IDAT', zlib.deflateSync(raw, { level: 9 })), chunk('IEND', Buffer.alloc(0)),
]);
fs.writeFileSync(path.join(__dirname, 'icon.png'), png);
console.log('wrote build/icon.png');
