// Recognises a song from a few seconds of audio by asking Shazam's servers, the way open-source apps like SongRec
// do. Port of src/main/shazam.js: Shazam has no public API, this uses the same private endpoint as its phone app,
// so it can break or be blocked at any time.
//
// The audio (16 kHz, mono) is cut into overlapping slices, each turned into a spectrum. The strongest, most
// isolated peaks in those spectra, sorted into four frequency bands, make up the "signature".

use base64::Engine;
use serde_json::{json, Value};
use std::f64::consts::PI;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

const RATE: f64 = 16000.0;
const N: usize = 2048; // samples per spectrum
const BINS: usize = N / 2 + 1;

struct Fft {
    hann: Vec<f64>,
    rev: Vec<usize>,
    cos: Vec<f64>,
    sin: Vec<f64>,
}

static FFT: LazyLock<Fft> = LazyLock::new(|| {
    // Hann window, as numpy's hanning(2050)[1:-1].
    let hann = (0..N).map(|n| 0.5 - 0.5 * ((2.0 * PI * (n as f64 + 1.0)) / (N as f64 + 1.0)).cos()).collect();
    let rev = (0..N)
        .map(|i| {
            let mut r = 0;
            for b in 0..11 {
                if i & (1 << b) != 0 {
                    r |= 1 << (10 - b);
                }
            }
            r
        })
        .collect();
    let cos = (0..N / 2).map(|k| ((2.0 * PI * k as f64) / N as f64).cos()).collect();
    let sin = (0..N / 2).map(|k| ((2.0 * PI * k as f64) / N as f64).sin()).collect();
    Fft { hann, rev, cos, sin }
});

