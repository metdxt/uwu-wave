//! The render engine: turns a [`Song`] into audio, block by block.
//!
//! Channels (tracks, buses, master) are processed in dependency order (routing, sends and
//! sidechains form a DAG). Note events are sample-accurate: blocks are split at event boundaries.

use crate::ctx::Ctx;
use crate::dsp::{pan_gains, Frame, Ramp};
use crate::fx::meter::Meter;
use crate::fx::Effect;
use crate::instrument::{Instrument, NoteOn};
use crate::param::{Clock, Param};
use crate::song::{ChanKind, Song};
use crate::time::Time;
use crate::Error;
use std::fmt;
use std::path::Path;
use std::time::Instant;

/// Maximum frames per processing block (parameters are evaluated once per block).
pub const MAX_BLOCK: usize = 64;

/// Render settings (all optional).
#[derive(Clone, Debug, Default)]
pub struct RenderOptions {
    /// Override the song's sample rate.
    pub sample_rate: Option<u32>,
    /// Start rendering here (skips earlier notes).
    pub from: Option<Time>,
    /// Stop here (instead of the song end).
    pub to: Option<Time>,
    /// Only these tracks are audible (names).
    pub solo: Vec<String>,
    /// These tracks are muted (names).
    pub mute: Vec<String>,
    /// Capture per-track/bus stems.
    pub stems: bool,
    /// Don't render the reverb/release tail after the end.
    pub no_tail: bool,
}

#[derive(Clone, Copy, Debug)]
enum EvKind {
    Off(u64),
    On { id: u64, pitch: f32, vel: f32, dur: f64 },
}

#[derive(Clone, Copy, Debug)]
struct Ev {
    beat: f64,
    kind: EvKind,
}

struct SendRt {
    to: usize,
    level: Param,
    pre: bool,
    ramp: Ramp,
    delay: Comp,
}

/// Plugin-delay-compensation delay (no-op when 0).
#[derive(Default)]
struct Comp {
    buf: Vec<Frame>,
    pos: usize,
}

impl Comp {
    fn new(d: usize) -> Comp {
        Comp { buf: vec![Frame::ZERO; d], pos: 0 }
    }
    #[inline]
    fn tick(&mut self, x: Frame) -> Frame {
        if self.buf.is_empty() {
            return x;
        }
        let y = std::mem::replace(&mut self.buf[self.pos], x);
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
        y
    }
}

/// Maximum sidechain sources per insert.
const MAX_SC: usize = 8;

struct Chan {
    name: String,
    kind: ChanKind,
    inst: Option<Box<dyn Instrument>>,
    fx: Vec<Box<dyn Effect>>,
    sc: Vec<Vec<(String, usize)>>,
    out_delay: Comp,
    stem_delay: Comp,
    gain: Param,
    pan: Param,
    bend: Param,
    modwheel: Param,
    gl: Ramp,
    gr: Ramp,
    out: Option<usize>,
    sends: Vec<SendRt>,
    events: Vec<Ev>,
    ev_pos: usize,
    silenced: bool,
    meter: Meter,
    notes: usize,
}

/// A prepared, running render. Use [`Engine::process`] to pull audio (e.g. for live playback)
/// or [`Engine::render_all`] for offline rendering.
pub struct Engine {
    pub sr: u32,
    pub name: String,
    chans: Vec<Chan>,
    order: Vec<usize>,
    tempo: Param,
    beat: f64,
    secs: f64,
    frame: u64,
    start_beat: f64,
    end_beat: f64,
    flushed: bool,
    tail: bool,
    max_tail: f32,
    quiet_frames: u64,
    tail_frames: u64,
    finished: bool,
    inputs: Vec<Vec<Frame>>,
    outs: Vec<Vec<Frame>>,
    capture: bool,
    stems: Vec<Vec<Frame>>,
    sections: Vec<crate::song::SectionInfo>,
    started: Instant,
    /// PDC: frames of total master latency still to discard from the output.
    skip_mix: usize,
    /// PDC: frames to trim from the start of stems.
    skip_stems: usize,
    latency: usize,
}

/// Enables flush-to-zero / denormals-are-zero for the current thread while alive, so silent
/// feedback paths (reverbs, filters, plucked strings) never fall into slow denormal arithmetic.
struct FlushDenormals {
    #[cfg(target_arch = "x86_64")]
    saved: u32,
}

