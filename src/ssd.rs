//! SSD interface.

use crate::consts::DATA_ACQUISITION_BASE_DIR;
#[cfg(test)]
use crate::consts::MIN_AVAILABLE_SSD_SPACE_BEFORE_SIGNUP;
#[cfg(not(test))]
use crate::consts::{SSD_MAPPER_PATH, SSD_MOUNT_DIR};
use eyre::Result;
use fs_extra::dir;
#[cfg(not(test))]
use nix::sys::statvfs::statvfs;
#[cfg(not(test))]
use std::fs;
use std::{
    convert::identity,
    future::Future,
    io,
    path::Path,
    sync::atomic::{AtomicU8, Ordering},
};
use walkdir::WalkDir;

static STATE: AtomicU8 = AtomicU8::new(STATE_UNKNOWN);

const STATE_UNKNOWN: u8 = 0;
const STATE_ACTIVE: u8 = 1;
const STATE_NOT_MOUNTED: u8 = 2;
const STATE_FAILED: u8 = 3;

/// SSD statistics.
#[derive(Debug)]
pub struct Stats {
    /// Available space on the SSD.
    pub available_space: u64,
    /// Number of signups left to upload.
    pub signups: i64,
    /// Number of files left to upload.
    pub documents: i64,
    /// Accumulative size of all files left to upload.
    pub documents_size: Result<u64>,
}

/// Calculate SSD statistics.
pub fn stats() -> Result<Option<Stats>> {
    fn map_err(err: walkdir::Error) -> io::Result<walkdir::Result<i64>> {
        if err.io_error().is_some() { Err(err.into_io_error().unwrap()) } else { Ok(Err(err)) }
    }
    if Path::new(DATA_ACQUISITION_BASE_DIR).exists() {
        let signups = perform(|| {
            WalkDir::new(DATA_ACQUISITION_BASE_DIR)
                .min_depth(1)
                .max_depth(1)
                .into_iter()
                .try_fold(0, |count, entry| Ok(count + i64::from(entry?.file_type().is_dir())))
                .map_or_else(map_err, |count| Ok(Ok(count)))
        })
        .map_or(Ok(-1), identity)?;
        let documents = perform(|| {
            WalkDir::new(DATA_ACQUISITION_BASE_DIR)
                .min_depth(2)
                .into_iter()
                .try_fold(0, |count, entry| Ok(count + i64::from(entry?.file_type().is_file())))
                .map_or_else(map_err, |count| Ok(Ok(count)))
        })
        .map_or(Ok(-1), identity)?;
        let documents_size =
            perform(|| Ok(dir::get_size(DATA_ACQUISITION_BASE_DIR).map_err(eyre::Report::from)))
                .map_or(Err(eyre::eyre!("ssd perform failed")), identity);
        return Ok(Some(Stats {
            available_space: available_space(),
            signups,
            documents,
            documents_size,
        }));
    }
    Ok(None)
}

/// Performs an I/O operation on the SSD inside the closure `f`. `f` is not
/// called if the SSD is not mounted or there was an error in the past.
///
/// Returns `Some(x)` if `f` was called and returned `Ok(x)`. Returns `None`
/// otherwise.
pub fn perform<R, F: FnOnce() -> io::Result<R>>(f: F) -> Option<R> {
    if is_active() {
        match f() {
            Ok(value) => return Some(value),
            Err(err) => set_failed(&err),
        }
    }
    None
}

/// Asynchronous version of [`perform`].
pub async fn perform_async<R, F: Future<Output = io::Result<R>>>(f: F) -> Option<R> {
    if is_active() {
        match f.await {
            Ok(value) => return Some(value),
            Err(err) => set_failed(&err),
        }
    }
    None
}

/// Returns `true` if the SSD is in active state.
pub fn is_active() -> bool {
    let mut state = STATE.load(Ordering::Relaxed);
    if state == STATE_UNKNOWN {
        let new_state = if is_mounted() { STATE_ACTIVE } else { STATE_NOT_MOUNTED };
        match STATE.compare_exchange(state, new_state, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_prev_state) => {
                state = new_state;
            }
            Err(curr_state) => state = curr_state,
        }
    }
    state == STATE_ACTIVE
}

/// Returns available disk space on the SSD.
#[must_use]
pub fn available_space() -> u64 {
    #[cfg(not(test))]
    {
        if is_active() {
            return statvfs(SSD_MOUNT_DIR)
                .map_or(0, |stat| stat.fragment_size() * stat.blocks_available());
        }
        0
    }
    #[cfg(test)]
    {
        MIN_AVAILABLE_SSD_SPACE_BEFORE_SIGNUP
    }
}

/// Returns `true` if the SSD is mounted.
#[must_use]
pub fn is_mounted() -> bool {
    #[cfg(not(test))]
    {
        // pad SSD_MOUNT_DIR with spaces to avoid false positives when the mountdir is a substring of another mountpoint
        let search_pattern = format!(" {SSD_MOUNT_DIR} ");
        let mounted = fs::read_to_string("/proc/self/mounts")
            .expect("non-UTF8 content of mounts")
            .contains(&search_pattern);

        if !mounted {
            tracing::error!(
                "SSD not mounted, {SSD_MAPPER_PATH} of {SSD_MOUNT_DIR} not found in \
                 /proc/self/mounts"
            );
        }
        mounted
    }
    // Disable SSD check in unit tests.
    #[cfg(test)]
    true
}

fn set_failed(err: &io::Error) {
    let mut state = STATE.load(Ordering::Relaxed);
    loop {
        match STATE.compare_exchange_weak(state, STATE_FAILED, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => {
                tracing::error!("SSD I/O error: {:#?}", err);
                break;
            }
            Err(STATE_FAILED) => break,
            Err(new_state) => state = new_state,
        }
    }
}
