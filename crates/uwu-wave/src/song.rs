//! The song: tracks, buses, routing, arrangement.
//!
//! ```ignore
//! let song = Song::new("Neon Rain").bpm(124).meter(4, 4);
//! let verb = song.bus("verb").fx(Reverb::hall());
//! let kick = song.track("kick", DrumKit::eight_oh_eight()).gain(-2.db());
//! let bass = song.track("bass", Synth::acid())
//!     .fx(Compressor::new().sidechain("kick"))
//!     .send(&verb, -12.db());
//! song.section("intro", 8.bars(), |s| {
//!     s.play(&kick, "bd ~ bd ~");
//!     s.play(&bass, "c2 ~ c2 eb2");
//! });
//! song.run();
//! ```
//!
//! Handles ([`Song`], [`Track`]) are cheap reference-counted clones: keep as many as you like
//! and mutate through any of them. Builder methods take `&self` and return a handle so they
//! chain both as expressions and as statements.

use crate::clip::{IntoSeq, Seq};
use crate::engine::{Engine, Render, RenderOptions};
use crate::fx::Effect;
use crate::instrument::Instrument;
use crate::param::{Curve, Lane, Param};
use crate::time::{Span, Time, set_meter};
use crate::units::Val;
use crate::Error;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChanKind {
    Track,
    Bus,
    Master,
}

pub(crate) struct Placement {
    pub span: Span,
    pub seq: Arc<dyn Seq>,
}

pub(crate) struct SendDef {
    pub to: usize,
    pub level: Param,
    pub pre: bool,
}

pub(crate) struct ChanData {
    pub name: String,
    pub kind: ChanKind,
    pub instrument: Option<Box<dyn Instrument>>,
    pub fx: Vec<Box<dyn Effect>>,
    pub gain: Param,
    pub pan: Param,
    pub mute: bool,
    pub solo: bool,
    pub output: Option<usize>,
    pub sends: Vec<SendDef>,
    pub clips: Vec<Placement>,
    pub transpose: f32,
    pub vel: f32,
}

/// A named region of the arrangement.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionInfo {
    pub name: String,
    pub start: Time,
    pub len: Time,
}
impl SectionInfo {
    pub fn end(&self) -> Time {
        self.start + self.len
    }
}

pub(crate) struct SongData {
    pub name: String,
    pub tempo: Param,
    pub meter: (u32, u32),
    pub sr: u32,
    pub chans: Vec<ChanData>,
    pub sections: Vec<SectionInfo>,
    pub markers: Vec<(String, Time)>,
    pub cursor: Time,
    pub length: Option<Time>,
    pub max_tail: f32,
    pub rendered: bool,
}

/// A song handle. Cheap to clone.
#[derive(Clone)]
pub struct Song(pub(crate) Rc<RefCell<SongData>>);

/// A track/bus/master handle. Cheap to clone.
#[derive(Clone)]
pub struct Track {
    song: Rc<RefCell<SongData>>,
    idx: usize,
}

impl Song {
    /// New song at 120 BPM in 4/4, 48 kHz. Resets the thread's meter to 4/4.
    pub fn new(name: impl Into<String>) -> Song {
        set_meter(4, 4);
        let master = ChanData::new("master", ChanKind::Master, None);
        Song(Rc::new(RefCell::new(SongData {
            name: name.into(),
            tempo: Param::Const(120.0),
            meter: (4, 4),
            sr: 48_000,
            chans: vec![master],
            sections: vec![],
            markers: vec![],
            cursor: Time::ZERO,
            length: None,
            max_tail: 12.0,
            rendered: false,
        })))
    }
    /// Tempo: a constant (`.bpm(128)`) or automation (`.bpm(auto![0 => 90, bar(16) => 140])`).
    pub fn bpm(&self, bpm: impl Into<Param>) -> Song {
        self.0.borrow_mut().tempo = bpm.into();
        self.clone()
    }
    /// Alias of [`Song::bpm`].
    pub fn tempo(&self, bpm: impl Into<Param>) -> Song {
        self.bpm(bpm)
    }
    /// Time signature. Call it before creating bar positions.
    pub fn meter(&self, num: u32, den: u32) -> Song {
        set_meter(num, den);
        self.0.borrow_mut().meter = (num, den);
        self.clone()
    }
    pub fn sample_rate(&self, sr: u32) -> Song {
        self.0.borrow_mut().sr = sr;
        self.clone()
    }
    /// Force the song length (otherwise: end of the last section/clip).
    pub fn length(&self, t: impl Into<Time>) -> Song {
        self.0.borrow_mut().length = Some(t.into());
        self.clone()
    }
    /// Maximum seconds of reverb/release tail rendered after the end (default 12; stops early on silence).
    pub fn tail(&self, secs: f32) -> Song {
        self.0.borrow_mut().max_tail = secs;
        self.clone()
    }
    pub fn name(&self) -> String {
        self.0.borrow().name.clone()
    }
    pub fn sr(&self) -> u32 {
        self.0.borrow().sr
    }
    pub fn meter_sig(&self) -> (u32, u32) {
        self.0.borrow().meter
    }
    pub fn tempo_param(&self) -> Param {
        self.0.borrow().tempo.clone()
    }

