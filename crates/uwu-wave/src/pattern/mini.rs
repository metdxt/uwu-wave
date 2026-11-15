//! Mini-notation parser (see the module docs of [`crate::pattern`]).

use super::rat::Rat;
use super::{DEFAULT_VEL, Pattern};
use crate::theory::{Chord, Pitch, gm_drum, midi_of, parse_note_prefix};
use std::fmt;

/// A mini-notation parse error, with the 1-based column and a caret-pointed rendering:
///
/// ```text
/// pattern error at column 7: unknown note or drum name "x4"
///   c4 e4 x4
///         ^^
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct PatternError {
    pub message: String,
    pub source: String,
    /// 1-based column (in characters).
    pub col: usize,
    /// Length of the offending span (in characters, ≥ 1).
    pub len: usize,
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "pattern error at column {}: {}", self.col, self.message)?;
        writeln!(f, "  {}", self.source)?;
        write!(
            f,
            "  {}{}",
            " ".repeat(self.col.saturating_sub(1)),
            "^".repeat(self.len.max(1))
        )
    }
}
impl std::error::Error for PatternError {}
impl From<PatternError> for crate::error::Error {
    fn from(e: PatternError) -> crate::error::Error {
        crate::error::Error::Invalid(e.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Mode {
    /// Notes, chords, drums, numbers.
    Notes,
    /// Structure patterns: `x`/`X`/`t`/`1` = hit (pitch 1), `0`/`f` = no hit.
    Bool,
}

#[derive(Clone, Debug)]
enum Node {
    Rest,
    Note(f32),
    Chord(Vec<f32>),
    Seq(Vec<Step>),
    Alt(Vec<Step>),
    Stack(Vec<Node>),
    Poly(Vec<Vec<Step>>, Option<Rat>),
    Fast(Box<Node>, Rat),
    Slow(Box<Node>, Rat),
    Degrade(Box<Node>, f32, u64),
    Euclid(Box<Node>, u32, u32, i32),
    Vel(Box<Node>, f32),
}

#[derive(Clone, Debug)]
struct Step {
    node: Node,
    weight: f64,
}

pub(crate) fn parse(src: &str, mode: Mode) -> Result<Pattern, PatternError> {
    let mut p = Parser {
        src,
        chars: src.chars().collect(),
        pos: 0,
        mode,
    };
    let parts = p.layers(None)?;
    if parts.iter().all(|s| s.is_empty()) {
        return Err(p.err(0, src.chars().count().max(1), "empty pattern"));
    }
    let node = stack_of(parts.into_iter().map(Node::Seq).collect());
    Ok(compile(&node))
}

fn stack_of(mut nodes: Vec<Node>) -> Node {
    if nodes.len() == 1 {
        nodes.pop().unwrap()
    } else {
        Node::Stack(nodes)
    }
}

struct Parser<'a> {
    src: &'a str,
    chars: Vec<char>,
    pos: usize,
    mode: Mode,
}

const OPENERS: [(char, char); 3] = [('[', ']'), ('<', '>'), ('{', '}')];

impl Parser<'_> {
    fn err(&self, pos: usize, len: usize, msg: impl Into<String>) -> PatternError {
        PatternError {
            message: msg.into(),
            source: self.src.to_string(),
            col: pos + 1,
            len: len.max(1),
        }
    }
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }
    fn peek_at(&self, k: usize) -> Option<char> {
        self.chars.get(self.pos + k).copied()
    }
    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|c| c.is_whitespace()) {
            self.pos += 1;
        }
    }

    /// Comma-separated layers of steps until `close` (or end of input when `None`).
    fn layers(&mut self, close: Option<(char, usize)>) -> Result<Vec<Vec<Step>>, PatternError> {
        let mut parts: Vec<Vec<Step>> = vec![vec![]];
        loop {
            self.skip_ws();
            let Some(c) = self.peek() else {
                if let Some((cl, open_pos)) = close {
                    return Err(self.err(
                        open_pos,
                        1,
                        format!("missing closing '{cl}' for this '{}'", self.chars[open_pos]),
                    ));
                }
                break;
            };
            if Some(c) == close.map(|c| c.0) {
                self.pos += 1;
                break;
            }
            match c {
                ',' => {
                    self.pos += 1;
                    parts.push(vec![]);
                }
                '_' => {
                    let cur = parts.last_mut().unwrap();
                    match cur.last_mut() {
                        Some(s) => s.weight += 1.0,
                        None => {
                            return Err(self.err(self.pos, 1, "'_' (tie) needs a step before it"));
                        }
                    }
                    self.pos += 1;
                }
                ']' | '>' | '}' | ')' => {
                    let msg = match close {
                        Some((cl, _)) => format!("unexpected '{c}' (expected '{cl}')"),
                        None => format!("unexpected '{c}' without a matching opening bracket"),
                    };
                    return Err(self.err(self.pos, 1, msg));
                }
                _ => {
                    let steps = self.step()?;
                    parts.last_mut().unwrap().extend(steps);
                }
            }
        }
        Ok(parts)
    }

    fn step(&mut self) -> Result<Vec<Step>, PatternError> {
        let start = self.pos;
        let mut node = self.term()?;
        let mut weight = 1.0;
        let mut reps = 1usize;
        loop {
            let save = self.pos;
            self.skip_ws();
            let Some(c) = self.peek() else { break };
            let op_pos = self.pos;
            match c {
                '*' | '/' => {
                    self.pos += 1;
                    let k = self.rat_arg(op_pos)?;
                    node = if c == '*' {
                        Node::Fast(Box::new(node), k)
                    } else {
                        Node::Slow(Box::new(node), k)
                    };
                }
                '!' => {
                    self.pos += 1;
                    if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                        let (v, p0, l) = self.number()?;
                        if v < 1.0 || v.fract() != 0.0 || v > 1024.0 {
                            return Err(self.err(
                                p0,
                                l,
                                "replicate count must be a whole number ≥ 1",
                            ));
                        }
                        reps += v as usize - 1;
                    } else {
                        reps += 1;
                    }
                }
                '@' => {
                    self.pos += 1;
                    self.skip_ws();
                    let (v, p0, l) = self.number()?;
                    if v <= 0.0 {
                        return Err(self.err(p0, l, "weight must be > 0"));
                    }
                    weight = v;
                }
                '?' => {
                    self.pos += 1;
                    let prob = if self.peek().is_some_and(|c| c.is_ascii_digit() || c == '.') {
                        let (v, p0, l) = self.number()?;
                        if !(0.0..=1.0).contains(&v) {
                            return Err(self.err(
                                p0,
                                l,
                                "drop probability must be between 0 and 1",
                            ));
                        }
                        v
                    } else {
                        0.5
                    };
                    let seed = 0x5EED_0000 ^ (op_pos as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                    node = Node::Degrade(Box::new(node), prob as f32, seed);
                }
                '^' => {
                    self.pos += 1;
                    self.skip_ws();
                    let (v, p0, l) = self.number()?;
                    if !(0.0..=1.0).contains(&v) {
                        return Err(self.err(p0, l, "velocity must be between 0 and 1"));
                    }
                    node = Node::Vel(Box::new(node), v as f32);
                }
                '(' => {
                    self.pos += 1;
                    let mut args = vec![];
                    loop {
                        self.skip_ws();
                        if self.peek() == Some('-')
                            || self.peek().is_some_and(|c| c.is_ascii_digit())
                        {
                            let (v, p0, l) = self.number()?;
                            if v.fract() != 0.0 {
                                return Err(self.err(
                                    p0,
                                    l,
                                    "euclid arguments must be whole numbers",
                                ));
                            }
                            args.push((v as i64, p0, l));
                        } else {
                            return Err(self.err(
                                self.pos,
                                1,
                                "expected a number in euclid arguments, e.g. bd(3,8) or bd(3,8,2)",
                            ));
                        }
                        self.skip_ws();
                        match self.peek() {
                            Some(',') => self.pos += 1,
                            Some(')') => {
                                self.pos += 1;
                                break;
                            }
                            _ => {
                                return Err(self.err(
                                    self.pos,
                                    1,
                                    "expected ',' or ')' in euclid arguments",
                                ));
                            }
                        }
                    }
                    if args.len() < 2 || args.len() > 3 {
                        return Err(self.err(op_pos, self.pos - op_pos, "euclid needs 2 or 3 arguments: (hits, steps) or (hits, steps, rotation)"));
                    }
                    let (k, kp, kl) = args[0];
                    let (n, np, nl) = args[1];
                    if !(1..=256).contains(&n) {
                        return Err(self.err(np, nl, "euclid steps must be between 1 and 256"));
                    }
                    if k < 0 || k > n {
                        return Err(self.err(
                            kp,
                            kl,
                            "euclid hits must be between 0 and the number of steps",
                        ));
                    }
                    let r = args.get(2).map(|a| a.0).unwrap_or(0);
                    node = Node::Euclid(Box::new(node), k as u32, n as u32, r as i32);
                }
                _ => {
                    self.pos = save;
                    break;
                }
            }
        }
        let _ = start;
        Ok(vec![Step { node, weight }; reps])
    }

    fn rat_arg(&mut self, op_pos: usize) -> Result<Rat, PatternError> {
        self.skip_ws();
        if matches!(self.peek(), Some('<' | '[')) {
            return Err(self.err(
                self.pos,
                1,
                "patterned speed factors are not supported; use a number (e.g. *2, /1.5)",
            ));
        }
        if !self.peek().is_some_and(|c| c.is_ascii_digit() || c == '.') {
            return Err(self.err(
                op_pos,
                1,
                format!(
                    "'{}' needs a number after it, e.g. a{}2",
                    self.chars[op_pos], self.chars[op_pos]
                ),
            ));
        }
        let p0 = self.pos;
        let (v, _, l) = self.number()?;
        if v <= 0.0 {
            return Err(self.err(p0, l, "speed factor must be > 0"));
        }
        // exact decimal → rational
        let text: String = self.chars[p0..p0 + l].iter().collect();
        Ok(match text.split_once('.') {
            Some((_, frac)) if !frac.is_empty() => {
                let d = 10u64.pow(frac.len().min(9) as u32);
                Rat::new((v * d as f64).round() as u64, d)
            }
            _ => Rat::from_f64(v),
        })
    }

    /// A number (optional leading '-', digits, optional fraction). Returns (value, pos, len).
    fn number(&mut self) -> Result<(f64, usize, usize), PatternError> {
        let p0 = self.pos;
        if self.peek() == Some('-') {
            self.pos += 1;
        }
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let text: String = self.chars[p0..self.pos].iter().collect();
        match text.parse::<f64>() {
            Ok(v) => Ok((v, p0, self.pos - p0)),
            Err(_) => Err(self.err(p0, (self.pos - p0).max(1), "expected a number")),
        }
    }

    fn term(&mut self) -> Result<Node, PatternError> {
        self.skip_ws();
        let Some(c) = self.peek() else {
            return Err(self.err(self.pos, 1, "unexpected end of pattern"));
        };
        let open_pos = self.pos;
        if let Some(&(_, close)) = OPENERS.iter().find(|(o, _)| *o == c) {
            self.pos += 1;
            let parts = self.layers(Some((close, open_pos)))?;
            if parts.iter().all(|p| p.is_empty()) {
                return Err(self.err(open_pos, self.pos - open_pos, "empty group"));
            }
            let parts: Vec<Vec<Step>> = parts.into_iter().filter(|p| !p.is_empty()).collect();
            return match c {
                '[' => Ok(stack_of(parts.into_iter().map(Node::Seq).collect())),
                '<' => Ok(stack_of(parts.into_iter().map(Node::Alt).collect())),
                _ => {
                    let mut steps = None;
                    if self.peek() == Some('%') {
                        self.pos += 1;
                        steps = Some(self.rat_arg(self.pos - 1)?);
                    }
                    Ok(Node::Poly(parts, steps))
                }
            };
        }
        match c {
            '~' => {
                self.pos += 1;
                Ok(Node::Rest)
            }
            '-' if !self
                .peek_at(1)
                .is_some_and(|c| c.is_ascii_digit() || c == '.') =>
            {
                self.pos += 1;
                Ok(Node::Rest)
            }
            '-' | '.' | '0'..='9' => {
                if c == '.' && !self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
                    return Err(self.err(
                        self.pos,
                        1,
                        "unexpected '.' (use '~' or '-' for a rest)",
                    ));
                }
                let (v, p0, l) = self.number()?;
                if self.peek().is_some_and(|c| c.is_alphabetic()) {
                    return Err(self.err(p0, l + 1, "unexpected letter after a number"));
                }
                Ok(match self.mode {
                    Mode::Notes => Node::Note(v as f32),
                    Mode::Bool => {
                        if v != 0.0 {
                            Node::Note(1.0)
                        } else {
                            Node::Rest
                        }
                    }
                })
            }
            c if c.is_alphabetic() => {
                let p0 = self.pos;
                while let Some(c) = self.peek() {
                    let word_char = c.is_alphanumeric()
                        || matches!(c, '#' | '\'' | '+' | '°' | 'ø' | 'Δ' | '♯' | '♭');
                    let slash_chord =
                        c == '/' && self.peek_at(1).is_some_and(|c| c.is_alphabetic());
                    if word_char || slash_chord {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                let word: String = self.chars[p0..self.pos].iter().collect();
                self.atom(&word).map_err(|m| self.err(p0, self.pos - p0, m))
            }
            _ => Err(self.err(self.pos, 1, format!("unexpected '{c}'"))),
        }
    }

    fn atom(&self, w: &str) -> Result<Node, String> {
        if self.mode == Mode::Bool {
            return match w {
                "x" | "X" | "t" | "true" => Ok(Node::Note(1.0)),
                "f" | "false" => Ok(Node::Rest),
                _ => Err(format!(
                    "unknown structure value {w:?} (use x or 1 for a hit, ~ or 0 for none)"
                )),
            };
        }
        resolve_word(w)
    }
}

