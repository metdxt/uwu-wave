//! Real-time playback through the default audio device (cpal + a lock-free rtrb ring buffer).
//!
//! The main thread renders ahead with the song [`Engine`](crate::engine::Engine) at the device's
//! sample rate; the audio callback only pops frames. A status line (position, section, peak
//! meter) is drawn on stderr. `--loop` plays the range once live while capturing it, then loops
//! the capture.

use crate::Error;
use crate::dsp::Frame;
use crate::engine::{MAX_BLOCK, RenderOptions, Report};
use crate::song::Song;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Why playback failed.
#[derive(Debug)]
pub enum PlayError {
    /// No usable output device/stream (caller may fall back to offline rendering; the song is
    /// untouched).
    NoDevice(String),
    /// The song failed to build/render.
    Render(Error),
}

impl std::fmt::Display for PlayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlayError::NoDevice(e) => write!(f, "no audio output: {e}"),
            PlayError::Render(e) => write!(f, "{e}"),
        }
    }
}

struct Shared {
    played: AtomicU64,
    peak: AtomicU32,
    underruns: AtomicU64,
    started: AtomicBool,
}

fn write_out<T: cpal::SizedSample + cpal::FromSample<f32>>(
    data: &mut [T],
    ch: usize,
    cons: &mut rtrb::Consumer<Frame>,
    sh: &Shared,
) {
    let mut peak = 0f32;
    let mut n = 0u64;
    let mut missing = false;
    for fr in data.chunks_mut(ch.max(1)) {
        let f = match cons.pop() {
            Ok(f) => {
                n += 1;
                f
            }
            Err(_) => {
                missing = true;
                Frame::ZERO
            }
        };
        let (l, r) = (f.l.clamp(-1.0, 1.0), f.r.clamp(-1.0, 1.0));
        peak = peak.max(f.peak());
        match fr.len() {
            1 => fr[0] = T::from_sample(0.5 * (l + r)),
            _ => {
                fr[0] = T::from_sample(l);
                fr[1] = T::from_sample(r);
                for x in &mut fr[2..] {
                    *x = T::from_sample(0.0f32);
                }
            }
        }
    }
    sh.played.fetch_add(n, Ordering::Relaxed);
    if missing && sh.started.load(Ordering::Relaxed) {
        sh.underruns.fetch_add(1, Ordering::Relaxed);
    }
    sh.peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
}

fn build_stream(
    dev: &cpal::Device,
    cfg: &cpal::SupportedStreamConfig,
    mut cons: rtrb::Consumer<Frame>,
    sh: Arc<Shared>,
) -> Result<cpal::Stream, String> {
    let config: cpal::StreamConfig = cfg.config();
    let ch = config.channels as usize;
    let err = |e| eprintln!("\naudio stream error: {e}");
    macro_rules! build {
        ($t:ty) => {
            dev.build_output_stream(
                &config,
                move |d: &mut [$t], _: &cpal::OutputCallbackInfo| write_out(d, ch, &mut cons, &sh),
                err,
                None,
            )
        };
    }
    let s = match cfg.sample_format() {
        cpal::SampleFormat::F32 => build!(f32),
        cpal::SampleFormat::F64 => build!(f64),
        cpal::SampleFormat::I16 => build!(i16),
        cpal::SampleFormat::U16 => build!(u16),
        cpal::SampleFormat::I32 => build!(i32),
        cpal::SampleFormat::U32 => build!(u32),
        cpal::SampleFormat::I8 => build!(i8),
        cpal::SampleFormat::U8 => build!(u8),
        f => return Err(format!("unsupported sample format {f}")),
    };
    s.map_err(|e| e.to_string())
}

fn meter(peak: f32) -> String {
    let db = crate::units::gain_to_db(peak).max(-60.0);
    let n = (((db + 60.0) / 60.0) * 16.0).round() as usize;
    let bar: String = (0..16)
        .map(|i| {
            if i < n {
                if i >= 15 { '█' } else { '▮' }
            } else {
                '·'
            }
        })
        .collect();
    format!(
        "{bar} {:>5.1} dB{}",
        db,
        if peak > 1.0 { " CLIP" } else { "" }
    )
}

