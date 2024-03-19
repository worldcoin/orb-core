//! Generic time-series list.

use crate::timestamped::Timestamped;
use serde::{Serialize, Serializer};
use std::{
    collections::vec_deque::{Iter, VecDeque},
    iter::{DoubleEndedIterator, ExactSizeIterator, FusedIterator},
    marker::PhantomData,
    time::Duration,
};

#[cfg(test)]
use mock_instant::Instant;
#[cfg(not(test))]
use std::time::Instant;

/// Generic time-series list.
///
/// See [the module-level documentation](self) for details.
#[derive(Debug)]
pub struct TimeSeries<T> {
    buf: VecDeque<Timestamped<T>>,
    limit: Option<usize>,
    timeout: Option<Duration>,
}

/// [`TimeSeries`] builder.
pub struct Builder<T> {
    capacity: Option<usize>,
    limit: Option<usize>,
    timeout: Option<Duration>,
    _marker: PhantomData<T>,
}

impl<T> Builder<T> {
    /// Creates a new [`TimeSeries`].
    #[must_use]
    pub fn build(self) -> TimeSeries<T> {
        TimeSeries {
            buf: self.capacity.map_or_else(VecDeque::new, VecDeque::with_capacity),
            limit: self.limit,
            timeout: self.timeout,
        }
    }

    /// Sets the internal buffer capacity. The buffer will be able to hold
    /// exactly capacity elements without reallocating.
    #[must_use]
    pub fn capacity(mut self, capacity: usize) -> Self {
        self.capacity = Some(capacity);
        self
    }

    /// Sets the maximum items count. The items added earlier will be deleted to
    /// maintain the items limit.
    #[must_use]
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Sets the data timeout.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

impl<T> TimeSeries<T> {
    /// Creates a new [`Builder`].
    #[must_use]
    pub fn builder() -> Builder<T> {
        Builder { capacity: None, limit: None, timeout: None, _marker: PhantomData }
    }

    /// Appends an element to the collection.
    pub fn push(&mut self, value: T) {
        self.cleanup_exceeding();
        self.cleanup_stale();
        self.buf.push_back(Timestamped::new(value));
    }

    /// Returns an iterator over the timestamped values.
    pub fn iter(&mut self) -> Iter<'_, Timestamped<T>> {
        self.cleanup_stale();
        self.buf.iter()
    }

    /// Returns an iterator over the values.
    pub fn values(
        &mut self,
    ) -> impl Iterator<Item = &T> + DoubleEndedIterator + FusedIterator + ExactSizeIterator + '_
    {
        self.iter().map(|timestamped| &timestamped.value)
    }

    /// Removes the last element from the collection and returns it, or None if
    /// it is empty.
    pub fn pop(&mut self) -> Option<Timestamped<T>> {
        self.buf.pop_back()
    }

    fn cleanup_exceeding(&mut self) {
        if let Some(limit) = self.limit {
            if self.buf.len() >= limit {
                self.buf.pop_front();
            }
        }
    }

    fn cleanup_stale(&mut self) {
        let Some(timeout) = self.timeout else {
            return;
        };
        let now = Instant::now();
        let mut i = 0;
        for value in &self.buf {
            if let Some(duration) = now.checked_duration_since(value.timestamp) {
                if duration <= timeout {
                    break;
                }
            }
            i += 1;
        }
        for _ in 0..i {
            self.buf.pop_front();
        }
    }
}

impl<T: Serialize> Serialize for TimeSeries<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.buf.serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mock_instant::MockClock;

    #[test]
    fn test_persistence() {
        let mut data = TimeSeries::builder().build();
        data.push(1);
        data.push(2);
        data.push(3);
        assert_eq!(collect_values(&mut data), &[1, 2, 3]);
    }

    #[test]
    fn test_limit() {
        let mut data = TimeSeries::builder().limit(3).build();
        data.push(1);
        data.push(2);
        data.push(3);
        assert_eq!(collect_values(&mut data), &[1, 2, 3]);
        data.push(4);
        assert_eq!(collect_values(&mut data), &[2, 3, 4]);
        data.push(1);
        assert_eq!(collect_values(&mut data), &[3, 4, 1]);
    }

    #[test]
    fn test_timeout() {
        let mut data = TimeSeries::builder().timeout(Duration::from_millis(1000)).build();
        data.push(1);
        MockClock::advance(Duration::from_millis(100));
        data.push(2);
        MockClock::advance(Duration::from_millis(100));
        data.push(3);
        assert_eq!(collect_values(&mut data), &[1, 2, 3]);
        MockClock::advance(Duration::from_millis(801));
        data.push(4);
        assert_eq!(collect_values(&mut data), &[2, 3, 4]);
        data.push(5);
        assert_eq!(collect_values(&mut data), &[2, 3, 4, 5]);
        MockClock::advance(Duration::from_millis(100));
        assert_eq!(collect_values(&mut data), &[3, 4, 5]);
        MockClock::advance(Duration::from_millis(2000));
        assert!(collect_values(&mut data).is_empty());
    }

    fn collect_values<T: Copy>(data: &mut TimeSeries<T>) -> Vec<T> {
        data.values().copied().collect::<Vec<_>>()
    }
}
