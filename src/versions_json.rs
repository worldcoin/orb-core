//! Versions JSON

use serde::Deserialize;

#[allow(missing_docs, dead_code)]
#[derive(Deserialize)]
pub struct SlotReleases {
    pub slot_a: String,
    pub slot_b: String,
}

#[allow(missing_docs, dead_code)]
#[derive(Deserialize)]
pub struct VersionsJson {
    pub releases: SlotReleases,
}
