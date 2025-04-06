use std::time::Instant;

use crate::filters::*;

pub const SAMPLE_RATE: f32 = 400.0;

const FINGER_THRESHOLD: f32 = 100_000.0;
const FINGER_COOLDOWN_MS: u32 = 1000;

const EDGE_THRESHOLD: f32 = -2000.0;

const LP_CUT_OFF: f32 = 5.0;
const HP_CUT_OFF: f32 = 0.5;

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
            differentiator: Differentiator::new(SAMPLE_RATE),

            bpm: None,
        }
    }

    pub fn run(&mut self, sample: f32) -> (f32, bool) {
        let mut result_sample = sample;
        let mut beat_detected = false;

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
                        beat_detected = true;
                        let bpm = 60_000
                            / self
                                .crossed_time
                                .unwrap()
                                .duration_since(self.last_heartbeat.unwrap())
                                .as_millis();

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

        (result_sample, beat_detected)
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
        self.bpm = None;
    }
}
