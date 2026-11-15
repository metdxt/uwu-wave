//! Drum-machine grids: `grid(pitch, "x..x")` and the `drums!` macro.

use super::{DEFAULT_VEL, Pattern, PatternError};
use crate::theory::Pitch;

/// Accent / ghost velocities used by grids.
pub const ACCENT_VEL: f32 = 1.0;
pub const GHOST_VEL: f32 = 0.45;

/// One drum (or any pitch) from a step grid: `x` hit, `X` accent, `o` ghost, `1`..`9` velocity
/// (n/9), `.` `-` `~` rest, `_` hold the previous hit one more step, `|` starts the next cycle.
/// Whitespace is ignored; each cycle's steps divide it equally.
///
/// ```
/// use uwu_wave::prelude::*;
/// let kick = grid("bd", "x..x ..x. | x... x.x.");   // two cycles
/// let bass = grid(C2, "x.x_ ..xo");
/// assert_eq!(kick.cycles(), 2.0);
/// # let _ = bass;
/// ```
#[track_caller]
pub fn grid(pitch: impl Into<Pitch>, steps: &str) -> Pattern {
    parse_grid(pitch.into().0, steps).unwrap_or_else(|e| panic!("{e}"))
}

/// Several grid rows stacked; row names are GM drum names, note names or MIDI numbers.
/// This is what [`drums!`](crate::drums) expands to.
#[track_caller]
pub fn drum_grid(rows: &[(&str, &str)]) -> Pattern {
    let mut layers = vec![];
    for (name, steps) in rows {
        let pitch = match Pitch::parse(name) {
            Ok(p) => p,
            Err(e) => panic!("drums!: row {name:?}: {e}"),
        };
        match parse_grid(pitch.0, steps) {
            Ok(p) => layers.push(p),
            Err(e) => panic!("drums!: row {name:?}: {e}"),
        }
    }
    Pattern::layers(layers)
}

pub(crate) fn parse_grid(pitch: f32, src: &str) -> Result<Pattern, PatternError> {
    let err = |pos: usize, msg: &str| PatternError {
        message: msg.to_string(),
        source: src.to_string(),
        col: pos + 1,
        len: 1,
    };
    let mut cycles: Vec<Vec<(Option<f32>, f64)>> = vec![vec![]];
    let mut cycle_starts = vec![0usize];
    for (i, c) in src.chars().enumerate() {
        let cur = cycles.last_mut().unwrap();
        let vel = match c {
            c if c.is_whitespace() => continue,
            '|' => {
                cycles.push(vec![]);
                cycle_starts.push(i);
                continue;
            }
            'x' => Some(DEFAULT_VEL),
            'X' => Some(ACCENT_VEL),
            'o' | 'O' => Some(GHOST_VEL),
            '1'..='9' => Some(c.to_digit(10).unwrap() as f32 / 9.0),
            '.' | '-' | '~' | '0' => None,
            '_' => {
                match cur.last_mut() {
                    Some(s) => s.1 += 1.0,
                    None => {
                        return Err(err(
                            i,
                            "'_' (hold) needs a step before it in the same cycle",
                        ));
                    }
                }
                continue;
            }
            _ => {
                return Err(err(
                    i,
                    &format!(
                        "unexpected {c:?} in grid (use x X o 1-9 for hits, . - ~ for rests, _ to hold, | for the next cycle)"
                    ),
                ));
            }
        };
        cur.push((vel, 1.0));
    }
    let mut pats = vec![];
    for (k, cyc) in cycles.iter().enumerate() {
        if cyc.is_empty() {
            return Err(err(cycle_starts[k], "empty grid cycle"));
        }
        // a held hit lasts for its weight; a held rest is just a longer rest
        let items = cyc
            .iter()
            .map(|&(v, w)| {
                (
                    v.map(|v| Pattern::atom(pitch, v))
                        .unwrap_or_else(Pattern::silence),
                    w,
                )
            })
            .collect();
        pats.push(Pattern::fastcat_w(items));
    }
    Ok(if pats.len() == 1 {
        pats.pop().unwrap()
    } else {
        Pattern::concat(pats)
    })
}
