//! A generic wrapper that attaches a timestamp to a value.
//!
//! # Examples
//!
//! ```
//! # use orb::timestamped::Timestamped;
//!
//! let frame = vec![4, 2, 3, 5];
//!
//! // The timestamp is automatically generated when you call the `new` method:
//! let timestamped_frame = Timestamped::new(frame);
//!
//! // The wrapper can be transparently dereferenced to the inner type:
//! assert_eq!(timestamped_frame[2], 3);
//! ```

use crate::utils::serializable_instant::SerializableInstant;
#[cfg(test)]
use mock_instant::Instant;
use schemars::JsonSchema;
use serde::{Serialize, Serializer};
use std::ops::{Deref, DerefMut};
#[cfg(not(test))]
use std::time::Instant;

/// A generic wrapper that attaches a timestamp to a value.
///
/// See [the module-level documentation](self) for details.
#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
pub struct Timestamped<T> {
    /// The wrapped value.
    pub value: T,
    /// The attached timestamp.
    #[schemars(with = "String")]
    #[serde(serialize_with = "serialize_instant")]
    pub timestamp: Instant,
}

impl<T> Timestamped<T> {
    /// Creates a new `Timestamped<T>` with the timestamp corresponding to
    /// "now".
    pub fn new(value: T) -> Self {
        Self::with_timestamp(value, Instant::now())
    }

    /// Creates a new `Timestamped<T>` with custom timestamp.
    pub fn with_timestamp(value: T, timestamp: Instant) -> Self {
        Self { value, timestamp }
    }

    /// Unwraps the value.
    pub fn into_inner(self) -> T {
        self.value
    }

    /// Maps `Timestamped<T>` to `Timestamped<U>` by applying a function to a
    /// contained value.
    pub fn map<U, F>(self, f: F) -> Timestamped<U>
    where
        F: FnOnce(T) -> U,
    {
        let Timestamped { value, timestamp } = self;
        Timestamped { value: f(value), timestamp }
    }
}

impl<T> Deref for Timestamped<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T> DerefMut for Timestamped<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

fn serialize_instant<S>(instant: &Instant, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    SerializableInstant::new(*instant).serialize(serializer)
}
