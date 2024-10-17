//! A wrapper for [`std::time::Instant`] that can be serialized with serde.
//!
//! See: <https://github.com/serde-rs/serde/issues/1375>

#[cfg(test)]
use mock_instant::Instant;
use schemars::JsonSchema;
use serde::{Serialize, Serializer};
#[cfg(not(test))]
use std::time::Instant;
use std::time::SystemTime;

/// A wrapper for [`Instant`] that can be serialized with serde.
#[derive(JsonSchema, Clone, Copy, Debug)]
pub struct SerializableInstant(#[schemars(with = "String")] Instant);

impl SerializableInstant {
    /// Creates a new [`SerializableInstant`].
    #[must_use]
    pub fn new(instant: Instant) -> Self {
        Self(instant)
    }

    /// Returns the inner [`Instant`].
    #[must_use]
    pub fn into_instant(self) -> Instant {
        self.0
    }
}

impl Serialize for SerializableInstant {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let system_now = SystemTime::now();
        let instant_now = Instant::now();
        let approx = system_now - (instant_now - self.0);
        approx.serialize(serializer)
    }
}
