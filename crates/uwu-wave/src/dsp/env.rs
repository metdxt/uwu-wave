//! Envelopes.

/// ADSR stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdsrStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// Classic ADSR. Linear attack, exponential decay/release (times are "to ~-60 dB").
/// Attack starts from the current level, so retriggers don't click.
#[derive(Clone, Copy, Debug)]
pub struct Adsr {
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    stage: AdsrStage,
    level: f32,
    att_inc: f32,
    dec_coef: f32,
    rel_coef: f32,
}

impl Default for Adsr {
    fn default() -> Adsr {
        Adsr::new(0.005, 0.1, 1.0, 0.1)
    }
}

impl Adsr {
    /// Times in seconds, sustain 0..1.
    pub fn new(attack: f32, decay: f32, sustain: f32, release: f32) -> Adsr {
        Adsr {
            attack,
            decay,
            sustain,
            release,
            stage: AdsrStage::Idle,
            level: 0.0,
            att_inc: 1.0,
            dec_coef: 0.0,
            rel_coef: 0.0,
        }
    }
    fn coef(t: f32, sr: f32) -> f32 {
        if t <= 0.0 { 0.0 } else { (-6.9 / (t * sr)).exp() }
    }
    /// Start (or retrigger) the envelope.
    pub fn gate_on(&mut self, sr: f32) {
        self.att_inc = if self.attack <= 0.0 { 1.0 } else { 1.0 / (self.attack * sr) };
        self.dec_coef = Self::coef(self.decay, sr);
        self.rel_coef = Self::coef(self.release, sr);
        self.stage = AdsrStage::Attack;
    }
    /// Enter the release stage.
    pub fn gate_off(&mut self, sr: f32) {
        if self.stage != AdsrStage::Idle {
            self.rel_coef = Self::coef(self.release, sr);
            self.stage = AdsrStage::Release;
        }
    }
    /// Kill immediately.
    pub fn reset(&mut self) {
        self.stage = AdsrStage::Idle;
        self.level = 0.0;
    }
    pub fn stage(&self) -> AdsrStage {
        self.stage
    }
    pub fn level(&self) -> f32 {
        self.level
    }
    pub fn is_idle(&self) -> bool {
        self.stage == AdsrStage::Idle
    }
    /// Next envelope value (0..1).
    #[inline]
    pub fn next(&mut self) -> f32 {
        match self.stage {
            AdsrStage::Idle => {}
            AdsrStage::Attack => {
                self.level += self.att_inc;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = AdsrStage::Decay;
                }
            }
            AdsrStage::Decay => {
                self.level = self.sustain + (self.level - self.sustain) * self.dec_coef;
                if (self.level - self.sustain).abs() < 1e-4 {
                    self.level = self.sustain;
                    self.stage = AdsrStage::Sustain;
                }
            }
            AdsrStage::Sustain => {
                self.level = self.sustain;
            }
            AdsrStage::Release => {
                self.level *= self.rel_coef;
                if self.level < 1e-4 {
                    self.level = 0.0;
                    self.stage = AdsrStage::Idle;
                }
            }
        }
        if self.stage == AdsrStage::Sustain && self.sustain <= 1e-4 {
            self.level = 0.0;
            self.stage = AdsrStage::Idle;
        }
        self.level
    }
}