/// Play `song` on the default output device. Blocks until the end (forever with `looping`).
pub fn play(song: &Song, mut opts: RenderOptions, looping: bool) -> Result<Report, PlayError> {
    let host = cpal::default_host();
    let dev = host
        .default_output_device()
        .ok_or_else(|| PlayError::NoDevice("no default output device".into()))?;
    let cfg = dev
        .default_output_config()
        .map_err(|e| PlayError::NoDevice(e.to_string()))?;
    let sr = cfg.sample_rate().0;
    opts.sample_rate = Some(sr);
    opts.stems = false;
    if looping {
        opts.no_tail = true;
    }
    let cap = (sr as usize / 4).max(4096); // 250 ms render-ahead
    let (mut prod, cons) = rtrb::RingBuffer::<Frame>::new(cap);
    let sh = Arc::new(Shared {
        played: AtomicU64::new(0),
        peak: AtomicU32::new(0),
        underruns: AtomicU64::new(0),
        started: AtomicBool::new(false),
    });
    let stream = build_stream(&dev, &cfg, cons, sh.clone()).map_err(PlayError::NoDevice)?;

    let sections = song.sections();
    let (num, den) = song.meter_sig();
    let bpb = num as f64 * 4.0 / den.max(1) as f64;
    let mut engine = song.engine(&opts).map_err(PlayError::Render)?;
    let dev_name = dev.name().unwrap_or_else(|_| "default".into());
    eprintln!(
        "▶ {} on {dev_name} @ {sr} Hz{}  (Ctrl-C to stop)",
        song.name(),
        if looping { ", looping" } else { "" }
    );

    // prefill, then start the device
    let mut buf = [Frame::ZERO; MAX_BLOCK];
    let mut rendered: u64 = 0;
    let mut beats: VecDeque<(u64, f64)> = VecDeque::new(); // (frame, beat) at each block start
    let mut capture: Vec<Frame> = vec![];
    let mut capture_beats: Vec<f64> = vec![];
    let mut done = false;
    let mut fill = |prod: &mut rtrb::Producer<Frame>,
                    engine: &mut crate::engine::Engine,
                    rendered: &mut u64,
                    beats: &mut VecDeque<(u64, f64)>,
                    capture: &mut Vec<Frame>,
                    capture_beats: &mut Vec<f64>,
                    done: &mut bool| {
        while !*done && prod.slots() >= MAX_BLOCK {
            let beat = engine.position().beat;
            let k = engine.process(&mut buf);
            for f in &buf[..k] {
                let _ = prod.push(*f);
            }
            beats.push_back((*rendered, beat));
            if looping {
                capture.extend_from_slice(&buf[..k]);
                capture_beats.push(beat);
            }
            *rendered += k as u64;
            if k < MAX_BLOCK {
                *done = true;
            }
        }
    };
    fill(
        &mut prod,
        &mut engine,
        &mut rendered,
        &mut beats,
        &mut capture,
        &mut capture_beats,
        &mut done,
    );
    stream
        .play()
        .map_err(|e| PlayError::NoDevice(e.to_string()))?;
    sh.started.store(true, Ordering::Relaxed);

    let mut last_status = Instant::now() - Duration::from_secs(1);
    let mut held_peak = 0f32;
    let mut loop_pos = 0usize;
    let mut loops = 0u64;
    let mut shown_beat = engine.start_beat();
    loop {
        if !done {
            fill(
                &mut prod,
                &mut engine,
                &mut rendered,
                &mut beats,
                &mut capture,
                &mut capture_beats,
                &mut done,
            );
        } else if looping && !capture.is_empty() {
            while prod.slots() > 0 {
                if loop_pos == 0 {
                    loops += 1;
                }
                if loop_pos.is_multiple_of(MAX_BLOCK) {
                    beats.push_back((rendered, capture_beats[loop_pos / MAX_BLOCK]));
                }
                let _ = prod.push(capture[loop_pos]);
                rendered += 1;
                loop_pos = (loop_pos + 1) % capture.len();
            }
        }
        let played = sh.played.load(Ordering::Relaxed);
        while beats.len() > 1 && beats[1].0 <= played {
            beats.pop_front();
        }
        if let Some(&(f0, b0)) = beats.front()
            && f0 <= played
        {
            shown_beat = b0;
        }
        if last_status.elapsed() >= Duration::from_millis(50) {
            last_status = Instant::now();
            let p = f32::from_bits(sh.peak.swap(0, Ordering::Relaxed));
            held_peak = p.max(held_peak * 0.7);
            let bar = (shown_beat / bpb).floor();
            let beat_in = shown_beat - bar * bpb;
            let sec = sections
                .iter()
                .rev()
                .find(|s| s.start.0 <= shown_beat + 1e-9 && shown_beat < s.end().0)
                .map(|s| s.name.as_str())
                .unwrap_or("");
            let under = sh.underruns.load(Ordering::Relaxed);
            let mut err = std::io::stderr().lock();
            let _ = write!(
                err,
                "\r  {:>4}:{:<5.2} {:<14} {}{}{}\x1b[K",
                bar as i64,
                beat_in,
                if sec.is_empty() {
                    String::new()
                } else {
                    format!("[{sec}]")
                },
                meter(held_peak),
                if loops > 1 {
                    format!("  loop {loops}")
                } else {
                    String::new()
                },
                if under > 0 {
                    format!("  ({under} underruns)")
                } else {
                    String::new()
                }
            );
            let _ = err.flush();
        }
        if done && !looping && sh.played.load(Ordering::Relaxed) >= rendered {
            break;
        }
        std::thread::sleep(Duration::from_millis(3));
    }
    // let the device drain its own buffer
    std::thread::sleep(Duration::from_millis(150));
    drop(stream);
    eprintln!();
    Ok(engine.report())
}