    fn add_chan(&self, c: ChanData) -> Track {
        let mut d = self.0.borrow_mut();
        if d.chans.iter().any(|x| x.name == c.name) {
            panic!("uwu-wave: a track or bus named {:?} already exists", c.name);
        }
        d.chans.push(c);
        Track { song: self.0.clone(), idx: d.chans.len() - 1 }
    }
    /// Add an instrument track (routed to master).
    pub fn track(&self, name: impl Into<String>, instrument: impl Instrument + 'static) -> Track {
        self.add_chan(ChanData::new(name, ChanKind::Track, Some(Box::new(instrument))))
    }
    /// Add a bus (an effect return / group). Route tracks into it with [`Track::to`] or [`Track::send`].
    pub fn bus(&self, name: impl Into<String>) -> Track {
        self.add_chan(ChanData::new(name, ChanKind::Bus, None))
    }
    /// The master bus (add mastering effects here).
    pub fn master(&self) -> Track {
        Track { song: self.0.clone(), idx: 0 }
    }
    /// Look up a track/bus by name.
    pub fn get(&self, name: &str) -> Option<Track> {
        let d = self.0.borrow();
        d.chans.iter().position(|c| c.name == name).map(|idx| Track { song: self.0.clone(), idx })
    }
    /// All tracks and buses (excluding master).
    pub fn tracks(&self) -> Vec<Track> {
        let n = self.0.borrow().chans.len();
        (1..n).map(|idx| Track { song: self.0.clone(), idx }).collect()
    }

    /// Append a section of length `len` at the arrangement cursor and fill it.
    /// Returns the section's start time.
    pub fn section(&self, name: impl Into<String>, len: impl Into<Time>, f: impl FnOnce(&mut Section)) -> Time {
        let (start, len) = {
            let d = self.0.borrow();
            (d.cursor, len.into())
        };
        let mut s = Section { song: self.clone(), name: name.into(), start, len };
        f(&mut s);
        let mut d = self.0.borrow_mut();
        d.sections.push(SectionInfo { name: s.name.clone(), start, len });
        d.cursor = start + len;
        start
    }
    /// Move the arrangement cursor (where the next section starts).
    pub fn goto(&self, t: impl Into<Time>) -> &Song {
        self.0.borrow_mut().cursor = t.into();
        self
    }
    /// Current arrangement cursor.
    pub fn cursor(&self) -> Time {
        self.0.borrow().cursor
    }
    /// Add a named marker (usable with `--from <name>`).
    pub fn marker(&self, name: impl Into<String>, at: impl Into<Time>) -> &Song {
        self.0.borrow_mut().markers.push((name.into(), at.into()));
        self
    }
    pub fn sections(&self) -> Vec<SectionInfo> {
        self.0.borrow().sections.clone()
    }
    pub fn markers(&self) -> Vec<(String, Time)> {
        let d = self.0.borrow();
        let mut m: Vec<(String, Time)> = d.sections.iter().map(|s| (s.name.clone(), s.start)).collect();
        m.extend(d.markers.iter().cloned());
        m
    }
    /// Resolve a section or marker name to a time.
    pub fn locate(&self, name: &str) -> Option<Time> {
        self.markers().into_iter().find(|(n, _)| n == name).map(|(_, t)| t)
    }
    /// Where the song ends (before the tail).
    pub fn end(&self) -> Time {
        let d = self.0.borrow();
        if let Some(l) = d.length {
            return l;
        }
        let mut end = d.cursor;
        for c in &d.chans {
            for p in &c.clips {
                end = end.max(match p.span {
                    Span::At(t) => t + Time(p.seq.len()),
                    Span::Range(_, e) => e,
                    Span::From(t) => t,
                });
            }
        }
        end
    }

    /// Build a render engine (consumes the instruments/effects: a song renders once).
    pub fn engine(&self, opts: &RenderOptions) -> Result<Engine, Error> {
        Engine::build(self, opts)
    }
    /// Render with options.
    pub fn render_with(&self, opts: &RenderOptions) -> Result<Render, Error> {
        let mut e = self.engine(opts)?;
        Ok(e.render_all())
    }
    /// Render the whole song with default options.
    pub fn render(&self) -> Result<Render, Error> {
        self.render_with(&RenderOptions::default())
    }
    /// Render and write a WAV file (24-bit).
    pub fn render_wav(&self, path: impl AsRef<Path>) -> Result<Render, Error> {
        let r = self.render()?;
        r.write_wav(path)?;
        Ok(r)
    }
    /// Command-line entry point: `cargo run --example my_song -- --help`.
    pub fn run(self) {
        crate::io::cli::run(self)
    }

