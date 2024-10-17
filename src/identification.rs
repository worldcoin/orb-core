//! Orb identification.
use crate::process::Command;
#[cfg(not(test))]
use crate::versions_json::VersionsJson;
#[cfg(not(test))]
use eyre::bail;
use eyre::Result;
use once_cell::sync::Lazy;
use orb_endpoints::OrbId;
use std::{
    env, fs, result,
    str::FromStr,
    sync::{Arc, RwLock},
};

#[cfg(not(test))]
const VERSIONS_PATH: &str = "/usr/persistent/versions.json";
const ORB_NAME_PATH: &str = "/usr/persistent/orb-name";
const JABIL_ID_PATH: &str = "/usr/persistent/jabil-id";
const HARDWARE_VERSION_PATH: &str = "/usr/persistent/hardware_version";
#[cfg(not(test))]
const ORB_PUBLIC_KEY_PATH: &str = "/usr/persistent/se/keystore/sss_70000002_0002_0040.bin";

/// Orb (Jetson) identification which is bound to the Jetson in the Orb.
pub static ORB_ID: Lazy<OrbId> = Lazy::new(read_orb_id);
/// Orb Name (for easier reference of a whole Orb device), should not get changed if Jetson is replaced in an Orb.
pub static ORB_NAME: Lazy<String> = Lazy::new(read_orb_name);

/// Errors describe state of the fetching tokens via DBus. At startup, token is
/// NotRequested. Then, depending on the result of DBus calls to
/// AuthTokenManager, it could transition either to NotReady() or to Ok(token)
#[derive(Debug, Clone, thiserror::Error)]
pub enum TokenError {
    /// No attempts to request the token from AuthTokenManager is made yet.
    #[error("Token is not yet requested")]
    NotRequested(),
    /// AuthTokenManager is working, but not yet ready to provide the token.
    #[error("AuthTokenManager don't have the token yet")]
    NotReady(String),
}

/// Orb token.
pub static ORB_TOKEN: Lazy<Arc<RwLock<result::Result<String, TokenError>>>> =
    Lazy::new(|| Arc::new(RwLock::new(Err(TokenError::NotRequested()))));

/// The git commit during compile-time.
#[allow(clippy::manual_string_new)]
pub static GIT_VERSION: Lazy<String> = Lazy::new(|| env!("GIT_VERSION").to_string());

/// The current slot.
pub static CURRENT_BOOT_SLOT: Lazy<String> = Lazy::new(read_current_slot);

/// The odm production mode fuse content.
pub static ODM_PRODUCTION_MODE: Lazy<String> = Lazy::new(read_odm_production_mode);

/// Orb OS version for the current slot.
pub static ORB_OS_VERSION: Lazy<String> = Lazy::new(|| orb_os_version().unwrap());

/// The release type for the current slot.
pub static RELEASE_TYPE: Lazy<String> = Lazy::new(|| current_release_type().unwrap());

/// The hardware version of this Orb (f.e. EVT4).
pub static HARDWARE_VERSION: Lazy<String> = Lazy::new(read_hardware_version);

/// The Orb's public key.
#[cfg(not(test))]
pub static ORB_PUBLIC_KEY: Lazy<Vec<u8>> =
    Lazy::new(|| fs::read(ORB_PUBLIC_KEY_PATH).expect("couldn't read orb public key"));

#[cfg(test)]
pub static ORB_PUBLIC_KEY: Lazy<Vec<u8>> = Lazy::new(|| Vec::from("test orb public key"));

#[cfg(not(test))]
fn read_orb_id() -> OrbId {
    let orb_id = env::var("ORB_ID").expect("Could not read the orb id environment variable");
    OrbId::from_str(&orb_id).expect("Could not parse orb id from {orb_id}")
}

#[cfg(test)]
fn read_orb_id() -> OrbId {
    OrbId::from_str("00000000").expect("Could not parse orb id from {orb_id}")
}

fn read_orb_name() -> String {
    if let Ok(contents) = fs::read_to_string(ORB_NAME_PATH) {
        contents.trim().to_string()
    } else {
        tracing::warn!("Warning: Could not read orb name file'.");
        "unnamed-orb".to_string()
    }
}

/// Tries to read the jabil id from the file system which should get set during manufacturing/repair.
/// Can return an `Err` if file is not existent.
pub fn read_jabil_id() -> Result<String> {
    let jabil_id = String::from(fs::read_to_string(JABIL_ID_PATH)?.trim());
    Ok(jabil_id)
}

/// Set the jabil id in the file system.
pub fn set_jabil_id(jabil_id: String) -> Result<()> {
    fs::write(JABIL_ID_PATH, jabil_id)?;
    Ok(())
}

/// Returns a currently valid backend token, if there is one
///
/// # Panics
///
/// If RW lock is poisoned
pub fn get_orb_token() -> Result<String> {
    Ok((*ORB_TOKEN.read().unwrap()).clone()?)
}

#[cfg(not(test))]
fn read_versions_json() -> VersionsJson {
    let versions_str = fs::read_to_string(VERSIONS_PATH).expect("couldn't read versions.json file");
    serde_json::from_str(&versions_str).expect("couldn't deserialize versions.json file")
}

fn read_current_slot() -> String {
    match env::var("CURRENT_BOOT_SLOT")
        .expect("Could not read the current boot slot environment variable")
    {
        s if s.is_empty() => panic!("CURRENT_BOOT_SLOT environmental variable is empty"),
        other => other,
    }
}

fn read_odm_production_mode() -> String {
    match env::var("ODM_PRODUCTION_MODE")
        .expect("Could not read the odm production mode environment variable")
    {
        s if s.is_empty() => panic!("ODM_PRODUCTION_MODE environmental variable is empty"),
        other => other,
    }
}

#[cfg(not(test))]
fn orb_os_version() -> Result<String> {
    let versions_json = read_versions_json();
    match CURRENT_BOOT_SLOT.as_str() {
        "a" => Ok(versions_json.releases.slot_a),
        "b" => Ok(versions_json.releases.slot_b),
        slot => bail!("Unexpected slot: {slot}"),
    }
}

#[allow(clippy::unnecessary_wraps)]
#[cfg(test)]
fn orb_os_version() -> Result<String> {
    Ok("test".into())
}

fn current_release_type() -> Result<String> {
    let output = Command::new("/usr/local/bin/release-type").arg(&*CURRENT_BOOT_SLOT).output()?;
    Ok(String::from_utf8(output.stdout)?.trim().into())
}

fn read_hardware_version() -> String {
    fs::read_to_string(HARDWARE_VERSION_PATH).unwrap_or(String::from("UNKNOWN"))
}
