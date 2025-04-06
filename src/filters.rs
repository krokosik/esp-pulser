use std::f32::consts::PI;

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

pub struct Differentiator {
    prev: Option<f32>,
    sampling_rate: f32,
}

impl Differentiator {
    pub fn new(sampling_rate: f32) -> Self {
        Differentiator {
            prev: None,
            sampling_rate,
        }
    }

    pub fn diff(&mut self, x: f32) -> Option<f32> {
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
    pub fn reset_state(&mut self) {
        self.prev = None;
    }
}