impl FlushDenormals {
    fn new() -> FlushDenormals {
        #[cfg(target_arch = "x86_64")]
        {
            let mut csr: u32 = 0;
            // SAFETY: stmxcsr/ldmxcsr only read/write the SSE control register; FTZ (bit 15) and
            // DAZ (bit 6) are supported by every x86_64 CPU.
            unsafe {
                std::arch::asm!("stmxcsr [{}]", in(reg) &mut csr as *mut u32, options(nostack, preserves_flags));
                let new = csr | 0x8040;
                std::arch::asm!("ldmxcsr [{}]", in(reg) &new as *const u32, options(nostack, preserves_flags, readonly));
            }
            FlushDenormals { saved: csr }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            FlushDenormals {}
        }
    }
}

impl Drop for FlushDenormals {
    fn drop(&mut self) {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: restores the value saved in new().
        unsafe {
            std::arch::asm!("ldmxcsr [{}]", in(reg) &self.saved as *const u32, options(nostack, preserves_flags, readonly));
        }
    }
}

/// A finished render.
pub struct Render {
    pub sr: u32,
    pub audio: Vec<Frame>,
    /// (name, audio) per track/bus when [`RenderOptions::stems`] was set.
    pub stems: Vec<(String, Vec<Frame>)>,
    pub report: Report,
}

impl Render {
    pub fn duration_secs(&self) -> f64 {
        self.audio.len() as f64 / self.sr as f64
    }
    /// Write the mix as a 24-bit WAV.
    pub fn write_wav(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        crate::io::wav::write_wav(path, self.sr, &self.audio, crate::io::wav::BitDepth::Int24)
    }
    /// Write each stem as `<dir>/<name>.wav`.
    pub fn write_stems(&self, dir: impl AsRef<Path>) -> Result<Vec<std::path::PathBuf>, Error> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;
        let mut out = vec![];
        for (name, a) in &self.stems {
            let p = dir.join(format!("{}.wav", sanitize(name)));
            crate::io::wav::write_wav(&p, self.sr, a, crate::io::wav::BitDepth::Int24)?;
            out.push(p);
        }
        Ok(out)
    }
}

pub(crate) fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// Per-channel statistics.
#[derive(Clone, Debug)]
pub struct ChanReport {
    pub name: String,
    pub kind: ChanKind,
    pub notes: usize,
    pub peak_db: f32,
    pub rms_db: f32,
}

/// Post-render summary (printable).
#[derive(Clone, Debug)]
pub struct Report {
    pub name: String,
    pub sr: u32,
    pub duration_secs: f64,
    pub render_secs: f64,
    pub channels: Vec<ChanReport>,
    pub master_peak_db: f32,
    pub master_rms_db: f32,
    pub lufs: f32,
    pub sections: Vec<crate::song::SectionInfo>,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let speed = if self.render_secs > 0.0 { self.duration_secs / self.render_secs } else { 0.0 };
        writeln!(f, "♪ {} — {:.1}s @ {} Hz (rendered in {:.2}s, {:.0}× realtime)", self.name, self.duration_secs, self.sr, self.render_secs, speed)?;
        if !self.sections.is_empty() {
            let s: Vec<String> = self.sections.iter().map(|s| format!("{}@{}", s.name, s.start)).collect();
            writeln!(f, "  sections: {}", s.join("  "))?;
        }
        writeln!(f, "  {:<18} {:>6} {:>6} {:>9} {:>9}", "channel", "kind", "notes", "peak dB", "rms dB")?;
        for c in &self.channels {
            let kind = match c.kind {
                ChanKind::Track => "track",
                ChanKind::Bus => "bus",
                ChanKind::Master => "master",
            };
            writeln!(f, "  {:<18} {:>6} {:>6} {:>9.1} {:>9.1}", truncate(&c.name, 18), kind, c.notes, c.peak_db, c.rms_db)?;
        }
        write!(f, "  master: peak {:.1} dBFS, rms {:.1} dB, ~{:.1} LUFS", self.master_peak_db, self.master_rms_db, self.lufs)?;
        if self.master_peak_db > 0.0 {
            write!(f, "  ⚠ CLIPPING")?;
        }
        Ok(())
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n - 1).collect::<String>() + "…" }
}

