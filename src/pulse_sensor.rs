const ESP32_ADC_RESOLUTION: u16 = 4095;

#[derive(Debug)]
pub struct PulseSensor {
    /// holds raw Analog in 0. updated every call to read_sensor()
    bpm: u16,
    /// holds the latest incoming raw data (0..4095)
    signal: u16,
    /// holds the time interval (ms) between beats! Must be seeded!
    ibi: u16,
    /// "true" when User's live heartbeat is detected. "false" when not a "live beat".
    pulse: bool,
    /// The start of beat has been detected and not read by the Sketch.
    qs: bool,
    /// used to seed and reset the thresh variable
    thresh_setting: u16,
    /// used to hold amplitude of pulse waveform, seeded (sample value)
    amp: u16,
    /// used to find IBI. Time (sample_counter) of the previous detected beat start.
    last_beat_time: u32,

    /// expected time between calls to read_sensor(), in milliseconds.
    sample_interval_ms: u32,
    /// array to hold last ten IBI values (ms)
    rate: [u16; 10],
    /// used to determine pulse timing. Milliseconds since we started.
    sample_counter: u32,
    /// used to monitor duration between beats
    n: u16,
    /// used to find peak in pulse wave, seeded (sample value)
    p: u16,
    /// used to find trough in pulse wave, seeded (sample value)
    t: u16,
    /// used to find instant moment of heart beat, seeded (sample value)
    thresh: u16,
    /// used to seed rate array so we startup with reasonable BPM
    first_beat: bool,
    /// used to seed rate array so we startup with reasonable BPM
    second_beat: bool,
}

impl PulseSensor {
    // Constructs a PulseSensor manager using a default configuration.
    pub fn new() -> Self {
        Self {
            bpm: 0,
            signal: 0,
            ibi: 750, // 750ms per beat = 80 Beats Per Minute (BPM)
            pulse: false,
            qs: false,
            thresh_setting: ESP32_ADC_RESOLUTION / 10 * 6,
            amp: ESP32_ADC_RESOLUTION / 10, // beat amplitude 1/10 of input range.
            last_beat_time: 0,
            sample_interval_ms: 2, // 500 Hz
            rate: [0; 10],
            sample_counter: 0,
            n: 0,
            p: ESP32_ADC_RESOLUTION / 2, // peak at 1/2 the input range of 0..1023
            t: ESP32_ADC_RESOLUTION / 2, // trough at 1/2 the input range.
            thresh: ESP32_ADC_RESOLUTION / 10 * 6,
            first_beat: true,   // looking for the first beat
            second_beat: false, // not yet looking for the second beat in a row
        }
    }

    // sets variables to default start values
    pub fn reset_variables(&mut self) {
        self.qs = false;
        self.bpm = 0;
        self.ibi = 750;
        self.pulse = false;
        self.sample_counter = 0;
        self.last_beat_time = 0;
        self.p = ESP32_ADC_RESOLUTION / 2;
        self.t = ESP32_ADC_RESOLUTION / 2;
        self.thresh = self.thresh_setting;
        self.amp = ESP32_ADC_RESOLUTION / 10;
        self.first_beat = true;
        self.second_beat = false;
    }

    // Returns the sample most recently-read from this PulseSensor.
    pub fn get_latest_sample(&self) -> u16 {
        self.signal
    }

    // Returns the latest beats-per-minute measurement on this PulseSensor.
    pub fn get_beats_per_minute(&self) -> u16 {
        self.bpm
    }

    // Returns the latest inter-beat interval (milliseconds) on this PulseSensor.
    pub fn get_inter_beat_interval_ms(&self) -> u16 {
        self.ibi
    }

    // Reads and clears the 'saw start of beat' flag, "QS".
    pub fn saw_start_of_beat(&mut self) -> bool {
        let ret = self.qs;
        self.qs = false;
        ret
    }

    // Returns true if this PulseSensor signal is inside a beat vs. outside.
    pub fn is_inside_beat(&self) -> bool {
        self.pulse
    }