/// Power spectrum of 2048 windowed samples, scaled like Shazam's (bins 0..1024).
fn spectrum(input: &[f64], out: &mut [f64], re: &mut [f64], im: &mut [f64]) {
    let f = &*FFT;
    for i in 0..N {
        re[f.rev[i]] = input[i];
        im[f.rev[i]] = 0.0;
    }
    let mut size = 2;
    while size <= N {
        let half = size >> 1;
        let step = N / size;
        let mut start = 0;
        while start < N {
            for k in 0..half {
                let (wr, wi) = (f.cos[k * step], -f.sin[k * step]);
                let (a, b) = (start + k, start + k + half);
                let tr = re[b] * wr - im[b] * wi;
                let ti = re[b] * wi + im[b] * wr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
            start += size;
        }
        size <<= 1;
    }
    for k in 0..BINS {
        out[k] = ((re[k] * re[k] + im[k] * im[k]) / (1 << 17) as f64).max(1e-10);
    }
}

const NEAR: [i32; 8] = [-10, -7, -4, -3, 1, 2, 5, 8];
const OTHER_SLICES: [i32; 14] = [-53, -45, 165, 172, 179, 186, 193, 200, 214, 221, 228, 235, 242, 249];
fn loudness(v: f64) -> f64 {
    v.max(1.0 / 64.0).ln() * 1477.3 + 6144.0
}

#[derive(Clone, Copy)]
struct Peak {
    fft: u32,
    mag: u32,
    bin: u32,
}

fn ring(i: i64) -> usize {
    (i & 255) as usize
}

/// The peaks of 16 kHz mono 16-bit samples, by band.
fn find_peaks(samples: &[i16]) -> [Vec<Peak>; 4] {
    let hann = &FFT.hann;
    let mut ring_buf = [0i16; N];
    let mut ring_at = 0usize;
    let mut ffts = vec![vec![0f64; BINS]; 256];
    let mut spread = vec![vec![0f64; BINS]; 256];
    let (mut fft_at, mut spread_at, mut done) = (0i64, 0i64, 0i64);
    let mut windowed = vec![0f64; N];
    let (mut re, mut im) = (vec![0f64; N], vec![0f64; N]);
    let mut bands: [Vec<Peak>; 4] = Default::default();

    let mut chunk = 0;
    while chunk + 128 <= samples.len() {
        for i in 0..128 {
            ring_buf[ring_at] = samples[chunk + i];
            ring_at = (ring_at + 1) & (N - 1);
        }
        for i in 0..N {
            windowed[i] = ring_buf[(ring_at + i) & (N - 1)] as f64 * hann[i];
        }
        spectrum(&windowed, &mut ffts[ring(fft_at)], &mut re, &mut im);
        let last = ffts[ring(fft_at)].clone();
        fft_at = (fft_at + 1) & 255;

        // Spread each peak over its neighbouring bins and over the slices just before it.
        {
            let s = &mut spread[ring(spread_at)];
            s.copy_from_slice(&last);
            for k in 0..=BINS - 3 {
                s[k] = s[k].max(s[k + 1]).max(s[k + 2]);
            }
        }
        for k in 0..BINS {
            let mut m = spread[ring(spread_at)][k];
            for back in [1, 3, 6] {
                let before = &mut spread[ring(spread_at - back)];
                if before[k] > m {
                    m = before[k];
                }
                before[k] = m;
            }
        }
        spread_at = (spread_at + 1) & 255;
        done += 1;
        chunk += 128;
        if done < 46 {
            continue;
        }

        // A peak in the slice 46 back counts when it beats everything around it, nearby and in other slices.
        let f46 = &ffts[ring(fft_at - 46)];
        let s49 = &spread[ring(spread_at - 49)];
        for bin in 10..1015usize {
            let v = f46[bin];
            if v < 1.0 / 64.0 || v < s49[bin - 1] {
                continue;
            }
            let mut near = 0f64;
            for o in NEAR {
                near = near.max(s49[(bin as i32 + o) as usize]);
            }
            if v <= near {
                continue;
            }
            let mut other = near;
            for o in OTHER_SLICES {
                other = other.max(spread[ring(spread_at + o as i64)][bin - 1]);
            }
            if v <= other {
                continue;
            }
            let (mag, before, after) = (loudness(v), loudness(f46[bin - 1]), loudness(f46[bin + 1]));
            let curve = mag * 2.0 - before - after;
            if curve <= 0.0 {
                continue;
            }
            let corrected = bin as f64 * 64.0 + ((after - before) * 32.0) / curve;
            let hz = corrected * (RATE / 2.0 / 1024.0 / 64.0);
            let band = if hz > 250.0 && hz < 520.0 {
                0
            } else if hz > 520.0 && hz < 1450.0 {
                1
            } else if hz > 1450.0 && hz < 3500.0 {
                2
            } else if hz > 3500.0 && hz <= 5500.0 {
                3
            } else {
                continue;
            };
            bands[band].push(Peak { fft: (done - 46) as u32, mag: mag.trunc() as u32, bin: corrected.trunc() as u32 });
        }
    }
    bands
}

fn encode(bands: &[Vec<Peak>; 4], sample_count: usize) -> Vec<u8> {
    let mut body = Vec::new();
    for (band, peaks) in bands.iter().enumerate() {
        if peaks.is_empty() {
            continue;
        }
        let mut bytes = Vec::new();
        let mut pass = 0u32;
        for p in peaks {
            if p.fft - pass >= 255 {
                bytes.push(0xff);
                bytes.extend_from_slice(&p.fft.to_le_bytes());
                pass = p.fft;
            }
            bytes.extend_from_slice(&[(p.fft - pass) as u8, p.mag as u8, (p.mag >> 8) as u8, p.bin as u8, (p.bin >> 8) as u8]);
            pass = p.fft;
        }
        body.extend_from_slice(&(0x6003_0040u32 + band as u32).to_le_bytes());
        body.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        let pad = (4 - bytes.len() % 4) % 4;
        body.extend_from_slice(&bytes);
        body.extend(std::iter::repeat_n(0u8, pad));
    }
    let size = 8 + body.len() as u32; // what follows the 48-byte header
    let head: [u32; 14] = [
        0xcafe_2580,
        0,
        size,
        0x9411_9c00,
        0,
        0,
        0,
        3 << 27, // 16 kHz
        0,
        0,
        (sample_count as f64 + RATE * 0.24).trunc() as u32,
        (15 << 19) + 0x40000,
        0x4000_0000,
        size,
    ];
    let mut all: Vec<u8> = head.iter().flat_map(|v| v.to_le_bytes()).collect();
    all.extend_from_slice(&body);
    let crc = crc32fast::hash(&all[8..]);
    all[4..8].copy_from_slice(&crc.to_le_bytes());
    all
}

const USER_AGENT: &str = "Dalvik/2.1.0 (Linux; U; Android 10; SM-G973F Build/QP1A.190711.020)";
static SHAZAM_HTTP: LazyLock<ureq::Agent> = LazyLock::new(|| crate::lyrics::agent(15));

pub struct Match {
    pub title: String,
    pub artist: String,
}

/// Recognises 16 kHz mono 16-bit samples (3 to 12 seconds). None when Shazam doesn't know it.
pub fn recognise(samples: &[i16]) -> Result<Option<Match>, String> {
    let usable = &samples[..samples.len() - samples.len() % 128];
    let signature = encode(&find_peaks(usable), usable.len());
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let id = || uuid::Uuid::new_v4().to_string().to_uppercase();
    let url = format!(
        "https://amp.shazam.com/discovery/v5/en/US/android/-/tag/{}/{}?sync=true&webv3=true&sampling=true&connected=&shazamapiversion=v3&sharehub=true&video=v3",
        id(),
        id()
    );
    let body = json!({
        "geolocation": { "altitude": 300, "latitude": 45, "longitude": 2 },
        "signature": {
            "samplems": ((usable.len() as f64 / RATE) * 1000.0).round() as u64,
            "timestamp": now,
            "uri": format!("data:audio/vnd.shazam.sig;base64,{}", base64::engine::general_purpose::STANDARD.encode(&signature)),
        },
        "timestamp": now,
        "timezone": "Europe/Paris",
    });
    let mut res = SHAZAM_HTTP
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Content-Language", "en_US")
        .header("User-Agent", USER_AGENT)
        .send_json(&body)
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("Shazam answered {}", res.status().as_u16()));
    }
    let data: Value = res.body_mut().read_json().map_err(|e| e.to_string())?;
    let Some(track) = data.get("track") else { return Ok(None) };
    let title = crate::lyrics::str_of(track, "title");
    if title.is_empty() {
        return Ok(None);
    }
    Ok(Some(Match { title: title.to_string(), artist: crate::lyrics::str_of(track, "subtitle").to_string() }))
}
