//! Presets built from the sample library (`assets/library`, see [`super::library`]).

use super::kit::{SampleKit, gm};
use super::library::{Library, Patch};
use super::sampler::Sampler;
use crate::units::Units;

fn patch(id: &str) -> Patch {
    Library::patch(id).unwrap_or_else(|e| panic!("uwu-wave: {e}"))
}

/// A sampler from a patch, keeping only zones accepted by `keep`.
fn sampler_where(p: &Patch, mut keep: impl FnMut(&super::sampler::Zone) -> bool) -> Sampler {
    let mut q = p.clone();
    q.zones.retain(|z| keep(z));
    q.sampler()
}

impl Sampler {
    /// Acoustic grand piano, A0–C8 (`keys/piano`). Black keys have two recordings that
    /// alternate round-robin. Gated with a damper-like release; soft notes are darker.
    pub fn piano() -> Sampler {
        Sampler::library("keys/piano")
            .trim(true)
            .named("piano")
            .release(0.35)
            .velocity_curve(1.6)
            .tone(0.55)
            .gain(0.6)
            .voices(48)
    }

    /// Electric bass guitar, D1–G4, fingered, no effects (`plucked/bass-guitar`, drop-D).
    pub fn bass_guitar() -> Sampler {
        Sampler::library("plucked/bass-guitar")
            .trim(true)
            .named("bass guitar")
            .release(0.07)
            .velocity_curve(1.2)
            .normalize(0.6)
            .voices(8)
    }

    /// Clean Stratocaster, one take per note, D2–C#6 (`plucked/electric-guitar`).
    pub fn guitar() -> Sampler {
        Sampler::library("plucked/electric-guitar")
            .trim(true)
            .named("guitar")
            .release(0.12)
            .normalize(0.5)
            .voices(16)
    }

    /// Parker Fly guitar with 4 takes per note alternating round-robin (only the notes you play
    /// are downloaded). Notes playable on several strings use all of them.
    pub fn guitar_fly() -> Sampler {
        // takes 1-4 keep the round-robin varied without fetching every recording
        takes(&patch("plucked/electric-guitar-fly"), 4)
            .trim(true)
            .named("guitar (parker fly)")
            .release(0.12)
            .normalize(0.5)
            .voices(16)
    }

    /// Celtic harp, F2–G6, up to 4 alternating takes per note (`plucked/celtic-harp`). Notes
    /// ring on after release like real strings.
    pub fn harp() -> Sampler {
        takes(&patch("plucked/celtic-harp"), 4)
            .trim(true)
            .named("harp")
            .release(1.2)
            .tone(0.4)
            .normalize(0.5)
            .voices(32)
    }

    /// Zither, E3–C7, up to 4 alternating takes per note (`plucked/zither`).
    pub fn zither() -> Sampler {
        takes(&patch("plucked/zither"), 4)
            .trim(true)
            .named("zither")
            .release(1.0)
            .tone(0.3)
            .normalize(0.5)
            .voices(32)
    }
}

/// Keep round-robin takes 1..=n of a patch.
fn takes(p: &Patch, n: u32) -> Sampler {
    let keep: Vec<bool> = p.manifest.zones.iter().map(|z| z.rr <= n).collect();
    let mut i = 0;
    sampler_where(p, |_| {
        i += 1;
        keep[i - 1]
    })
}

impl SampleKit {
    /// Acoustic drum kit (`drums/acoustic-kit`), GM mapped: 35/36 kicks, 37 side stick, 38/40
    /// snares, 39 snap, 42/44/46 hats (choked), 41/43/45/47/48/50 toms, 49/57 crash, 51 ride,
    /// 54 tap.
    pub fn acoustic() -> SampleKit {
        SampleKit::library("drums/acoustic-kit")
            .named("acoustic kit")
            .choke(&[gm::HAT_CLOSED, gm::HAT_PEDAL, gm::HAT_OPEN])
            .pan(gm::HAT_CLOSED, 0.3)
            .pan(gm::HAT_PEDAL, 0.3)
            .pan(gm::HAT_OPEN, 0.3)
            .pan(gm::TOM_HIGH, 0.35)
            .pan(gm::TOM_HIGH_MID, 0.2)
            .pan(gm::TOM_LOW_MID, 0.05)
            .pan(gm::TOM_LOW, -0.1)
            .pan(gm::TOM_HIGH_FLOOR, -0.25)
            .pan(gm::TOM_LOW_FLOOR, -0.35)
            .pan(gm::CRASH, -0.35)
            .pan(gm::CRASH_2, 0.4)
            .pan(gm::RIDE, -0.3)
            .decay(gm::CRASH, 3.0)
            .decay(gm::CRASH_2, 3.0)
            .decay(gm::RIDE, 3.5)
            .level(gm::RIDE, 4.0.db())
            .level(gm::CRASH, -2.0.db())
            .level(gm::TAMBOURINE, 2.0.db())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::Ctx;
    use crate::dsp::Frame;
    use crate::instrument::{Instrument, NoteOn};

    #[test]
    fn kit_from_library() {
        if Library::patch("drums/acoustic-kit").is_err() {
            return;
        }
        let k = SampleKit::acoustic();
        assert!(
            k.pitches().contains(&36) && k.pitches().contains(&57),
            "{:?}",
            k.pitches()
        );
    }

    #[test]
    fn piano_preset_maps_88_keys_and_plays() {
        if Library::patch("keys/piano").is_err() {
            eprintln!("library not found, skipping");
            return;
        }
        let mut p = Sampler::piano();
        let mut keys: Vec<i32> = p.zones().iter().map(|z| z.root as i32).collect();
        keys.dedup();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), 88);
        assert_eq!((keys[0], keys[87]), (21, 108));
        let sr = 48000.0;
        let ctx = Ctx::offline(sr);
        p.prepare(sr);
        for (i, k) in [48.0, 60.0, 64.0, 67.0].iter().enumerate() {
            p.note_on(
                &NoteOn {
                    id: i as u64,
                    pitch: *k,
                    vel: 0.8,
                    artic: Default::default(),
                    dur: 1.0,
                },
                &ctx,
            );
        }
        let mut peak = 0f32;
        for b in 0..200 {
            let mut buf = [Frame::ZERO; 64];
            p.render(&mut buf, &ctx.advanced(b * 64));
            for f in buf {
                assert!(f.l.is_finite() && f.r.is_finite());
                peak = peak.max(f.peak());
            }
        }
        assert!(peak > 0.05 && peak < 2.0, "{peak}");
    }
}
