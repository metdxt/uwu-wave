//! Standard MIDI File export (format 1, no dependencies).
//!
//! ```no_run
//! # use uwu_wave::prelude::*;
//! # let song = Song::new("x");
//! uwu_wave::io::midi::write_midi(&song, "out/x.mid").unwrap();
//! ```
//! Or from the command line: `cargo run --example my_song -- --midi out/my_song.mid`.

use crate::Error;
use crate::param::Clock;
use crate::song::{ChanKind, Song};
use std::path::Path;

/// Ticks per quarter note.
pub const PPQ: u32 = 480;

/// Is this track (by name) a drum track? Drum tracks go to MIDI channel 10.
pub fn is_drum_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "drum", "kit", "perc", "beat", "kick", "snare", "hat", "hh", "clap", "808", "909", "tom",
        "cymbal", "ride", "crash",
    ]
    .iter()
    .any(|k| n.contains(k))
}

/// The tempo as `(beat, bpm)` change points, sampled once per beat (a single point when the
/// tempo is constant). Also returns the song length in seconds.
pub fn tempo_map(song: &Song, end_beat: f64) -> (Vec<(f64, f64)>, f64) {
    let tempo = song.tempo_param();
    if let Some(b) = tempo.as_const() {
        let b = (b as f64).max(1.0);
        return (vec![(0.0, b)], end_beat * 60.0 / b);
    }
    let mut pts: Vec<(f64, f64)> = vec![];
    let mut secs = 0.0;
    let mut beat = 0.0;
    let step: f64 = 1.0 / 16.0;
    while beat < end_beat.max(0.0) + 1e-9 {
        let bpm = (tempo.at(Clock { beat, secs }) as f64).max(1.0);
        if beat.fract().abs() < 1e-9 && pts.last().map(|p| (p.1 - bpm).abs() > 0.01).unwrap_or(true)
        {
            pts.push((beat, bpm));
        }
        let db = step.min(end_beat - beat);
        if db <= 0.0 {
            break;
        }
        secs += db * 60.0 / bpm;
        beat += db;
        beat = (beat * 16.0).round() / 16.0;
    }
    if pts.is_empty() {
        pts.push((0.0, 120.0));
    }
    (pts, secs)
}

fn vlq(out: &mut Vec<u8>, mut v: u32) {
    let mut buf = [0u8; 5];
    let mut i = 4;
    buf[i] = (v & 0x7f) as u8;
    v >>= 7;
    while v > 0 {
        i -= 1;
        buf[i] = 0x80 | (v & 0x7f) as u8;
        v >>= 7;
    }
    out.extend_from_slice(&buf[i..]);
}

/// Absolute-tick events → an `MTrk` chunk. Events at the same tick keep their order.
fn chunk(mut evs: Vec<(u64, u8, Vec<u8>)>) -> Vec<u8> {
    evs.sort_by_key(|e| (e.0, e.1));
    let mut body = vec![];
    let mut t = 0u64;
    for (tick, _, bytes) in evs {
        vlq(&mut body, (tick - t).min(u32::MAX as u64 >> 4) as u32);
        t = tick;
        body.extend_from_slice(&bytes);
    }
    vlq(&mut body, 0);
    body.extend_from_slice(&[0xff, 0x2f, 0x00]);
    let mut out = b"MTrk".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

fn meta_text(kind: u8, s: &str) -> Vec<u8> {
    let mut v = vec![0xff, kind];
    vlq(&mut v, s.len() as u32);
    v.extend_from_slice(s.as_bytes());
    v
}

fn tick(beat: f64) -> u64 {
    (beat.max(0.0) * PPQ as f64).round() as u64
}

/// Encode the song as a format-1 Standard MIDI File: a conductor track (name, meter, tempo
/// map) plus one track per instrument track with its notes (fractional pitches are rounded;
/// drum-named tracks use channel 10).
pub fn midi_bytes(song: &Song) -> Vec<u8> {
    let end = song.end().0;
    let (num, den) = song.meter_sig();
    let mut tracks = vec![];

    // conductor
    let mut ev = vec![(0, 0, meta_text(0x03, &song.name()))];
    let dd = (den.max(1) as f32).log2().round() as u8;
    ev.push((
        0,
        1,
        vec![0xff, 0x58, 0x04, num.clamp(1, 255) as u8, dd, 24, 8],
    ));
    let (tm, _) = tempo_map(song, end);
    for (b, bpm) in tm {
        let us = (60_000_000.0 / bpm).round().clamp(1.0, 16_777_215.0) as u32;
        ev.push((
            tick(b),
            2,
            vec![
                0xff,
                0x51,
                0x03,
                (us >> 16) as u8,
                (us >> 8) as u8,
                us as u8,
            ],
        ));
    }
    for (name, t) in song.markers() {
        ev.push((tick(t.0), 3, meta_text(0x06, &name)));
    }
    tracks.push(chunk(ev));

    // instrument tracks
    let mut next_ch = 0u8;
    for t in song
        .tracks()
        .into_iter()
        .filter(|t| t.kind() == ChanKind::Track)
    {
        let name = t.name();
        let ch = if is_drum_name(&name) {
            9
        } else {
            let c = next_ch;
            next_ch = (next_ch + 1) % 16;
            if next_ch == 9 {
                next_ch = 10;
            }
            c
        };
        let mut ev = vec![(0, 0, meta_text(0x03, &name))];
        for n in t.notes() {
            let p = n.pitch.round();
            if !(0.0..=127.0).contains(&p) {
                continue;
            }
            let vel = (n.vel * 127.0).round().clamp(1.0, 127.0) as u8;
            let (a, b) = (tick(n.start), tick(n.start + n.dur).max(tick(n.start) + 1));
            // note-offs sort before note-ons at the same tick
            ev.push((a, 2, vec![0x90 | ch, p as u8, vel]));
            ev.push((b, 1, vec![0x80 | ch, p as u8, 64]));
        }
        tracks.push(chunk(ev));
    }

    let mut out = b"MThd".to_vec();
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&(tracks.len() as u16).to_be_bytes());
    out.extend_from_slice(&(PPQ as u16).to_be_bytes());
    for t in tracks {
        out.extend_from_slice(&t);
    }
    out
}