/// A note/chord/drum word → node.
fn resolve_word(w: &str) -> Result<Node, String> {
    if let Some(p) = gm_drum(w) {
        return Ok(Node::Note(p.0));
    }
    let bad = || {
        format!(
            "unknown note or drum name {w:?} (try c4, eb3, cs4, bd, sd, hh, or a chord like Cmaj7 / c3'm7)"
        )
    };
    let Some((semi, oct, rest)) = parse_note_prefix(w, true) else {
        return Err(bad());
    };
    let root = Pitch(midi_of(semi, oct.unwrap_or(4)));
    if rest.is_empty() {
        return Ok(Node::Note(root.0));
    }
    // chord: optional apostrophe, quality, optional /bass
    let q = rest.strip_prefix('\'').unwrap_or(rest);
    if q.contains('\'') {
        return Err(bad());
    }
    let sym = format!("{}'{}", root_name(semi, oct.unwrap_or(4)), q);
    match Chord::parse(&sym) {
        Ok(c) => Ok(Node::Chord(c.pitches())),
        Err(e) => {
            if rest.starts_with('\'') || oct.is_none() {
                Err(format!("{e}"))
            } else {
                Err(bad())
            }
        }
    }
}

fn root_name(semi: i32, oct: i32) -> String {
    // normalise into a plain sharp name + octave that Chord::parse accepts
    let midi = midi_of(semi, oct) as i32;
    const N: [&str; 12] = [
        "c", "c#", "d", "d#", "e", "f", "f#", "g", "g#", "a", "a#", "b",
    ];
    format!(
        "{}{}",
        N[midi.rem_euclid(12) as usize],
        midi.div_euclid(12) - 1
    )
}

