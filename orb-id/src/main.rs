//! Interface to read the orb-id from the fusebanks
//!
//! This needs to be a seperate and minimal binary,
//! since it is going to have the SETUID/SETGID bit set

#![warn(clippy::pedantic)]
#![allow(clippy::doc_markdown, clippy::missing_errors_doc)]

use eyre::{eyre, Result, WrapErr as _};
use std::{fs, process::Command, str};

const ODM_LOCK_FUSE_PATH: &str = "/sys/devices/platform/tegra-fuse/odm_lock";
const RESERVED_ODM0_FUSE_PATH: &str = "/sys/devices/platform/tegra-fuse/reserved_odm0";

const ODM_LOCK_MASK: i32 = 0x0000_0001;

fn main() -> Result<()> {
    let odm_lock_set = check_odm_lock_set().wrap_err("failed to `check the odm_lock` fuse")?;

    let orb_id = if odm_lock_set {
        let reserved_odm0_string = fs::read_to_string(RESERVED_ODM0_FUSE_PATH)
            .wrap_err_with(|| format!("unable to read `{RESERVED_ODM0_FUSE_PATH}`"))?;

        // The string is in format of "0x00000000", returning 8 chars without the prefix.
        reserved_odm0_string[2..10].to_string()
    } else {
        let output = Command::new("/usr/local/bin/orb-id-legacy")
            .output()
            .wrap_err("failed running `orb-id-legacy`")?;
        output
            .status
            .success()
            .then_some(())
            .ok_or_else(|| eyre!("`/usr/local/bin/orb-id-legacy` terminated unsuccessfully"))?;

        // TODO(andronat): We should remove legacy support.
        str::from_utf8(&output.stdout)
            .wrap_err("failed parsing `orb-id-legacy` output as utf8")?
            .to_string()
    };
    print!("{orb_id}");
    Ok(())
}

fn check_odm_lock_set() -> Result<bool> {
    let odm_lock_string = fs::read_to_string(ODM_LOCK_FUSE_PATH)
        .wrap_err_with(|| format!("unable to read `{ODM_LOCK_FUSE_PATH}`"))?;

    let subslice = &odm_lock_string[2..10];
    let odm_lock_value = i32::from_str_radix(subslice, 16)
        .wrap_err_with(|| format!("failed parsing `{subslice}` as hex string"))?;
    Ok((ODM_LOCK_MASK & odm_lock_value) == 1)
}