/// Write the song's notes as a Standard MIDI File.
pub fn write_midi(song: &Song, path: impl AsRef<Path>) -> Result<(), Error> {
    if let Some(dir) = path.as_ref().parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    std::fs::write(path, midi_bytes(song))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::Clip;
    use crate::instrument::Silence;
    use crate::units::Units;

    #[test]
    fn vlq_encoding() {
        let enc = |v| {
            let mut o = vec![];
            vlq(&mut o, v);
            o
        };
        assert_eq!(enc(0), vec![0]);
        assert_eq!(enc(0x7f), vec![0x7f]);
        assert_eq!(enc(0x80), vec![0x81, 0x00]);
        assert_eq!(enc(0x3fff), vec![0xff, 0x7f]);
        assert_eq!(enc(0x0fff_ffff), vec![0xff, 0xff, 0xff, 0x7f]);
    }

    #[test]
    fn writes_tracks_and_drum_channel() {
        let song = Song::new("midi").bpm(90);
        let keys = song.track("keys", Silence);
        let drums = song.track("drums", Silence);
        song.section("a", 1.bars(), |s| {
            s.once(
                &keys,
                Clip::new(4.beats()).note(0, 1, 60, 1.0).note(1, 1, 64, 0.5),
            );
            s.once(&drums, Clip::new(4.beats()).note(0, 0.25, 36, 1.0));
        });
        let b = midi_bytes(&song);
        assert_eq!(&b[..4], b"MThd");
        assert_eq!(u16::from_be_bytes([b[10], b[11]]), 3); // conductor + 2
        // tempo 90 bpm = 666667 us
        let us = 666_667u32.to_be_bytes();
        assert!(
            b.windows(6)
                .any(|w| w == [0xff, 0x51, 0x03, us[1], us[2], us[3]])
        );
        assert!(b.windows(3).any(|w| w == [0x90, 60, 127]));
        assert!(b.windows(3).any(|w| w == [0x90, 64, 64]));
        assert!(
            b.windows(3).any(|w| w == [0x99, 36, 127]),
            "drums on channel 10"
        );
        // chunk lengths are consistent
        let mut i = 14;
        let mut n = 0;
        while i < b.len() {
            assert_eq!(&b[i..i + 4], b"MTrk");
            let len = u32::from_be_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]) as usize;
            assert_eq!(&b[i + 8 + len - 3..i + 8 + len], &[0xff, 0x2f, 0x00]);
            i += 8 + len;
            n += 1;
        }
        assert_eq!(n, 3);
    }

    #[test]
    fn tempo_automation_is_sampled() {
        let song = Song::new("t").bpm(crate::auto![0 => 100, 8.beats() => 140]);
        let (m, secs) = tempo_map(&song, 8.0);
        assert!(m.len() >= 8, "{m:?}");
        assert!((m[0].1 - 100.0).abs() < 0.01);
        assert!(secs > 8.0 * 60.0 / 140.0 && secs < 8.0 * 60.0 / 100.0);
    }
}