impl Engine {
    pub(crate) fn build(song: &Song, opts: &RenderOptions) -> Result<Engine, Error> {
        let song_end = song.end();
        let all_notes: Vec<Vec<crate::clip::Note>> = std::iter::once(vec![]).chain(song.tracks().iter().map(|t| t.notes())).collect();
        let mut d = song.data_mut();
        if d.rendered {
            return Err(Error::AlreadyRendered);
        }
        d.rendered = true;
        let sr = opts.sample_rate.unwrap_or(d.sr);
        let n = d.chans.len();
        let names: Vec<String> = d.chans.iter().map(|c| c.name.clone()).collect();
        for s in opts.solo.iter().chain(opts.mute.iter()) {
            if !names.contains(s) {
                return Err(Error::Invalid(format!("unknown track {s:?} (have: {})", names.join(", "))));
            }
        }
        let any_solo = !opts.solo.is_empty() || d.chans.iter().any(|c| c.solo);

        let mut next_id: u64 = 1;
        let mut chans = Vec::with_capacity(n);
        for (i, c) in d.chans.iter_mut().enumerate() {
            // ---- events
            let mut evs = vec![];
            let notes = all_notes[i].len();
            for nt in &all_notes[i] {
                let id = next_id;
                next_id += 1;
                evs.push(Ev { beat: nt.start, kind: EvKind::On { id, pitch: nt.pitch, vel: nt.vel, dur: nt.dur } });
                evs.push(Ev { beat: nt.start + nt.dur, kind: EvKind::Off(id) });
            }
            evs.sort_by(|a, b| {
                a.beat.total_cmp(&b.beat).then_with(|| {
                    let ra = matches!(a.kind, EvKind::On { .. }) as u8;
                    let rb = matches!(b.kind, EvKind::On { .. }) as u8;
                    ra.cmp(&rb)
                })
            });
            // ---- sidechains
            let mut sc = vec![];
            for f in &c.fx {
                let mut srcs = vec![];
                for src in f.sidechains() {
                    match names.iter().position(|x| *x == src) {
                        Some(j) if j != i => {
                            if !srcs.iter().any(|(n, _): &(String, usize)| *n == src) && srcs.len() < MAX_SC {
                                srcs.push((src, j));
                            }
                        }
                        Some(_) => return Err(Error::Graph(format!("{:?} cannot sidechain itself", c.name))),
                        None => return Err(Error::Graph(format!("{:?}: sidechain source {src:?} not found (have: {})", c.name, names.join(", ")))),
                    }
                }
                sc.push(srcs);
            }
            let mut inst = c.instrument.take();
            if let Some(inst) = inst.as_mut() {
                inst.prepare(sr as f32);
                let start = opts.from.map(|t| t.0).unwrap_or(0.0);
                let stop = opts.to.map(|t| t.0).unwrap_or(f64::INFINITY);
                let used: Vec<crate::clip::Note> = all_notes[i].iter().filter(|n| n.start >= start && n.start < stop).copied().collect();
                inst.preload(&used);
            }
            let mut fx = std::mem::take(&mut c.fx);
            for f in &mut fx {
                f.prepare(sr as f32);
            }
            let silenced = c.kind == ChanKind::Track
                && (c.mute || opts.mute.contains(&c.name) || (any_solo && !(c.solo || opts.solo.contains(&c.name))));
            chans.push(Chan {
                name: c.name.clone(),
                kind: c.kind,
                inst,
                fx,
                sc,
                out_delay: Comp::default(),
                stem_delay: Comp::default(),
                gain: c.gain.clone(),
                pan: c.pan.clone(),
                bend: c.bend.clone(),
                modwheel: c.modwheel.clone(),
                gl: Ramp::default(),
                gr: Ramp::default(),
                out: if c.kind == ChanKind::Master { None } else { Some(c.output.unwrap_or(0)) },
                sends: c.sends.iter().map(|s| SendRt { to: s.to, level: s.level.clone(), pre: s.pre, ramp: Ramp::default(), delay: Comp::default() }).collect(),
                events: evs,
                ev_pos: 0,
                silenced,
                meter: Meter::new(sr as f32),
                notes,
            });
        }

        // ---- topological order
        let mut deps: Vec<Vec<usize>> = vec![vec![]; n]; // deps[i] = channels i waits for
        for (i, c) in chans.iter().enumerate() {
            if let Some(o) = c.out {
                deps[o].push(i);
            }
            for s in &c.sends {
                if s.to == i {
                    return Err(Error::Graph(format!("{:?} sends to itself", c.name)));
                }
                deps[s.to].push(i);
            }
            for s in c.sc.iter().flatten() {
                deps[i].push(s.1);
            }
        }
        let mut order = vec![];
        let mut state = vec![0u8; n]; // 0 new, 1 visiting, 2 done
        fn visit(i: usize, deps: &[Vec<usize>], state: &mut [u8], order: &mut Vec<usize>, chans: &[Chan]) -> Result<(), Error> {
            match state[i] {
                2 => return Ok(()),
                1 => return Err(Error::Graph(format!("routing cycle through {:?}", chans[i].name))),
                _ => {}
            }
            state[i] = 1;
            for &j in &deps[i] {
                visit(j, deps, state, order, chans)?;
            }
            state[i] = 2;
            order.push(i);
            Ok(())
        }
        for i in 0..n {
            visit(i, &deps, &mut state, &mut order, &chans)?;
        }

        // ---- plugin delay compensation
        let lat: Vec<usize> = chans.iter().map(|c| c.fx.iter().map(|f| f.latency()).sum()).collect();
        let mut lin = vec![0usize; n];
        let mut lout = vec![0usize; n];
        for &i in &order {
            lout[i] = lin[i] + lat[i];
            let c = &chans[i];
            for d in c.out.iter().copied().chain(c.sends.iter().map(|s| s.to)) {
                lin[d] = lin[d].max(lout[i]);
            }
        }
        // order is topological, but a destination's lin may grow after we visited an earlier
        // source, so recompute until stable (graphs are tiny)
        loop {
            let mut changed = false;
            for &i in &order {
                let lo = lin[i] + lat[i];
                if lo != lout[i] {
                    lout[i] = lo;
                    changed = true;
                }
                let c = &chans[i];
                for d in c.out.iter().copied().chain(c.sends.iter().map(|s| s.to)) {
                    if lout[i] > lin[d] {
                        lin[d] = lout[i];
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for i in 0..n {
            let lo = lout[i];
            let c = &mut chans[i];
            if let Some(o) = c.out {
                c.out_delay = Comp::new(lin[o] - lo);
            }
            for s in &mut c.sends {
                s.delay = Comp::new(lin[s.to] - lo);
            }
            c.stem_delay = Comp::new(lin[0].saturating_sub(lo));
        }

        let tempo = d.tempo.clone();
        let start_beat = opts.from.map(|t| t.0).unwrap_or(0.0).max(0.0);
        let end_beat = opts.to.map(|t| t.0).unwrap_or(song_end.0);
        // seconds at start: integrate tempo
        let mut secs = 0.0;
        let mut b = 0.0;
        let step: f64 = 1.0 / 32.0;
        while b < start_beat {
            let db = step.min(start_beat - b);
            let bpm = tempo.at(Clock { beat: b, secs }).max(1.0) as f64;
            secs += db * 60.0 / bpm;
            b += db;
        }
        for c in &mut chans {
            c.ev_pos = c.events.partition_point(|e| e.beat < start_beat);
        }
        let tail = !opts.no_tail;
        Ok(Engine {
            sr,
            name: d.name.clone(),
            inputs: vec![vec![Frame::ZERO; MAX_BLOCK]; n],
            outs: vec![vec![Frame::ZERO; MAX_BLOCK]; n],
            stems: vec![vec![]; n],
            capture: opts.stems,
            chans,
            order,
            tempo,
            beat: start_beat,
            secs,
            frame: 0,
            start_beat,
            end_beat,
            flushed: false,
            tail,
            max_tail: d.max_tail,
            quiet_frames: 0,
            tail_frames: 0,
            finished: end_beat <= start_beat,
            sections: d.sections.clone(),
            started: Instant::now(),
            skip_mix: lout[0],
            skip_stems: lin[0],
            latency: lout[0],
        })
    }

    /// Total plugin-delay-compensation latency of the mix in frames (already compensated in the
    /// output of [`Engine::process`]).
    pub fn latency(&self) -> usize {
        self.latency
    }
    /// Current timeline position.
    pub fn position(&self) -> Clock {
        Clock { beat: self.beat, secs: self.secs }
    }
    pub fn is_finished(&self) -> bool {
        self.finished
    }
    /// Where the song ends (beats).
    pub fn end_beat(&self) -> f64 {
        self.end_beat
    }
    pub fn start_beat(&self) -> f64 {
        self.start_beat
    }
    /// Channel names in processing order.
    pub fn channel_names(&self) -> Vec<String> {
        self.chans.iter().map(|c| c.name.clone()).collect()
    }

    /// Fill `out` with the next frames. Returns how many frames were written; fewer than
    /// `out.len()` means the song is over.
    pub fn process(&mut self, out: &mut [Frame]) -> usize {
        let _ftz = FlushDenormals::new();
        while self.skip_mix > 0 && !self.finished {
            let k = self.skip_mix.min(MAX_BLOCK);
            self.block(k);
            self.skip_mix -= k;
        }
        let mut done = 0;
        while done < out.len() && !self.finished {
            let n = (out.len() - done).min(MAX_BLOCK);
            self.block(n);
            out[done..done + n].copy_from_slice(&self.outs[0][..n]);
            done += n;
        }
        done
    }

    /// Render everything into memory.
    pub fn render_all(&mut self) -> Render {
        let mut audio = Vec::with_capacity(((self.end_beat - self.beat).max(0.0) * 0.5 * self.sr as f64) as usize + 1024);
        let mut buf = [Frame::ZERO; MAX_BLOCK];
        loop {
            let k = self.process(&mut buf);
            audio.extend_from_slice(&buf[..k]);
            if k < MAX_BLOCK {
                break;
            }
        }
        let report = self.report();
        let stems = if self.capture {
            self.chans
                .iter()
                .zip(self.stems.iter_mut())
                .filter(|(c, _)| c.kind != ChanKind::Master)
                .map(|(c, s)| {
                    let mut s = std::mem::take(s);
                    s.drain(..self.skip_stems.min(s.len()));
                    (c.name.clone(), s)
                })
                .collect()
        } else {
            vec![]
        };
        Render { sr: self.sr, audio, stems, report }
    }

    /// Statistics so far.
    pub fn report(&self) -> Report {
        let m = &self.chans[0].meter;
        Report {
            name: self.name.clone(),
            sr: self.sr,
            duration_secs: self.frame as f64 / self.sr as f64,
            render_secs: self.started.elapsed().as_secs_f64(),
            channels: self
                .chans
                .iter()
                .map(|c| ChanReport { name: c.name.clone(), kind: c.kind, notes: c.notes, peak_db: c.meter.peak_db(), rms_db: c.meter.rms_db() })
                .collect(),
            master_peak_db: m.peak_db(),
            master_rms_db: m.rms_db(),
            lufs: m.lufs(),
            sections: self.sections.clone(),
        }
    }

    fn block(&mut self, n: usize) {
        let clock = Clock { beat: self.beat, secs: self.secs };
        let bpm = (self.tempo.at(clock) as f64).max(1.0);
        let sr = self.sr as f32;
        let bps = bpm / 60.0 / sr as f64;
        let beat_end = self.beat + n as f64 * bps;
        let base = Ctx { sr, clock, bpm, frame: self.frame, sidechain: None, sidechains: &[], bend: 0.0, modwheel: 0.0 };
        let over = self.beat >= self.end_beat;

        if over && !self.flushed {
            // release every note still held
            self.flushed = true;
            for c in &mut self.chans {
                if let Some(inst) = c.inst.as_mut() {
                    for e in &c.events[c.ev_pos..] {
                        if let EvKind::Off(id) = e.kind {
                            inst.note_off(id, &base);
                        }
                    }
                }
                c.ev_pos = c.events.len();
            }
        }

        let Engine { chans, order, inputs, outs, stems, capture, .. } = self;
        for &ci in order.iter() {
            let mut buf = std::mem::take(&mut inputs[ci]);
            let c = &mut chans[ci];
            let base = Ctx { bend: c.bend.at(clock), modwheel: c.modwheel.at(clock), ..base };
            // ---- instrument + events (sample accurate)
            if let Some(inst) = c.inst.as_mut() {
                let mut pos = 0usize;
                loop {
                    let next = c.events.get(c.ev_pos).copied().filter(|e| e.beat < beat_end && e.beat < self.end_beat.max(0.0) + 1e9);
                    let off = match next {
                        Some(e) => (((e.beat - base.clock.beat) / bps).ceil().max(0.0) as usize).min(n),
                        None => n,
                    };
                    if off > pos {
                        let cx = base.advanced(pos);
                        inst.render(&mut buf[pos..off], &cx);
                        pos = off;
                    }
                    let Some(e) = next else { break };
                    let cx = base.advanced(pos);
                    match e.kind {
                        EvKind::On { id, pitch, vel, dur } => {
                            if e.beat < self.end_beat {
                                let note = NoteOn { id, pitch, vel, dur: (dur * 60.0 / bpm) as f32 };
                                inst.note_on(&note, &cx);
                            }
                        }
                        EvKind::Off(id) => inst.note_off(id, &cx),
                    }
                    c.ev_pos += 1;
                }
            }
            let b = &mut buf[..n];
            // ---- inserts
            for (fx, sc) in c.fx.iter_mut().zip(c.sc.iter()) {
                let mut arr: [(&str, &[Frame]); MAX_SC] = [("", &[]); MAX_SC];
                for (k, (name, src)) in sc.iter().enumerate() {
                    arr[k] = (name.as_str(), &outs[*src][..n]);
                }
                let list = &arr[..sc.len()];
                let cx = Ctx { sidechain: list.first().map(|x| x.1), sidechains: list, ..base };
                fx.process(b, &cx);
            }
            // ---- pre-fader sends
            for s in c.sends.iter_mut().filter(|s| s.pre) {
                let lv = if c.silenced { 0.0 } else { s.level.at(clock) };
                s.ramp.start(lv, n);
                let dst = &mut inputs[s.to];
                for (k, f) in b.iter().enumerate() {
                    dst[k] += s.delay.tick(*f * s.ramp.next());
                }
            }
            // ---- fader + pan
            let g = if c.silenced { 0.0 } else { c.gain.at(clock) };
            let (pl, pr) = pan_gains(c.pan.at(clock));
            c.gl.start(g * pl, n);
            c.gr.start(g * pr, n);
            for f in b.iter_mut() {
                f.l *= c.gl.next();
                f.r *= c.gr.next();
            }
            // ---- post-fader sends
            for s in c.sends.iter_mut().filter(|s| !s.pre) {
                let lv = s.level.at(clock);
                s.ramp.start(lv, n);
                let dst = &mut inputs[s.to];
                for (k, f) in b.iter().enumerate() {
                    dst[k] += s.delay.tick(*f * s.ramp.next());
                }
            }
            // ---- output routing
            if let Some(o) = c.out {
                let dst = &mut inputs[o];
                for (k, f) in b.iter().enumerate() {
                    dst[k] += c.out_delay.tick(*f);
                }
            }
            c.meter.process(b);
            outs[ci][..n].copy_from_slice(b);
            if *capture && c.kind != ChanKind::Master {
                let sd = &mut c.stem_delay;
                stems[ci].extend(b.iter().map(|f| sd.tick(*f)));
            }
            for f in buf.iter_mut() {
                *f = Frame::ZERO;
            }
            inputs[ci] = buf;
        }

        // ---- advance time
        self.beat = beat_end;
        self.secs += n as f64 / sr as f64;
        self.frame += n as u64;

        if over {
            if !self.tail {
                self.finished = true;
                return;
            }
            self.tail_frames += n as u64;
            let peak = self.outs[0][..n].iter().fold(0f32, |m, f| m.max(f.peak()));
            if peak < 1e-4 {
                self.quiet_frames += n as u64;
            } else {
                self.quiet_frames = 0;
            }
            if self.quiet_frames as f32 > 0.25 * sr || self.tail_frames as f32 > self.max_tail * sr {
                self.finished = true;
            }
        }
    }
}
