use esp_idf_svc::hal::units::Hertz;
use heapless::{binary_heap::Max, BinaryHeap, Vec};
use std::cmp::min;

use crate::{
    linreg::Linreg,
    signal::{Heartbeat, HeartbeatItr},
};

pub const MAX30102_NUM_SAMPLES: usize = 100;
pub const MAX30102_SAMPLE_RATE: Hertz = Hertz(25);

pub struct Max3012SampleData {
    /// "AC" component of R/IR signal sample
    /// (sensor value - DC mean subtracted)
    pub ac: [f32; MAX30102_NUM_SAMPLES],

    /// "DC" mean of the sample
    pub dc_mean: f32,

    /// Number of samples to skip from the beginning of the data
    /// (to ignore initial "junk" movement data)
    pub data_to_skip: usize,

    /// for scale, to display raw data
    pub ac_max: f32,
    pub ac_min: f32,

    linreg: Linreg,

    pub heartbeats: Vec<Heartbeat, 16>,

    pub heart_rate_bpm: Option<f32>,
}

impl Max3012SampleData {
    pub fn new() -> Self {
        Max3012SampleData {
            ac: [0.0; MAX30102_NUM_SAMPLES],
            dc_mean: 0.0,
            data_to_skip: 0,

            ac_max: 1.0,
            ac_min: 0.0,

            linreg: Linreg::new(MAX30102_NUM_SAMPLES),

            heartbeats: Vec::new(),

            heart_rate_bpm: None,
        }
    }

    pub fn update_from_samples(&mut self, data: impl Iterator<Item = f32>) {
        // self.data_to_skip = data
        //     .enumerate()
        //     .fold((0, None::<f32>), |(res, prev), (i, x)| {
        //         self.ac[i] = x;
        //         if let Some(prev) = prev {
        //             if (x - prev).abs() > 100_000.0 {
        //                 (i + 1, Some(prev))
        //             } else {
        //                 (res, Some(x))
        //             }
        //         } else {
        //             (res, Some(x))
        //         }
        //     })
        //     .0;

        // self.data_to_skip = 0;
        for (i, x) in data.enumerate() {
            self.ac[i] = x;
        }
    }

    pub fn process_signal(&mut self) {
        self.dc_mean = 0.0;
        self.ac_max = f32::MIN;
        self.ac_min = f32::MAX;

        self.linreg.update_from(&self.ac[self.data_to_skip..]);

        for (i, ac) in self.ac.iter_mut().enumerate() {
            if i < self.data_to_skip {
                *ac = 0.0;
            } else {
                *ac -= self.linreg.y(i as f32);
                self.ac_max = self.ac_max.max(*ac);
                self.ac_min = self.ac_min.min(*ac);
            }
        }

        self.heartbeats.clear();

        // Keep track of distances (in array indexes) between heartbeats
        let mut hb_dist: BinaryHeap<usize, Max, 16> = BinaryHeap::new();
        let mut last_hb_idx: Option<usize> = None;
        let hb_threshold = (self.ac_max - self.ac_min) / 4.0;
        for hb in HeartbeatItr::new(&self.ac) {
            // Ignore small amplitude "wiggles", focus on larger transitions.
            // This only works if overall signal is clean enough from motion
            // artifacts (i.e. actual heartbeats stay relatively close to
            // min/max amplitude values).
            let hb_val_diff = hb.high_value - hb.low_value;

            if hb_val_diff > hb_threshold {
                let _ = self.heartbeats.push(hb);
                if let Some(lhb) = last_hb_idx {
                    let _ = hb_dist.push(hb.high_idx - lhb);
                }

                last_hb_idx = Some(hb.high_idx);
            }
        }

        self.heart_rate_bpm = None;

        if hb_dist.len() > 2 && self.linreg.intercept > 100_000.0 {
            let mean_hb_dist = hb_dist.iter().sum::<usize>() as f32 / hb_dist.len() as f32;

            let bpm = 60.0 * MAX30102_SAMPLE_RATE.0 as f32 / mean_hb_dist;

            if bpm < 180.0 && bpm > 60.0 {
                self.heart_rate_bpm = Some(bpm);
            }
        }
    }
}