    // Returns the latest amp value.
    pub fn get_pulse_amplitude(&self) -> u16 {
        self.amp
    }

    // Returns the sample number of the most recent detected pulse.
    pub fn get_last_beat_time(&self) -> u32 {
        self.last_beat_time
    }

    // (internal to the library) Read a sample from this PulseSensor.
    pub fn read_next_sample(&mut self, sample: u16) {
        self.signal = sample;
    }

    // (internal to the library) Process the latest sample.
    pub fn process_latest_sample(&mut self) {
        self.sample_counter += self.sample_interval_ms; // keep track of the time in mS with this variable
        self.n = (self.sample_counter - self.last_beat_time) as u16; // monitor the time since the last beat to avoid noise

        //  find the peak and trough of the pulse wave
        if self.signal < self.thresh && self.n > (self.ibi / 5) * 3 {
            // avoid dichrotic noise by waiting 3/5 of last IBI
            if self.signal < self.t {
                // T is the trough
                self.t = self.signal; // keep track of lowest point in pulse wave
            }
        }

        if self.signal > self.thresh && self.signal > self.p {
            // thresh condition helps avoid noise
            self.p = self.signal // P is the peak
        } // keep track of highest point in pulse wave

        if self.n > 250 {
            // avoid high frequency noise
            if self.signal > self.thresh && !self.pulse && self.n > (self.ibi / 5) * 3 {
                self.pulse = true; // set the pulse flag when we think there is a pulse
                self.ibi = self.n; // measure time between beats in mS
                self.last_beat_time = self.sample_counter; // keep track of time for next pulse

                if self.second_beat {
                    // if this is the second beat, if second_beat == TRUE
                    self.second_beat = false; // clear second beat flag
                    for i in 0..10 {
                        // seed the running total to get a realisitic BPM at startup
                        self.rate[i] = self.ibi;
                    }
                }

                if self.first_beat {
                    self.first_beat = false; // clear first beat flag
                    self.second_beat = true; // set the second beat flag
                    return;
                }

                // keep a running total of the last 10 IBI values
                for i in 0..9 {
                    self.rate[i] = self.rate[i + 1];
                }
                self.rate[9] = self.ibi; // add the latest IBI to the rate array

                // average the last 10 IBI values
                let running_total = self.rate.iter().sum::<u16>() / 10;
                self.bpm = 60_000 / running_total; // how many beats can fit into a minute? that's BPM!
                self.qs = true; // set the Quantified Self flag
            }
        }

        if self.signal < self.thresh && self.pulse {
            // when the beat goes below the threshold, the beat is over
            self.pulse = false; // reset the pulse flag so we can do it again
            self.amp = self.p - self.t; // get amplitude of the pulse wave
            self.thresh = (self.p + self.t) / 2; // get the average of the peak and trough
            self.p = self.thresh; // reset the peak
            self.t = self.thresh; // reset the trough
        }

        if self.n > 2500 {
            // if 2.5 seconds go by without a beat
            self.thresh = self.thresh_setting; // reset the threshold
            self.p = ESP32_ADC_RESOLUTION / 2; // reset the peak
            self.t = ESP32_ADC_RESOLUTION / 2; // reset the trough
            self.last_beat_time = self.sample_counter; // bring the last beat up to date
            self.first_beat = true; // set the first beat flag so we can do a real IBI measurement
            self.second_beat = false; // clear the second beat flag
            self.qs = false; // reset the Quantified Self flag
            self.bpm = 0; // reset the BPM to 0
            self.ibi = 750; // reset the IBI to 750
            self.pulse = false; // reset the pulse flag
            self.amp = ESP32_ADC_RESOLUTION / 10; // reset the amplitude
        }
    }

    // (internal to the library) Update the thresh variables.
    fn set_threshold(&mut self, threshold: u16) {
        self.thresh_setting = threshold;
        self.thresh = threshold;
    }
}