    #[allow(dead_code)]
    pub(crate) fn data(&self) -> std::cell::Ref<'_, SongData> {
        self.0.borrow()
    }
    pub(crate) fn data_mut(&self) -> std::cell::RefMut<'_, SongData> {
        self.0.borrow_mut()
    }
}

impl ChanData {
    fn new(name: impl Into<String>, kind: ChanKind, instrument: Option<Box<dyn Instrument>>) -> ChanData {
        ChanData {
            name: name.into(),
            kind,
            instrument,
            fx: vec![],
            gain: Param::Const(1.0),
            pan: Param::Const(0.0),
            mute: false,
            solo: false,
            output: None,
            sends: vec![],
            clips: vec![],
            transpose: 0.0,
            vel: 1.0,
        }
    }
}

impl Track {
    fn with(&self, f: impl FnOnce(&mut ChanData)) -> Track {
        f(&mut self.song.borrow_mut().chans[self.idx]);
        self.clone()
    }
    pub fn name(&self) -> String {
        self.song.borrow().chans[self.idx].name.clone()
    }
    pub fn kind(&self) -> ChanKind {
        self.song.borrow().chans[self.idx].kind
    }
    #[allow(dead_code)]
    pub(crate) fn index(&self) -> usize {
        self.idx
    }
    /// Replace the instrument.
    pub fn instrument(&self, i: impl Instrument + 'static) -> Track {
        self.with(|c| c.instrument = Some(Box::new(i)))
    }
    /// Append an effect to the insert chain.
    pub fn fx(&self, e: impl Effect + 'static) -> Track {
        self.with(|c| c.fx.push(Box::new(e)))
    }
    /// Fader gain (linear; `-6.db()` works). Accepts automation: `.gain(auto![...].into())`.
    pub fn gain(&self, g: impl Into<Param>) -> Track {
        let g = g.into();
        self.with(|c| c.gain = g)
    }
    /// Fader in decibels, possibly automated: `.volume_db(auto![0 => -40, bar(4) => 0])`.
    pub fn volume_db(&self, db: impl Into<Param>) -> Track {
        let g = db.into().db_to_gain();
        self.with(|c| c.gain = g)
    }
    /// Alias of [`Track::gain`].
    pub fn volume(&self, g: impl Into<Param>) -> Track {
        self.gain(g)
    }
    /// Pan -1 (left) .. 1 (right); automatable.
    pub fn pan(&self, p: impl Into<Param>) -> Track {
        let p = p.into();
        self.with(|c| c.pan = p)
    }
    pub fn mute(&self) -> Track {
        self.with(|c| c.mute = true)
    }
    pub fn muted(&self, m: bool) -> Track {
        self.with(|c| c.mute = m)
    }
    pub fn solo(&self) -> Track {
        self.with(|c| c.solo = true)
    }
    /// Transpose all notes on this track (semitones).
    pub fn transpose(&self, st: impl Val) -> Track {
        let st = st.val();
        self.with(|c| c.transpose = st)
    }
    /// Scale all velocities on this track.
    pub fn velocity(&self, k: impl Val) -> Track {
        let k = k.val();
        self.with(|c| c.vel = k)
    }
    /// Route this channel's output into a bus (instead of master).
    pub fn to(&self, bus: &Track) -> Track {
        let b = bus.idx;
        self.with(|c| c.output = Some(b))
    }
    /// Post-fader send to a bus.
    pub fn send(&self, bus: &Track, level: impl Into<Param>) -> Track {
        let (to, level) = (bus.idx, level.into());
        self.with(|c| c.sends.push(SendDef { to, level, pre: false }))
    }
    /// Pre-fader send to a bus.
    pub fn send_pre(&self, bus: &Track, level: impl Into<Param>) -> Track {
        let (to, level) = (bus.idx, level.into());
        self.with(|c| c.sends.push(SendDef { to, level, pre: true }))
    }
    /// Place a sequence:
    /// * `play(bar(4), clip)` – once, starting at bar 4
    /// * `play(bars(0..8), pat)` – looped over bars 0..8
    /// * `play(bar(8).., pat)` – looped from bar 8 to the end of the song
    pub fn play(&self, at: impl Into<Span>, seq: impl IntoSeq) -> Track {
        let p = Placement { span: at.into(), seq: seq.into_seq() };
        self.with(|c| c.clips.push(p))
    }
    /// Number of placed clips.
    pub fn clip_count(&self) -> usize {
        self.song.borrow().chans[self.idx].clips.len()
    }
    /// All notes of this track on the absolute timeline (beats), with the track's transpose and
    /// velocity scaling applied and notes clipped at their placement's end. Sorted by start.
    pub fn notes(&self) -> Vec<crate::clip::Note> {
        let song = Song(self.song.clone());
        let end = song.end();
        let d = self.song.borrow();
        let c = &d.chans[self.idx];
        let mut out = vec![];
        for p in &c.clips {
            let (start, stop) = p.bounds(end);
            if stop <= start {
                continue;
            }
            for nt in p.seq.notes(0.0, stop - start) {
                let s = start + nt.start;
                if s >= stop || nt.dur <= 0.0 {
                    continue;
                }
                out.push(crate::clip::Note {
                    start: s,
                    dur: (s + nt.dur).min(stop) - s,
                    pitch: nt.pitch + c.transpose,
                    vel: (nt.vel * c.vel).clamp(0.0, 1.0),
                });
            }
        }
        out.sort_by(|a, b| a.start.total_cmp(&b.start));
        out
    }
    /// Placement regions `(start, end)` in beats (for arrangement views).
    pub fn regions(&self) -> Vec<(f64, f64)> {
        let end = Song(self.song.clone()).end();
        let d = self.song.borrow();
        d.chans[self.idx].clips.iter().map(|p| p.bounds(end)).collect()
    }
}

