//! Instruments receive note events and render audio.
//!
//! Most instruments are built from a [`Voice`] wrapped in [`Poly`], which handles voice
//! allocation, stealing, mono/legato modes and glide.

use crate::ctx::Ctx;
use crate::dsp::Frame;

/// A note-on event as seen by an instrument.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteOn {
    /// Unique id; the matching [`Instrument::note_off`] uses it.
    pub id: u64,
    /// Fractional MIDI note number.
    pub pitch: f32,
    /// 0..1
    pub vel: f32,
    /// Nominal duration in seconds (handy for one-shots / plucks; note-off still arrives).
    pub dur: f32,
}

/// The instrument interface. Implementations **add** into `out` (it may already contain audio).
pub trait Instrument: Send {
    /// Called once before rendering with the sample rate.
    fn prepare(&mut self, _sr: f32) {}
    /// Called after [`prepare`](Instrument::prepare) with every note this instrument will play
    /// (absolute beats, track transpose/velocity applied). Sample-based instruments load exactly
    /// what they need here, so nothing is loaded or downloaded while audio is running.
    fn preload(&mut self, _notes: &[crate::clip::Note]) {}
    fn note_on(&mut self, note: &NoteOn, ctx: &Ctx);
    fn note_off(&mut self, id: u64, ctx: &Ctx);
    /// Add audio for this block into `out`.
    fn render(&mut self, out: &mut [Frame], ctx: &Ctx);
    /// Is anything still sounding? (used to know when the song tail is over)
    fn active(&self) -> bool {
        true
    }
    /// Silence everything immediately.
    fn panic(&mut self) {}
    fn name(&self) -> String {
        "instrument".into()
    }
}

impl Instrument for Box<dyn Instrument> {
    fn prepare(&mut self, sr: f32) {
        (**self).prepare(sr)
    }
    fn preload(&mut self, notes: &[crate::clip::Note]) {
        (**self).preload(notes)
    }
    fn note_on(&mut self, note: &NoteOn, ctx: &Ctx) {
        (**self).note_on(note, ctx)
    }
    fn note_off(&mut self, id: u64, ctx: &Ctx) {
        (**self).note_off(id, ctx)
    }
    fn render(&mut self, out: &mut [Frame], ctx: &Ctx) {
        (**self).render(out, ctx)
    }
    fn active(&self) -> bool {
        (**self).active()
    }
    fn panic(&mut self) {
        (**self).panic()
    }
    fn name(&self) -> String {
        (**self).name()
    }
}

/// One voice of a polyphonic instrument. Voices are cloned from a prototype.
pub trait Voice: Send + Clone + 'static {
    fn prepare(&mut self, _sr: f32) {}
    /// Start a note (fresh attack).
    fn start(&mut self, note: &NoteOn, ctx: &Ctx);
    /// Mono/legato mode: move to a new pitch without retriggering, over `glide` seconds.
    /// Default: restart.
    fn legato(&mut self, note: &NoteOn, _glide: f32, ctx: &Ctx) {
        self.start(note, ctx)
    }
    /// Key released.
    fn release(&mut self, ctx: &Ctx);
    /// Add this voice's audio into `out`. Return `false` once the voice has gone silent.
    fn render(&mut self, out: &mut [Frame], ctx: &Ctx) -> bool;
}

/// Voice allocation mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PolyMode {
    /// Up to `max` simultaneous voices.
    Poly,
    /// One voice, every note retriggers.
    Mono,
    /// One voice; overlapping notes glide/legato without retriggering (acid basslines!).
    Legato,
}

#[derive(Clone)]
struct Slot<V> {
    voice: V,
    id: u64,
    held: bool,
    alive: bool,
    age: u64,
}

/// Polyphonic voice manager. `Poly::new(voice)` → 16 voices.
#[derive(Clone)]
pub struct Poly<V: Voice> {
    proto: V,
    slots: Vec<Slot<V>>,
    pub max: usize,
    pub mode: PolyMode,
    /// Glide time (seconds) for Legato mode.
    pub glide: f32,
    held: Vec<NoteOn>,
    counter: u64,
    sr: f32,
    name: String,
}

impl<V: Voice> Poly<V> {
    pub fn new(proto: V) -> Poly<V> {
        Poly { proto, slots: vec![], max: 16, mode: PolyMode::Poly, glide: 0.0, held: vec![], counter: 0, sr: 48000.0, name: "poly".into() }
    }
    pub fn voices(mut self, n: usize) -> Self {
        self.max = n.max(1);
        self
    }
    pub fn mono(mut self) -> Self {
        self.mode = PolyMode::Mono;
        self
    }
    /// Legato mode with glide time in seconds.
    pub fn legato(mut self, glide: f32) -> Self {
        self.mode = PolyMode::Legato;
        self.glide = glide;
        self
    }
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
    /// Access the prototype voice (e.g. to tweak before rendering).
    pub fn proto_mut(&mut self) -> &mut V {
        &mut self.proto
    }
    fn new_slot(&mut self) -> usize {
        let mut v = self.proto.clone();
        v.prepare(self.sr);
        self.slots.push(Slot { voice: v, id: u64::MAX, held: false, alive: false, age: 0 });
        self.slots.len() - 1
    }
    fn alloc(&mut self) -> usize {
        if let Some(i) = self.slots.iter().position(|s| !s.alive) {
            return i;
        }
        if self.slots.len() < self.max {
            return self.new_slot();
        }
        // steal: oldest released voice, else oldest held voice
        self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.held)
            .min_by_key(|(_, s)| s.age)
            .or_else(|| self.slots.iter().enumerate().min_by_key(|(_, s)| s.age))
            .map(|(i, _)| i)
            .unwrap_or(0)
    }
}

