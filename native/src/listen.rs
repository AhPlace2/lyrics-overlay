// Records what the computer is playing (Windows "loopback" audio) and returns it as 16 kHz mono 16-bit samples,
// the format song recognition wants. Replaces the hidden listen.html page of the Electron build.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Recording {
    pub samples: Vec<i16>,
    pub peak: f32,
}

pub fn record(ms: u64) -> Result<Recording, String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("No audio output to record")?;
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    let rate = config.sample_rate().0 as f64;
    let channels = config.channels() as usize;
    let mono: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::with_capacity((rate as usize) * (ms as usize / 1000 + 1))));

    // Building an input stream on an output device records what it plays (WASAPI loopback).
    let stream = {
        let sink = mono.clone();
        let err = |e| eprintln!("[listen] {e}");
        let push = move |data: &mut dyn Iterator<Item = f32>| {
            let mut out = sink.lock().unwrap();
            let mut frame = 0.0;
            let mut n = 0;
            for v in data {
                frame += v;
                n += 1;
                if n == channels {
                    out.push(frame / channels as f32);
                    frame = 0.0;
                    n = 0;
                }
            }
        };
        let cfg: cpal::StreamConfig = config.clone().into();
        match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(&cfg, move |d: &[f32], _: &_| push(&mut d.iter().copied()), err, None),
            cpal::SampleFormat::I16 => {
                device.build_input_stream(&cfg, move |d: &[i16], _: &_| push(&mut d.iter().map(|&v| v as f32 / 32768.0)), err, None)
            }
            cpal::SampleFormat::I32 => {
                device.build_input_stream(&cfg, move |d: &[i32], _: &_| push(&mut d.iter().map(|&v| v as f32 / 2147483648.0)), err, None)
            }
            other => return Err(format!("Unsupported audio format {other:?}")),
        }
        .map_err(|e| e.to_string())?
    };
    stream.play().map_err(|e| e.to_string())?;
    std::thread::sleep(Duration::from_millis(ms));
    drop(stream);

    let src = std::mem::take(&mut *mono.lock().unwrap());
    // Down to 16 kHz: each output sample is the mean of the input samples it covers (a simple low-pass).
    let ratio = rate / 16000.0;
    let count = (src.len() as f64 / ratio) as usize;
    let mut samples = Vec::with_capacity(count);
    let mut peak = 0f32;
    for i in 0..count {
        let a = (i as f64 * ratio) as usize;
        let b = (((i + 1) as f64 * ratio) as usize).clamp(a + 1, src.len());
        let v = (src[a..b].iter().sum::<f32>() / (b - a) as f32).clamp(-1.0, 1.0);
        peak = peak.max(v.abs());
        samples.push((v * 32767.0).round() as i16);
    }
    Ok(Recording { samples, peak })
}
