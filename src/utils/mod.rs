//! Utils module that can be used by multiple agents.

pub mod rkyv_ndarray;

pub use self::rkyv_ndarray::RkyvNdarray;

use std::{ffi::CString, thread, time::Duration};

/// Sample the rate of a function over a given time period.
/// Returns `true` if sampling should be performed.
#[must_use]
pub fn sample_at_fps(fps: f32, current_time: Duration, last_saved_time: Duration) -> bool {
    // >= is important, because 1.0 / INFINITY == 0.0, and we pass INFINITY to always sample.
    current_time.checked_sub(last_saved_time).unwrap_or_default()
        >= Duration::from_secs_f32(1.0 / fps)
}

/// Logs iris data (no-op).
#[cfg(not(feature = "log-iris-data"))]
pub fn log_iris_data(
    _iris_code: &str,
    _mask_code: &str,
    _iris_code_version: &str,
    _left_eye: bool,
    _context: &'static str,
) {
}

/// Logs iris data.
#[cfg(feature = "log-iris-data")]
pub fn log_iris_data(
    iris_code: &str,
    mask_code: &str,
    iris_code_version: &str,
    left_eye: bool,
    context: &'static str,
) {
    use crate::backend::signup_post::IrisData;
    let data = serde_json::to_string_pretty(&IrisData {
        code: iris_code.to_string(),
        mask: mask_code.to_string(),
        code_version: iris_code_version.to_string(),
    })
    .unwrap();
    tracing::info!("Iris data (left_eye: {left_eye}, context: {context:?}): {data}");
}

/// Sets the current process's name.
pub fn set_proc_name(name: impl AsRef<str>) {
    if let Ok(title) = CString::new(name.as_ref().as_bytes()) {
        unsafe { libc::prctl(libc::PR_SET_NAME, title.as_ptr(), 0, 0, 0) };
    }
}

/// Spawns a new thread, setting its unix name.
pub fn spawn_named_thread<F, T>(name: impl Into<String>, f: F) -> thread::JoinHandle<T>
where
    F: FnOnce() -> T,
    F: Send + 'static,
    T: Send + 'static,
{
    let name = name.into();
    thread::Builder::new()
        .name(name.clone())
        .spawn(move || {
            set_proc_name(name);
            f()
        })
        .expect("failed to spawn thread")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::sample_at_fps;

    #[test]
    fn infinity_always_samples() {
        // last sample is current time.
        assert!(sample_at_fps(f32::INFINITY, Duration::ZERO, Duration::ZERO));
        // last sample was 1 second ago.
        assert!(sample_at_fps(f32::INFINITY, Duration::from_secs(1), Duration::ZERO));
        // last sample was in the future 🪐
        assert!(sample_at_fps(f32::INFINITY, Duration::ZERO, Duration::from_secs(1)));
    }
}