impl Placement {
    /// Absolute (start, end) in beats given the song end.
    pub(crate) fn bounds(&self, song_end: Time) -> (f64, f64) {
        match self.span {
            Span::At(t) => (t.0, t.0 + self.seq.len()),
            Span::Range(a, b) => (a.0, b.0),
            Span::From(a) => (a.0, song_end.0.max(a.0)),
        }
    }
}

/// A section being filled (see [`Song::section`]). Positions are section-relative.
pub struct Section {
    song: Song,
    pub name: String,
    pub start: Time,
    pub len: Time,
}

impl Section {
    /// Absolute end time.
    pub fn end(&self) -> Time {
        self.start + self.len
    }
    /// Absolute time of a section-relative offset.
    pub fn abs(&self, offset: impl Into<Time>) -> Time {
        self.start + offset.into()
    }
    /// Loop `seq` over the whole section.
    pub fn play(&mut self, track: &Track, seq: impl IntoSeq) -> &mut Self {
        track.play(self.start..self.end(), seq);
        self
    }
    /// Play `seq` once at the section start.
    pub fn once(&mut self, track: &Track, seq: impl IntoSeq) -> &mut Self {
        track.play(self.start, seq);
        self
    }
    /// Play `seq` once at a section-relative offset.
    pub fn at(&mut self, offset: impl Into<Time>, track: &Track, seq: impl IntoSeq) -> &mut Self {
        track.play(self.abs(offset), seq);
        self
    }
    /// Loop `seq` over a section-relative range.
    pub fn during(&mut self, range: std::ops::Range<Time>, track: &Track, seq: impl IntoSeq) -> &mut Self {
        track.play(self.abs(range.start)..self.abs(range.end).min(self.end()), seq);
        self
    }
    /// Ramp an automation lane linearly across the whole section.
    pub fn ramp(&mut self, lane: &Lane, from: impl Val, to: impl Val) -> &mut Self {
        lane.ramp(self.start, self.end(), from, to, Curve::Linear);
        self
    }
    /// Ramp an automation lane exponentially across the whole section (use for frequencies).
    pub fn ramp_exp(&mut self, lane: &Lane, from: impl Val, to: impl Val) -> &mut Self {
        lane.ramp(self.start, self.end(), from, to, Curve::Exp);
        self
    }
    /// Ramp over a section-relative range with any curve.
    pub fn ramp_in(&mut self, lane: &Lane, range: std::ops::Range<Time>, from: impl Val, to: impl Val, curve: Curve) -> &mut Self {
        lane.ramp(self.abs(range.start), self.abs(range.end), from, to, curve);
        self
    }
    /// Set a lane value from the section start on.
    pub fn set(&mut self, lane: &Lane, v: impl Val) -> &mut Self {
        lane.set(self.start, v);
        self
    }
    /// Set a lane value at a section-relative offset.
    pub fn set_at(&mut self, offset: impl Into<Time>, lane: &Lane, v: impl Val) -> &mut Self {
        lane.set(self.abs(offset), v);
        self
    }
    /// Add a marker at a section-relative offset.
    pub fn marker(&mut self, name: impl Into<String>, offset: impl Into<Time>) -> &mut Self {
        let t = self.abs(offset);
        self.song.marker(name, t);
        self
    }
    /// The song this section belongs to.
    pub fn song(&self) -> &Song {
        &self.song
    }
}
