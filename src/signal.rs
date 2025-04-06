//! Signal shaping functions

/// Heartbeats, looking at high-to-low sensor transitions.
/// Fast rate of change, less likely to confuse with noise
/// (flip of the sign of the derivative).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heartbeat {
    pub high_idx: usize,
    pub high_value: f32,
    pub low_idx: usize,
    pub low_value: f32,
}

pub struct HeartbeatItr<'a, const N: usize> {
    deriv_itr: DerivItr<'a, N>,
    high: Option<DerivItrItem>,
    last_deriv: Option<f32>,
}

impl<'a, const N: usize> HeartbeatItr<'a, N> {
    pub fn new(data: &'a [f32; N]) -> Self {
        HeartbeatItr {
            deriv_itr: DerivItr::new(data),
            high: None,
            last_deriv: None,
        }
    }
}

impl<'a, const N: usize> Iterator for HeartbeatItr<'a, N> {
    type Item = Heartbeat;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let d = self.deriv_itr.next()?;

            let last_deriv = match self.last_deriv {
                Some(ld) => ld,
                None => {
                    self.last_deriv = Some(d.deriv);
                    continue;
                }
            };

            self.last_deriv = Some(d.deriv);

            if d.deriv >= 0.0 {
                if last_deriv >= 0.0 {
                    self.high = Some(d);
                } else {
                    match self.high {
                        Some(h) => {
                            break Some(Heartbeat {
                                high_idx: h.idx,
                                high_value: h.sample,
                                low_idx: d.idx,
                                low_value: d.sample,
                            })
                        }
                        None => continue,
                    }
                }
            }
        }
    }
}

/// Sample, its index and a first derivative
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DerivItrItem {
    idx: usize,
    sample: f32,
    deriv: f32,
}

pub struct DerivItr<'a, const N: usize> {
    data: &'a [f32; N],
    idx: usize,
}

impl<'a, const N: usize> DerivItr<'a, N> {
    pub fn new(data: &'a [f32; N]) -> Self {
        DerivItr { data, idx: 0 }
    }
}

impl<'a, const N: usize> Iterator for DerivItr<'a, N> {
    type Item = DerivItrItem;

    fn next(&mut self) -> Option<Self::Item> {
        if self.idx >= N - 1 {
            None
        } else {
            let i = self.idx;
            self.idx += 1;

            let s = self.data[i];
            let s1 = self.data[i + 1];
            Some(DerivItrItem {
                idx: i,
                sample: s,
                deriv: s1 - s,
            })
        }
    }
}
