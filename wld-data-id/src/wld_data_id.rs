use crate::s3_region::S3Region;
use eyre::{eyre, Error, Result};
use rand::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{convert::TryInto, fmt, path::Path, str::FromStr, time::Duration};
use uuid::Uuid;

const VERSION: u8 = 0;

#[derive(Serialize, Deserialize, JsonSchema, Clone, Default, Eq, PartialEq, Debug)]
struct WldDataId {
    /// The version of this structure.
    version: u8,
    /// The AWS region this object will be uploaded to.
    s3_region: S3Region,
    /// A globally unique id for the signup.
    signup_id: [u8; 10],
    /// An id for some data (e.g. an image), unique within the signup.
    data_id: u32,
}

impl WldDataId {
    fn to_uuid(&self) -> Uuid {
        let bytes = bincode::serialize(self).unwrap();
        Uuid::from_slice(&bytes).unwrap()
    }
}

impl From<Uuid> for WldDataId {
    fn from(uuid: Uuid) -> WldDataId {
        bincode::deserialize(uuid.as_bytes()).unwrap()
    }
}

impl FromStr for WldDataId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Uuid::parse_str(s)?.into())
    }
}

impl fmt::Display for WldDataId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_uuid().simple().encode_lower(&mut Uuid::encode_buffer()))
    }
}

#[allow(missing_docs)]
#[derive(Serialize, Deserialize, JsonSchema, Clone, Default, Debug, Eq, PartialEq)]
pub struct SignupId(WldDataId);

impl SignupId {
    /// Generates a globally unique signup id given the S3 region.
    #[must_use]
    pub fn new(s3_region: S3Region) -> Self {
        Self(WldDataId { version: VERSION, s3_region, signup_id: thread_rng().gen(), data_id: 0 })
    }

    /// Parses a signup id from the signup directory.
    pub fn from_signup_dir(path: &Path) -> Result<Self> {
        path.file_name().ok_or_else(|| eyre!("Invalid path {:?}", path))?.to_string_lossy().parse()
    }
}

impl fmt::Display for SignupId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for SignupId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse()?))
    }
}

impl From<ImageId> for SignupId {
    fn from(image_id: ImageId) -> Self {
        let mut tmp_id = image_id.0;
        tmp_id.data_id = 0;
        Self(tmp_id)
    }
}

#[allow(missing_docs)]
#[derive(Serialize, Deserialize, JsonSchema, Clone, Default, Debug, Eq, PartialEq)]
pub struct ImageId(WldDataId);

impl ImageId {
    /// Generates a new image id, given a signup id and the image timestamp.
    #[must_use]
    pub fn new(signup_id: &SignupId, timestamp: Duration) -> Self {
        let r: u32 = thread_rng().gen();
        let t_least_sig_bytes = &timestamp.as_nanos().to_le_bytes()[0..4];
        let t = u32::from_le_bytes(t_least_sig_bytes.try_into().unwrap());
        let mut new_id = signup_id.0.clone();
        new_id.data_id = r ^ t;
        Self(new_id)
    }

    /// Parses an image id from an image path.
    pub fn from_image_path(path: &Path) -> Result<Self> {
        path.file_stem().ok_or_else(|| eyre!("Invalid path {:?}", path))?.to_string_lossy().parse()
    }
}

impl fmt::Display for ImageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for ImageId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(ImageId(s.parse()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eyre::Result;
    use std::time::SystemTime;

    #[test]
    fn test_object_id() -> Result<()> {
        let start = SystemTime::now();
        let signup_id = SignupId::new(S3Region::Unknown);
        let image_id = ImageId::new(&signup_id, SystemTime::now().duration_since(start)?);
        let s = image_id.to_string();
        assert_eq!(s.parse::<WldDataId>()?.to_string(), s);
        Ok(())
    }

    #[test]
    fn test_sensitivity() {
        let signup_id = SignupId::new(S3Region::Unknown);
        let t1 = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system time must be after UNIX EPOCH");
        let t2 = t1 + Duration::from_nanos(1);

        assert_ne!(
            ImageId::new(&signup_id, t1).to_string(),
            ImageId::new(&signup_id, t2).to_string()
        );
    }
}
