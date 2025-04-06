use std::{f32::consts::PI, time::Instant};

pub const SAMPLE_RATE: f32 = 400.0;

const FINGER_THRESHOLD: f32 = 100_000.0;
const FINGER_COOLDOWN_MS: u32 = 1000;

const EDGE_THRESHOLD: f32 = -2000.0;

const LP_CUT_OFF: f32 = 5.0;
const HP_CUT_OFF: f32 = 0.5;

pub struct HighPassFilter {
    k_a0: f32,
    k_a1: f32,
    k_b1: f32,
    last_filter_value: Option<f32>,
    last_raw_value: Option<f32>,
}

impl HighPassFilter {
    /// Create a new high-pass filter based on number of samples for decay
    pub fn from_samples(samples: f32) -> Self {
        let k_x = (-1.0 / samples).exp();
        let k_a0 = (1.0 + k_x) / 2.0;

        Self {
            k_a0,
            k_a1: -k_a0,
            k_b1: k_x,
            last_filter_value: None,
            last_raw_value: None,
        }
    }

    /// Create a new high-pass filter based on cutoff frequency
    pub fn new(cutoff: f32, sampling_frequency: f32) -> Self {
        Self::from_samples(sampling_frequency / (cutoff * 2.0 * PI))
    }

    /// Process a new sample through the filter
    pub fn run(&mut self, value: f32) -> f32 {
        let filter_value = match (self.last_filter_value, self.last_raw_value) {
            (None, _) | (_, None) => 0.0,
            (Some(last_filter), Some(last_raw)) => {
                self.k_a0 * value + self.k_a1 * last_raw + self.k_b1 * last_filter
            }
        };

        self.last_filter_value = Some(filter_value);
        self.last_raw_value = Some(value);

        filter_value
    }

    /// Reset the filter state
    pub fn reset_state(&mut self) {
        self.last_filter_value = None;
        self.last_raw_value = None;
    }
}

pub struct LowPassFilter {
    k_a0: f32,
    k_b1: f32,
    last_value: Option<f32>,
}

impl LowPassFilter {
    /// Create a new low-pass filter based on number of samples for decay
    pub fn from_samples(samples: f32) -> Self {
        let k_x = (-1.0 / samples).exp();
        let k_a0 = 1.0 - k_x;

        Self {
            k_a0,
            k_b1: k_x,
            last_value: None,
        }
    }

    /// Create a new low-pass filter based on cutoff frequency
    pub fn new(cutoff: f32, sampling_frequency: f32) -> Self {
        Self::from_samples(sampling_frequency / (cutoff * 2.0 * PI))
    }

    /// Process a new sample through the filter
    pub fn run(&mut self, value: f32) -> f32 {
        let filter_value = match self.last_value {
            None => value,
            Some(last_value) => self.k_a0 * value + self.k_b1 * last_value,
        };

        self.last_value = Some(filter_value);
        filter_value
    }

    /// Reset the filter state
    pub fn reset_state(&mut self) {
        self.last_value = None;
    }
}

struct Differentiator {
    prev: Option<f32>,
    sampling_rate: f32,
}

impl Differentiator {
    fn new() -> Self {
        Differentiator {
            prev: None,
            sampling_rate: SAMPLE_RATE,
        }
    }

    fn diff(&mut self, x: f32) -> Option<f32> {
        match self.prev {
            None => {
                self.prev = Some(x);
                None
            }
            Some(prev) => {
                let res = (x - prev) * self.sampling_rate;
                self.prev = Some(x);
                Some(res)
            }
        }
    }
    fn reset_state(&mut self) {
        self.prev = None;
    }
}

pub struct SampleData {
    pub last_heartbeat: Option<Instant>,

    fingerprint_timestamp: Instant,
    finger_detected: bool,

    pub last_diff: Option<f32>,
    crossed: bool,
    crossed_time: Option<Instant>,

    hp_filter: HighPassFilter,
    lp_filter: LowPassFilter,
    differentiator: Differentiator,

    pub bpm: Option<f32>,
}

impl SampleData {
    pub fn new() -> Self {
        SampleData {
            last_heartbeat: None,
            fingerprint_timestamp: Instant::now(),
            finger_detected: false,
            last_diff: None,
            crossed: false,
            crossed_time: None,

            hp_filter: HighPassFilter::new(HP_CUT_OFF, SAMPLE_RATE),
            lp_filter: LowPassFilter::new(LP_CUT_OFF, SAMPLE_RATE),
            differentiator: Differentiator::new(),

            bpm: None,
        }
    }

    pub fn run(&mut self, sample: f32) -> f32 {
        let mut result_sample = sample;
        if sample > FINGER_THRESHOLD {
            if self.fingerprint_timestamp.elapsed().as_millis() > FINGER_COOLDOWN_MS as u128 {
                self.finger_detected = true;
            }
        } else {
            self.reset_state();
        }

        if self.finger_detected {
            let sample = self.lp_filter.run(sample);
            let sample = self.hp_filter.run(sample);
            let diff = self.differentiator.diff(sample);

            result_sample = sample;

            if diff.is_some() && self.last_diff.is_some() {
                let diff = diff.unwrap();
                let last_diff = self.last_diff.unwrap();

                if last_diff > 0.0 && diff < 0.0 {
                    self.crossed = true;
                    self.crossed_time = Some(Instant::now());
                }

                if diff > 0.0 {
                    self.crossed = false;
                }

                if self.crossed && diff < EDGE_THRESHOLD {
                    if self.last_heartbeat.is_some_and(|last_heartbeat| {
                        self.crossed_time.is_some_and(|crossed_time| {
                            crossed_time.duration_since(last_heartbeat).as_millis() > 300
                        })
                    }) {
                        let bpm = 60_000
                            / self
                                .crossed_time
                                .unwrap()
                                .duration_since(self.last_heartbeat.unwrap())
                                .as_millis();

                        log::info!(
                            "bpm: {} ibi: {}",
                            bpm,
                            self.crossed_time
                                .unwrap()
                                .duration_since(self.last_heartbeat.unwrap())
                                .as_millis()
                        );

                        let bpm = bpm as f32;

                        if bpm > 30.0 && bpm < 200.0 {
                            self.bpm = Some(bpm);
                        } else {
                            self.bpm = None;
                        }
                    }
                    self.crossed = false;
                    self.last_heartbeat = self.crossed_time;
                }
            }

            self.last_diff = diff;
        }

        result_sample
    }

    fn reset_state(&mut self) {
        self.hp_filter.reset_state();
        self.lp_filter.reset_state();
        self.differentiator.reset_state();

        self.last_heartbeat = None;
        self.fingerprint_timestamp = Instant::now();
        self.finger_detected = false;
        self.last_diff = None;
        self.crossed = false;
        self.crossed_time = None;
    }
}
