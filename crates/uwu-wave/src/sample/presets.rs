//! Presets built from the bundled assets.

use super::kit::{SampleKit, gm};
use super::load::{find_dir, note_in_name, piano_key, wav_files};
use super::sampler::{Sampler, Src, Zone};
use crate::units::Units;
use std::path::PathBuf;

fn dir(rel: &str) -> PathBuf {
    find_dir(rel.as_ref()).unwrap_or_else(|e| panic!("uwu-wave: {e}"))
}

/// Zones for every wav in `dirs` with a note name in its file name (`keep` filters by file
/// name, `tune` gives a per-file tuning correction in semitones).
fn zones_from(
    s: Sampler,
    dirs: &[PathBuf],
    keep: impl Fn(&str) -> bool,
    tune: impl Fn(&str) -> f32,
) -> Sampler {
    let mut s = s;
    for d in dirs {
        for f in wav_files(d).unwrap_or_else(|e| panic!("uwu-wave: {}: {e}", d.display())) {
            let name = f
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !keep(&name) {
                continue;
            }
            if let Some(p) = note_in_name(&name) {
                s = s.zone(Zone::with_src(Src::path(f), p).tune(tune(&name)));
            }
        }
    }
    assert!(
        !s.zones().is_empty(),
        "uwu-wave: no samples found in {dirs:?}"
    );
    s
}

impl Sampler {
    /// Acoustic grand piano, A0–C8 (`assets/piano`, Helmholtz-named files, see
    /// [`piano_key`](super::piano_key)). Black keys have two recordings (e.g. `Cs`/`Df`) that
    /// alternate round-robin. Gated with a damper-like release; soft notes are darker.
    pub fn piano() -> Sampler {
        let d = dir("piano");
        let mut s = Sampler::new().trim(true).named("piano");
        for f in wav_files(&d).unwrap_or_else(|e| panic!("uwu-wave: {}: {e}", d.display())) {
            let name = f
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            // cc1/dd1/ee1(+s/f) are byte-identical duplicates of c1/d1/e1
            let b = name.as_bytes();
            if b.len() > 2 && b[0] == b[1] && b[0].is_ascii_lowercase() && b[2] == b'1' {
                continue;
            }
            if let Some(k) = piano_key(&name) {
                s = s.zone(Zone::with_src(Src::path(f), k as f32));
            }
        }
        assert!(
            !s.zones().is_empty(),
            "uwu-wave: no piano samples in {}",
            d.display()
        );
        s.release(0.35)
            .velocity_curve(1.6)
            .tone(0.55)
            .gain(0.6)
            .voices(48)
    }

    /// Electric bass guitar, D1–G4, fingered, no effects (`wavebase/base/bass`, drop-D).
    pub fn bass_guitar() -> Sampler {
        zones_from(
            Sampler::new().trim(true).named("bass guitar"),
            &[dir("wavebase/base/bass")],
            |_| true,
            // measured: the low strings of this recording are slightly out of tune
            |n| match n.get(..8) {
                Some("string-4") => -0.15,
                Some("string-3") => 0.12,
                Some("string-2") => 0.03,
                _ => 0.0,
            },
        )
        .release(0.07)
        .velocity_curve(1.2)
        .normalize(0.6)
        .voices(8)
    }

    /// Clean Stratocaster, one take per note, D2–C#6 (`wavebase/base/guitar/stratocaster`).
    pub fn guitar() -> Sampler {
        zones_from(
            Sampler::new().trim(true).named("guitar"),
            &[dir("wavebase/base/guitar/stratocaster")],
            |_| true,
            |_| 0.0,
        )
        .release(0.12)
        .normalize(0.5)
        .voices(16)
    }

