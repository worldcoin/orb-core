use std::collections::VecDeque;

/// Repeats the target function with certain amount of time lag.
#[derive(Default)]
pub struct Lagging {
    tail: Option<f64>,
    record: VecDeque<(f64, f64)>,
    record_dt: f64,
}

impl Lagging {
    /// Adds a new partition of the target functions. Returns the lagged value.
    pub fn add(&mut self, x: f64, dt: f64, lag: f64) -> Option<f64> {
        if self.tail.is_some() {
            self.record.push_front((x, dt));
            self.record_dt += dt;
            while self.record_dt >= lag {
                let (back_x, back_dt) = self.record.back().unwrap();
                if self.record_dt - back_dt < lag {
                    let tail = self.tail.unwrap();
                    return Some(tail + (back_x - tail) * ((self.record_dt - lag) / back_dt));
                }
                let (back_x, back_dt) = self.record.pop_back().unwrap();
                self.tail = Some(back_x);
                self.record_dt -= back_dt;
            }
        } else {
            self.tail = Some(x);
        }
        None
    }

    /// Resets the signal.
    pub fn reset(&mut self) {
        self.tail = None;
        self.record.clear();
        self.record_dt = 0.0;
    }
}
