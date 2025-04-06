pub struct Linreg {
    pub intercept: f32,
    pub slope: f32,

    n: usize,

    // a few constants that depend on the array size
    sumx: f32,
    sum_xsq: f32,
    sumx_sq: f32,
}

impl Linreg {
    pub fn new(n: usize) -> Self {
        let sumx = ((n - 1) * n) as f32 / 2.0;
        let sum_xsq = (0..n).map(|x| x as f32 * x as f32).sum();
        Linreg {
            intercept: 0.0,
            slope: 1.0,
            n,
            sumx,
            sum_xsq,
            sumx_sq: sumx * sumx,
        }
    }

    fn update_constants(&mut self) {
        self.sumx = ((self.n - 1) * self.n) as f32 / 2.0;
        self.sum_xsq = (0..self.n).map(|x| x as f32 * x as f32).sum();
        self.sumx_sq = self.sumx * self.sumx;
    }

    pub fn y(&self, x: f32) -> f32 {
        self.intercept + self.slope * x
    }

    pub fn update_from(&mut self, data: &[f32]) {
        let mut sum_y = 0.0;
        let mut sum_xy = 0.0;

        for (i, x) in data.iter().enumerate() {
            sum_y += x;
            sum_xy += x * i as f32;
        }

        let n = data.len();
        if n == 0 {
            self.intercept = 0.0;
            self.slope = 1.0;
            return;
        }
        if n != self.n {
            self.n = n;
            self.update_constants();
        }

        let n = self.n as f32;
        self.intercept =
            (sum_y * self.sum_xsq - self.sumx * sum_xy) / (n * self.sum_xsq - self.sumx_sq);

        self.slope = (n * sum_xy - self.sumx * sum_y) / (n * self.sum_xsq - self.sumx_sq);
    }
}