    /// Parker Fly guitar with up to 8 takes per note alternating round-robin (only the notes you
    /// play are downloaded — each take is ~2 MB). Notes playable on several strings use all of them.
    pub fn guitar_fly() -> Sampler {
        let base = dir("wavebase/base/guitar/parker-fly");
        let dirs: Vec<PathBuf> = (1..=6)
            .map(|i| base.join(format!("string-{i}")))
            .filter(|d| d.is_dir())
            .collect();
        // takes 1-4 keep the round-robin varied without fetching every recording
        zones_from(
            Sampler::new().trim(true).named("guitar (parker fly)"),
            &dirs,
            |n| {
                n.rsplit_once('-')
                    .map(|(_, t)| matches!(t, "1.wav" | "2.wav" | "3.wav" | "4.wav"))
                    .unwrap_or(false)
            },
            |_| 0.0,
        )
        .release(0.12)
        .normalize(0.5)
        .voices(16)
    }

    /// Celtic harp, F2–G6, with up to 4 alternating takes per note (`wavebase/base/harp/celtic`).
    /// Notes ring on after release like real strings.
    pub fn harp() -> Sampler {
        zones_from(
            Sampler::new().trim(true).named("harp"),
            &[dir("wavebase/base/harp/celtic")],
            |n| {
                matches!(
                    n.rsplit_once("take-").map(|(_, t)| t),
                    Some("1.wav" | "2.wav" | "3.wav" | "4.wav")
                )
            },
            // the celtic harp recordings sit ~14 cents sharp of A440
            |_| -0.14,
        )
        .release(1.2)
        .tone(0.4)
        .normalize(0.5)
        .voices(32)
    }

    /// Zither, E3–C7, up to 4 alternating takes per note (`wavebase/base/harp/zither`).
    pub fn zither() -> Sampler {
        zones_from(
            Sampler::new().trim(true).named("zither"),
            &[dir("wavebase/base/harp/zither")],
            |n| {
                matches!(
                    n.rsplit_once("take-").map(|(_, t)| t),
                    Some("1.wav" | "2.wav" | "3.wav" | "4.wav")
                )
            },
            |_| 0.0,
        )
        .release(1.0)
        .tone(0.3)
        .normalize(0.5)
        .voices(32)
    }
}

impl SampleKit {
    /// Acoustic drum kit from `wavebase/base/drum/*.wav`, GM mapped: 35/36 kicks, 37 side
    /// stick, 38/40 snares, 39 snap, 42/44/46 hats (choked), 41/43/45/47/48/50 toms, 49/57 crash,
    /// 51 ride, 54 tap.
    pub fn acoustic() -> SampleKit {
        let d = dir("wavebase/base/drum");
        let f = |n: &str| d.join(n);
        SampleKit::new()
            .named("acoustic kit")
            .pad(gm::KICK, f("kick.wav"))
            .pad(gm::KICK_2, f("kick-2.wav"))
            .pad(gm::RIM, f("stick.wav"))
            .pad(gm::SNARE, f("snare.wav"))
            .pad(gm::SNARE_2, f("snare-2.wav"))
            .pad(gm::CLAP, f("snap.wav"))
            .pad(gm::HAT_CLOSED, f("hat-closed.wav"))
            .pad(gm::HAT_PEDAL, f("hat-closed-4.wav"))
            .pad(gm::HAT_OPEN, f("hat-opened.wav"))
            .pad(gm::TOM_LOW_FLOOR, f("tom.wav"))
            .derive(gm::TOM_HIGH_FLOOR, gm::TOM_LOW_FLOOR, 2.0)
            .pad(gm::TOM_LOW, f("tom-2.wav"))
            .derive(gm::TOM_LOW_MID, gm::TOM_LOW, 2.0)
            .derive(gm::TOM_HIGH_MID, gm::TOM_LOW, 4.0)
            .derive(gm::TOM_HIGH, gm::TOM_LOW, 7.0)
            .pad(gm::CRASH, f("crash.wav"))
            .derive(gm::CRASH_2, gm::CRASH, 1.5)
            .pad(gm::RIDE, f("ride.wav"))
            .pad(gm::TAMBOURINE, f("tap.wav"))
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
    fn piano_preset_maps_88_keys_and_plays() {
        if crate::sample::assets_dir()
            .map(|d| !d.join("piano").is_dir())
            .unwrap_or(true)
        {
            eprintln!("assets/piano not found, skipping");
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