impl<V: Voice> Instrument for Poly<V> {
    fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        self.proto.prepare(sr);
        for s in &mut self.slots {
            s.voice.prepare(sr);
        }
        // pre-create voices so note-ons never allocate
        let want = if self.mode == PolyMode::Poly { self.max.min(64) } else { 1 };
        while self.slots.len() < want {
            self.new_slot();
        }
    }
    fn note_on(&mut self, note: &NoteOn, ctx: &Ctx) {
        self.counter += 1;
        match self.mode {
            PolyMode::Poly => {
                let i = self.alloc();
                let s = &mut self.slots[i];
                s.voice.start(note, ctx);
                s.id = note.id;
                s.held = true;
                s.alive = true;
                s.age = self.counter;
            }
            PolyMode::Mono | PolyMode::Legato => {
                if self.slots.is_empty() {
                    self.new_slot();
                }
                let glide_into = self.mode == PolyMode::Legato && !self.held.is_empty() && self.slots[0].alive;
                self.held.push(*note);
                let s = &mut self.slots[0];
                if glide_into {
                    s.voice.legato(note, self.glide, ctx);
                } else {
                    s.voice.start(note, ctx);
                }
                s.id = note.id;
                s.held = true;
                s.alive = true;
                s.age = self.counter;
            }
        }
    }
    fn note_off(&mut self, id: u64, ctx: &Ctx) {
        match self.mode {
            PolyMode::Poly => {
                for s in &mut self.slots {
                    if s.id == id && s.held {
                        s.held = false;
                        s.voice.release(ctx);
                    }
                }
            }
            PolyMode::Mono | PolyMode::Legato => {
                let was_current = self.held.last().map(|n| n.id) == Some(id);
                self.held.retain(|n| n.id != id);
                if self.slots.is_empty() {
                    return;
                }
                if was_current {
                    if let Some(prev) = self.held.last().copied() {
                        // fall back to the still-held previous note
                        let s = &mut self.slots[0];
                        if self.mode == PolyMode::Legato {
                            s.voice.legato(&prev, self.glide, ctx);
                        } else {
                            s.voice.start(&prev, ctx);
                        }
                        s.id = prev.id;
                    } else {
                        let s = &mut self.slots[0];
                        s.held = false;
                        s.voice.release(ctx);
                    }
                }
            }
        }
    }
    fn render(&mut self, out: &mut [Frame], ctx: &Ctx) {
        for s in &mut self.slots {
            if s.alive {
                s.alive = s.voice.render(out, ctx);
                if !s.alive {
                    s.held = false;
                }
            }
        }
    }
    fn active(&self) -> bool {
        self.slots.iter().any(|s| s.alive)
    }
    fn panic(&mut self) {
        self.slots.clear();
        self.held.clear();
    }
    fn name(&self) -> String {
        self.name.clone()
    }
}

/// Stack two instruments: `Layer(a, b)` plays both on every note.
pub struct Layer<A: Instrument, B: Instrument>(pub A, pub B);

impl<A: Instrument, B: Instrument> Instrument for Layer<A, B> {
    fn prepare(&mut self, sr: f32) {
        self.0.prepare(sr);
        self.1.prepare(sr);
    }
    fn preload(&mut self, notes: &[crate::clip::Note]) {
        self.0.preload(notes);
        self.1.preload(notes);
    }
    fn note_on(&mut self, note: &NoteOn, ctx: &Ctx) {
        self.0.note_on(note, ctx);
        self.1.note_on(note, ctx);
    }
    fn note_off(&mut self, id: u64, ctx: &Ctx) {
        self.0.note_off(id, ctx);
        self.1.note_off(id, ctx);
    }
    fn render(&mut self, out: &mut [Frame], ctx: &Ctx) {
        self.0.render(out, ctx);
        self.1.render(out, ctx);
    }
    fn active(&self) -> bool {
        self.0.active() || self.1.active()
    }
    fn panic(&mut self) {
        self.0.panic();
        self.1.panic();
    }
    fn name(&self) -> String {
        format!("{} + {}", self.0.name(), self.1.name())
    }
}

/// An instrument that makes no sound (useful for buses or MIDI-only tracks).
pub struct Silence;
impl Instrument for Silence {
    fn note_on(&mut self, _: &NoteOn, _: &Ctx) {}
    fn note_off(&mut self, _: u64, _: &Ctx) {}
    fn render(&mut self, _: &mut [Frame], _: &Ctx) {}
    fn active(&self) -> bool {
        false
    }
    fn name(&self) -> String {
        "silence".into()
    }
}