/// Bjorklund euclidean rhythm: `k` hits spread over `n` steps, rotated left by `r`
/// ((3,8) → `x..x..x.`, (5,8) → `x.xx.xx.`).
pub(crate) fn euclid_bools(k: u32, n: u32, r: i32) -> Vec<bool> {
    let n = n.max(1) as usize;
    let k = (k as usize).min(n);
    let mut a: Vec<Vec<bool>> = vec![vec![true]; k];
    let mut b: Vec<Vec<bool>> = vec![vec![false]; n - k];
    while b.len() > 1 && !a.is_empty() {
        let m = a.len().min(b.len());
        let new_a: Vec<Vec<bool>> = (0..m)
            .map(|i| a[i].iter().chain(&b[i]).copied().collect())
            .collect();
        let rest: Vec<Vec<bool>> = if a.len() > m {
            a[m..].to_vec()
        } else {
            b[m..].to_vec()
        };
        a = new_a;
        b = rest;
    }
    let mut v: Vec<bool> = a.into_iter().chain(b).flatten().collect();
    let rot = (r as i64).rem_euclid(n as i64) as usize;
    v.rotate_left(rot);
    v
}

fn compile(node: &Node) -> Pattern {
    match node {
        Node::Rest => Pattern::silence(),
        Node::Note(p) => Pattern::atom(*p, DEFAULT_VEL),
        Node::Chord(ps) => Pattern::layers(
            ps.iter()
                .map(|&p| Pattern::atom(p, DEFAULT_VEL))
                .collect::<Vec<_>>(),
        ),
        Node::Seq(steps) => {
            Pattern::fastcat_w(steps.iter().map(|s| (compile(&s.node), s.weight)).collect())
        }
        Node::Alt(steps) => {
            let mut items = vec![];
            for s in steps {
                let pat = compile(&s.node);
                for _ in 0..(s.weight.round().max(1.0) as usize) {
                    items.push(pat.clone());
                }
            }
            Pattern::slowcat(items)
        }
        Node::Stack(nodes) => Pattern::layers(nodes.iter().map(compile).collect::<Vec<_>>()),
        Node::Poly(parts, steps) => {
            let base = steps.unwrap_or_else(|| Rat::int(parts[0].len().max(1) as u64));
            Pattern::layers(
                parts
                    .iter()
                    .map(|steps| {
                        let k = Rat::int(steps.len() as u64);
                        Pattern::fastcat_w(steps.iter().map(|s| (compile(&s.node), 1.0)).collect())
                            .fast_rat(base.div(k))
                    })
                    .collect::<Vec<_>>(),
            )
        }
        Node::Fast(n, k) => compile(n).fast_rat(*k),
        Node::Slow(n, k) => compile(n).fast_rat(Rat::ONE.div(*k)),
        Node::Degrade(n, prob, seed) => compile(n).degrade_seeded(*prob, *seed),
        Node::Euclid(n, k, steps, r) => {
            let inner = compile(n);
            let items = euclid_bools(*k, *steps, *r)
                .into_iter()
                .map(|b| {
                    if b {
                        (inner.clone(), 1.0)
                    } else {
                        (Pattern::silence(), 1.0)
                    }
                })
                .collect();
            Pattern::fastcat_w(items)
        }
        Node::Vel(n, v) => {
            let v = *v;
            compile(n).map_events(move |e| super::Event { vel: v, ..e })
        }
    }
}
