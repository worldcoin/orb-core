//! Building and uploading the personal custody package.

use super::{biometric_capture, qr_scan};
use crate::{
    agents::{
        camera::{Frame as _, FrameResolution},
        image_notary::IdentificationImages,
        python::face_identifier,
    },
    backend::signup_post::SignupReason,
    debug_report::LocationData,
    identification::{ORB_ID, ORB_OS_VERSION, ORB_PUBLIC_KEY},
    secure_element::sign,
    utils::serialize_with_sorted_keys::SerializeWithSortedKeys,
};
use data_encoding::{BASE64, HEXLOWER};
use eyre::{bail, ensure, Result, WrapErr};
use flate2::GzBuilder;
use hyrax::iriscode_commit::{compute_commitments_binary_outputs, HyraxCommitmentOutputSerialized};
use ndarray::prelude::*;
use orb_wld_data_id::SignupId;
use rand::random;
use ring::digest::{digest, Digest, SHA256};
use serde::Serialize;
use sodiumoxide::crypto::box_::PublicKey;
use std::{
    collections::BTreeMap,
    io::prelude::*,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::task;

const IRIS_MPC_VERSION: &str = env!("IRIS_MPC_VERSION");
const VERSION_V2: &str = "2.3";
const VERSION_V3: &str = "3.0";

/// A plan for building and uploading the personal custody package.
#[allow(missing_docs)]
pub struct Plan {
    pub capture_start: SystemTime,
    pub signup_id: SignupId,
    pub identification_image_ids: IdentificationImages,
    pub capture: biometric_capture::Capture,
    pub pipeline: Pipeline,
    pub credentials: Credentials,
    pub signup_reason: SignupReason,
    pub location_data: LocationData,
}

/// The credentials used to build the personal custody package.
#[allow(missing_docs)]
#[allow(clippy::struct_field_names)]
pub struct Credentials {
    pub operator_qr_code: qr_scan::user::Data,
    pub user_qr_code: qr_scan::user::Data,
    pub user_qr_code_string: String,
    pub backend_iris_public_key: PublicKey,
    pub backend_iris_encrypted_private_key: String,
    pub backend_normalized_iris_public_key: PublicKey,
    pub backend_normalized_iris_encrypted_private_key: String,
    pub backend_face_public_key: PublicKey,
    pub backend_face_encrypted_private_key: String,
    pub backend_tier2_public_key: Option<PublicKey>,
    pub backend_tier2_encrypted_private_key: Option<String>,
    pub self_custody_user_public_key: PublicKey,
    pub pcp_version: u16,
}

/// Biometric pipeline results used to build the personal custody package.
#[allow(missing_docs)]
pub struct Pipeline {
    pub face_identifier_thumbnail_image: Array3<u8>,
    pub face_identifier_embeddings: Vec<face_identifier::types::Embedding>,
    pub face_identifier_inference_backend: String,
    pub left_normalized_iris_image: Vec<u8>,
    pub left_normalized_iris_mask: Vec<u8>,
    pub left_normalized_iris_image_resized: Vec<u8>,
    pub left_normalized_iris_mask_resized: Vec<u8>,
    pub right_normalized_iris_image: Vec<u8>,
    pub right_normalized_iris_mask: Vec<u8>,
    pub right_normalized_iris_image_resized: Vec<u8>,
    pub right_normalized_iris_mask_resized: Vec<u8>,
    pub left_iris_code_shares: Option<[String; 3]>,
    pub left_iris_code: Option<String>,
    pub left_mask_code_shares: Option<[String; 3]>,
    pub left_mask_code: Option<String>,
    pub right_iris_code_shares: Option<[String; 3]>,
    pub right_iris_code: Option<String>,
    pub right_mask_code_shares: Option<[String; 3]>,
    pub right_mask_code: Option<String>,
    pub iris_version: Option<String>,
}

/// The personal custody packages to be uploaded.
#[allow(missing_docs)]
pub struct PersonalCustodyPackages {
    pub tier0: Vec<u8>,
    pub tier0_checksum: Digest,
    pub tier1: Vec<u8>,
    pub tier1_checksum: Digest,
    pub tier2: Vec<u8>,
    pub tier2_checksum: Digest,
}

struct HyraxCommitments {
    left_normalized_iris_image_commitment: Vec<u8>,
    left_normalized_iris_image_blinding_factors: Vec<u8>,
    left_normalized_iris_mask_commitment: Vec<u8>,
    left_normalized_iris_mask_blinding_factors: Vec<u8>,
    right_normalized_iris_image_commitment: Vec<u8>,
    right_normalized_iris_image_blinding_factors: Vec<u8>,
    right_normalized_iris_mask_commitment: Vec<u8>,
    right_normalized_iris_mask_blinding_factors: Vec<u8>,
    left_normalized_iris_image_commitment_resized: Vec<u8>,
    left_normalized_iris_image_blinding_factors_resized: Vec<u8>,
    left_normalized_iris_mask_commitment_resized: Vec<u8>,
    left_normalized_iris_mask_blinding_factors_resized: Vec<u8>,
    right_normalized_iris_image_commitment_resized: Vec<u8>,
    right_normalized_iris_image_blinding_factors_resized: Vec<u8>,
    right_normalized_iris_mask_commitment_resized: Vec<u8>,
    right_normalized_iris_mask_blinding_factors_resized: Vec<u8>,
}

#[derive(Serialize)]
struct InfoJson<'a> {
    signup_id: &'a str,
    signup_id_salt: String,
    signup_reason: &'a str,
    signup_reason_salt: String,
    orb_id: &'a str,
    orb_id_salt: String,
    operator_id: &'a str,
    operator_id_salt: String,
    timestamp: String,
    timestamp_salt: String,
    qr_code: &'a str,
    qr_code_salt: String,
    orb_public_key_certificate: String,
    left_ir_image_id: String,
    right_ir_image_id: String,
    thumbnail_image_id: String,
    software_version: &'static str,
    software_version_salt: String,
    orb_country: String,
    orb_country_salt: String,
}

#[derive(Serialize)]
struct FaceEmbeddingsJson<'a>(Vec<SerializeWithSortedKeys<Embedding<'a>>>);

#[allow(clippy::struct_field_names)]
#[derive(Serialize)]
struct IrisCodesJson<'a> {
    #[serde(rename = "IRIS_version")]
    iris_version: Option<&'a str>,
    left_iris_code: Option<&'a str>,
    left_mask_code: Option<&'a str>,
    right_iris_code: Option<&'a str>,
    right_mask_code: Option<&'a str>,
}

#[allow(clippy::struct_field_names)]
#[derive(Serialize)]
struct IrisCodeSharesJson<'a> {
    #[serde(rename = "IRIS_version")]
    iris_version: Option<&'a str>,
    #[serde(rename = "IRIS_shares_version")]
    iris_shares_version: &'a str,
    left_iris_code_shares: String,
    left_mask_code_shares: String,
    right_iris_code_shares: String,
    right_mask_code_shares: String,
}

#[derive(Serialize)]
struct BackendKeysJson<'a> {
    iris: SerializeWithSortedKeys<BackendKey<'a>>,
    normalized_iris: SerializeWithSortedKeys<BackendKey<'a>>,
    face: SerializeWithSortedKeys<BackendKey<'a>>,
}

#[derive(Serialize)]
struct BackendKey<'a> {
    public_key: String,
    encrypted_private_key: &'a str,
}

#[allow(clippy::struct_field_names)]
#[derive(Serialize)]
struct Embedding<'a> {
    embedding: String,
    embedding_type: &'a str,
    embedding_version: &'a str,
    embedding_inference_backend: &'a str,
}

struct Package<'a> {
    ts: Duration,
    capture_start: SystemTime,
    capture: biometric_capture::Capture,
    identification_image_ids: IdentificationImages,
    // Without the `Box` here, the binary segfaults.
    pipeline: Box<Pipeline>,
    hyrax: HyraxCommitments,
    credentials: Credentials,
    signup_id: String,
    signup_reason: &'a str,
    location_data: LocationData,
}

impl Plan {
    /// Runs the plan for building and uploading the personal custody package.
    pub async fn run(self) -> Result<PersonalCustodyPackages> {
        let Self {
            capture_start,
            signup_id,
            identification_image_ids,
            capture,
            pipeline,
            credentials,
            signup_reason,
            location_data,
        } = self;
        let pipeline_box = Box::new(pipeline);

        #[cfg(feature = "internal-pcp-export")]
        let signup_id2 = signup_id.clone();
        let packages = task::spawn_blocking(move || {
            let hyrax = (&*pipeline_box).into();
            Package {
                ts: UNIX_EPOCH.elapsed()?,
                capture_start,
                capture,
                identification_image_ids,
                pipeline: pipeline_box,
                hyrax,
                credentials,
                signup_id: signup_id.to_string(),
                signup_reason: signup_reason.to_screaming_snake_case(),
                location_data,
            }
            .build()
            .map(|(tier0, tier1, tier2)| {
                let tier0_checksum = digest(&SHA256, &tier0);
                let tier1_checksum = digest(&SHA256, &tier1);
                let tier2_checksum = digest(&SHA256, &tier2);
                PersonalCustodyPackages {
                    tier0,
                    tier0_checksum,
                    tier1,
                    tier1_checksum,
                    tier2,
                    tier2_checksum,
                }
            })
        })
        .await??;

        #[cfg(feature = "internal-pcp-export")]
        {
            tokio::fs::write(
                std::path::Path::new(&format!("/tmp/pcp.{}.tier0.tar.gz", &signup_id2)),
                &packages.tier0,
            )
            .await?;
            tokio::fs::write(
                std::path::Path::new(&format!("/tmp/pcp.{}.tier1.tar.gz", &signup_id2)),
                &packages.tier1,
            )
            .await?;
            tokio::fs::write(
                std::path::Path::new(&format!("/tmp/pcp.{}.tier2.tar.gz", &signup_id2)),
                &packages.tier2,
            )
            .await?;
        }

        Ok(packages)
    }
}

impl Package<'_> {
    fn build(&self) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
        let Self { ts, ref credentials, .. } = *self;
        let Credentials {
            backend_iris_public_key,
            backend_normalized_iris_public_key,
            backend_face_public_key,
            backend_tier2_public_key,
            self_custody_user_public_key,
            pcp_version,
            ..
        } = credentials;
        let mut hashes = BTreeMap::new();
        let mut tier0 = tar::Builder::new(Vec::new());
        let mut tier1 = tar::Builder::new(Vec::new());
        let mut tier2 = tar::Builder::new(Vec::new());

        let mut iris_tar = self.make_iris_tar(&mut hashes)?;
        iris_tar = encrypt(iris_tar, backend_iris_public_key);

        let mut normalized_iris_tar = self.make_normalized_iris_tar(&mut hashes)?;
        normalized_iris_tar = encrypt(normalized_iris_tar, backend_normalized_iris_public_key);

        let mut face_tar = self.make_face_tar(&mut hashes)?;
        face_tar = encrypt(face_tar, backend_face_public_key);

        let backend_keys_json = self.make_backend_keys_json(&mut hashes)?;

        if *pcp_version >= 3 {
            tar_append(&mut tier1, ts, "iris.tar", iris_tar)?;
            tar_append(&mut tier1, ts, "normalized_iris.tar", normalized_iris_tar)?;
            tar_append(&mut tier1, ts, "face.tar", face_tar)?;
            self.make_tier2(&mut tier2)?;
        } else {
            tar_append(&mut tier0, ts, "iris.tar", iris_tar)?;
            tar_append(&mut tier0, ts, "normalized_iris.tar", normalized_iris_tar)?;
            tar_append(&mut tier0, ts, "face.tar", face_tar)?;
        }

        let tier1_compressed = compress(tier1.into_inner()?, ts, "tier1.tar.gz")?;
        let tier1_encrypted = encrypt(tier1_compressed, self_custody_user_public_key);
        let tier2_compressed = compress(tier2.into_inner()?, ts, "tier2.tar.gz")?;
        let tier2_single_encrypted = backend_tier2_public_key
            .map(|backend_tier2_public_key| encrypt(tier2_compressed, &backend_tier2_public_key))
            .unwrap_or_default();
        let tier2_encrypted = encrypt(tier2_single_encrypted, self_custody_user_public_key);

        let info_json = self.make_info_json(&mut hashes)?;
        let face_embeddings_json = self.make_face_embeddings_json(&mut hashes)?;
        let iris_codes_json = self.make_iris_codes_json(&mut hashes)?;
        let iris_code_shares_jsons = self.make_iris_code_shares_jsons(&mut hashes)?;

        let hashes_json = self.make_hashes_json(
            hashes,
            digest(&SHA256, &tier1_encrypted),
            digest(&SHA256, &tier2_encrypted),
        )?;

        tar_append(&mut tier0, ts, "info.json", info_json)?;
        tar_append(&mut tier0, ts, "face_embeddings.json", face_embeddings_json)?;
        tar_append(&mut tier0, ts, "iris_codes.json", iris_codes_json)?;
        for (i, share_json) in iris_code_shares_jsons.iter().enumerate() {
            tar_append(&mut tier0, ts, &format!("iris_code_shares_{i}.json"), share_json)?;
        }
        tar_append(&mut tier0, ts, "hashes.sign", sign(digest(&SHA256, &hashes_json))?)?;
        tar_append(&mut tier0, ts, "hashes.json", hashes_json)?;
        tar_append(&mut tier0, ts, "backend_keys.json", backend_keys_json)?;

        let tier0_compressed = compress(tier0.into_inner()?, ts, "tier0.tar.gz")?;
        let tier0_encrypted = encrypt(tier0_compressed, self_custody_user_public_key);
        Ok((tier0_encrypted, tier1_encrypted, tier2_encrypted))
    }

    fn make_iris_tar(&self, hashes: &mut BTreeMap<String, Digest>) -> Result<Vec<u8>> {
        let mut archive = tar::Builder::new(Vec::new());
        let mut left_ir_png = Vec::new();
        let mut right_ir_png = Vec::new();
        self.capture.eye_left.ir_frame.write_png(&mut left_ir_png, FrameResolution::MAX)?;
        self.capture.eye_right.ir_frame.write_png(&mut right_ir_png, FrameResolution::MAX)?;
        hashes.insert("left_ir.png".to_owned(), digest(&SHA256, &left_ir_png));
        hashes.insert("right_ir.png".to_owned(), digest(&SHA256, &right_ir_png));
        tar_append(&mut archive, self.ts, "left_ir.png", &left_ir_png)?;
        tar_append(&mut archive, self.ts, "right_ir.png", &right_ir_png)?;
        Ok(archive.into_inner()?)
    }

    #[rustfmt::skip]
    fn make_normalized_iris_tar(&self, hashes: &mut BTreeMap<String, Digest>) -> Result<Vec<u8>> {
        let Self { ts, ref pipeline, ref hyrax, .. } = *self;
        let mut archive = tar::Builder::new(Vec::new());

        // Iris normalized images.
        hashes.insert("left_normalized_image.bin".to_owned(),                   digest(&SHA256, &pipeline.left_normalized_iris_image));
        hashes.insert("left_normalized_image_commitment.bin".to_owned(),        digest(&SHA256, &hyrax.left_normalized_iris_image_commitment));
        hashes.insert("left_normalized_image_blinding_factors.bin".to_owned(),  digest(&SHA256, &hyrax.left_normalized_iris_image_blinding_factors));
        hashes.insert("left_normalized_mask.bin".to_owned(),                    digest(&SHA256, &pipeline.left_normalized_iris_mask));
        hashes.insert("left_normalized_mask_commitment.bin".to_owned(),         digest(&SHA256, &hyrax.left_normalized_iris_mask_commitment));
        hashes.insert("left_normalized_mask_blinding_factors.bin".to_owned(),   digest(&SHA256, &hyrax.left_normalized_iris_mask_blinding_factors));
        hashes.insert("right_normalized_image.bin".to_owned(),                  digest(&SHA256, &pipeline.right_normalized_iris_image));
        hashes.insert("right_normalized_image_commitment.bin".to_owned(),       digest(&SHA256, &hyrax.right_normalized_iris_image_commitment));
        hashes.insert("right_normalized_image_blinding_factors.bin".to_owned(), digest(&SHA256, &hyrax.right_normalized_iris_image_blinding_factors));
        hashes.insert("right_normalized_mask.bin".to_owned(),                   digest(&SHA256, &pipeline.right_normalized_iris_mask));
        hashes.insert("right_normalized_mask_commitment.bin".to_owned(),        digest(&SHA256, &hyrax.right_normalized_iris_mask_commitment));
        hashes.insert("right_normalized_mask_blinding_factors.bin".to_owned(),  digest(&SHA256, &hyrax.right_normalized_iris_mask_blinding_factors));
        tar_append(&mut archive, ts, "left_normalized_image.bin",                   &pipeline.left_normalized_iris_image)?;
        tar_append(&mut archive, ts, "left_normalized_image_commitment.bin",        &hyrax.left_normalized_iris_image_commitment)?;
        tar_append(&mut archive, ts, "left_normalized_image_blinding_factors.bin",  &hyrax.left_normalized_iris_image_blinding_factors)?;
        tar_append(&mut archive, ts, "left_normalized_mask.bin",                    &pipeline.left_normalized_iris_mask)?;
        tar_append(&mut archive, ts, "left_normalized_mask_commitment.bin",         &hyrax.left_normalized_iris_mask_commitment)?;
        tar_append(&mut archive, ts, "left_normalized_mask_blinding_factors.bin",   &hyrax.left_normalized_iris_mask_blinding_factors)?;
        tar_append(&mut archive, ts, "right_normalized_image.bin",                  &pipeline.right_normalized_iris_image)?;
        tar_append(&mut archive, ts, "right_normalized_image_commitment.bin",       &hyrax.right_normalized_iris_image_commitment)?;
        tar_append(&mut archive, ts, "right_normalized_image_blinding_factors.bin", &hyrax.right_normalized_iris_image_blinding_factors)?;
        tar_append(&mut archive, ts, "right_normalized_mask.bin",                   &pipeline.right_normalized_iris_mask)?;
        tar_append(&mut archive, ts, "right_normalized_mask_commitment.bin",        &hyrax.right_normalized_iris_mask_commitment)?;
        tar_append(&mut archive, ts, "right_normalized_mask_blinding_factors.bin",  &hyrax.right_normalized_iris_mask_blinding_factors)?;

        // Resized Iris normalized images.
        hashes.insert("left_normalized_image_resized.bin".to_owned(),                   digest(&SHA256, &pipeline.left_normalized_iris_image_resized));
        hashes.insert("left_normalized_image_commitment_resized.bin".to_owned(),        digest(&SHA256, &hyrax.left_normalized_iris_image_commitment_resized));
        hashes.insert("left_normalized_image_blinding_factors_resized.bin".to_owned(),  digest(&SHA256, &hyrax.left_normalized_iris_image_blinding_factors_resized));
        hashes.insert("left_normalized_mask_resized.bin".to_owned(),                    digest(&SHA256, &pipeline.left_normalized_iris_mask_resized));
        hashes.insert("left_normalized_mask_commitment_resized.bin".to_owned(),         digest(&SHA256, &hyrax.left_normalized_iris_mask_commitment_resized));
        hashes.insert("left_normalized_mask_blinding_factors_resized.bin".to_owned(),   digest(&SHA256, &hyrax.left_normalized_iris_mask_blinding_factors_resized));
        hashes.insert("right_normalized_image_resized.bin".to_owned(),                  digest(&SHA256, &pipeline.right_normalized_iris_image_resized));
        hashes.insert("right_normalized_image_commitment_resized.bin".to_owned(),       digest(&SHA256, &hyrax.right_normalized_iris_image_commitment_resized));
        hashes.insert("right_normalized_image_blinding_factors_resized.bin".to_owned(), digest(&SHA256, &hyrax.right_normalized_iris_image_blinding_factors_resized));
        hashes.insert("right_normalized_mask_resized.bin".to_owned(),                   digest(&SHA256, &pipeline.right_normalized_iris_mask_resized));
        hashes.insert("right_normalized_mask_commitment_resized.bin".to_owned(),        digest(&SHA256, &hyrax.right_normalized_iris_mask_commitment_resized));
        hashes.insert("right_normalized_mask_blinding_factors_resized.bin".to_owned(),  digest(&SHA256, &hyrax.right_normalized_iris_mask_blinding_factors_resized));
        tar_append(&mut archive, ts, "left_normalized_image_resized.bin",                   &pipeline.left_normalized_iris_image_resized)?;
        tar_append(&mut archive, ts, "left_normalized_image_commitment_resized.bin",        &hyrax.left_normalized_iris_image_commitment_resized)?;
        tar_append(&mut archive, ts, "left_normalized_image_blinding_factors_resized.bin",  &hyrax.left_normalized_iris_image_blinding_factors_resized)?;
        tar_append(&mut archive, ts, "left_normalized_mask_resized.bin",                    &pipeline.left_normalized_iris_mask_resized)?;
        tar_append(&mut archive, ts, "left_normalized_mask_commitment_resized.bin",         &hyrax.left_normalized_iris_mask_commitment_resized)?;
        tar_append(&mut archive, ts, "left_normalized_mask_blinding_factors_resized.bin",   &hyrax.left_normalized_iris_mask_blinding_factors_resized)?;
        tar_append(&mut archive, ts, "right_normalized_image_resized.bin",                  &pipeline.right_normalized_iris_image_resized)?;
        tar_append(&mut archive, ts, "right_normalized_image_commitment_resized.bin",       &hyrax.right_normalized_iris_image_commitment_resized)?;
        tar_append(&mut archive, ts, "right_normalized_image_blinding_factors_resized.bin", &hyrax.right_normalized_iris_image_blinding_factors_resized)?;
        tar_append(&mut archive, ts, "right_normalized_mask_resized.bin",                   &pipeline.right_normalized_iris_mask_resized)?;
        tar_append(&mut archive, ts, "right_normalized_mask_commitment_resized.bin",        &hyrax.right_normalized_iris_mask_commitment_resized)?;
        tar_append(&mut archive, ts, "right_normalized_mask_blinding_factors_resized.bin",  &hyrax.right_normalized_iris_mask_blinding_factors_resized)?;

        Ok(archive.into_inner()?)
    }

    fn make_face_tar(&self, hashes: &mut BTreeMap<String, Digest>) -> Result<Vec<u8>> {
        let mut archive = tar::Builder::new(Vec::new());
        let thumbnail_png = self.make_face_thumbnail_png()?;
        hashes.insert("thumbnail.png".to_owned(), digest(&SHA256, &thumbnail_png));
        tar_append(&mut archive, self.ts, "thumbnail.png", &thumbnail_png)?;
        Ok(archive.into_inner()?)
    }

    fn make_face_thumbnail_png(&self) -> Result<Vec<u8>> {
        let image = &self.pipeline.face_identifier_thumbnail_image;
        let [height, width, depth] = image.shape() else {
            bail!("Unsupported face identifier thumbnail image shape: {:?}", image.shape());
        };
        ensure!(*depth == 3, "Unsupported face identifier thumbnail image pixel format: {depth}");
        let mut buf = Vec::new();
        {
            let mut encoder = png::Encoder::new(
                &mut buf,
                u32::try_from(*width).unwrap(),
                u32::try_from(*height).unwrap(),
            );
            encoder.set_color(png::ColorType::RGB);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::Fast);
            let mut writer = encoder.write_header()?;
            writer.write_image_data(image.as_standard_layout().as_slice().unwrap())?;
        }
        Ok(buf)
    }

    #[allow(clippy::too_many_lines)]
    fn make_info_json(&self, hashes: &mut BTreeMap<String, Digest>) -> Result<Vec<u8>> {
        fn salted_sha256(value: impl AsRef<str>, salt: impl AsRef<str>) -> Digest {
            digest(&SHA256, format!("{}{}", value.as_ref(), salt.as_ref()).as_ref())
        }
        let Self { credentials, signup_id, signup_reason, .. } = self;
        let Credentials { operator_qr_code, user_qr_code_string, .. } = credentials;
        let signup_id_salt = gen_salt();
        let signup_reason_salt = gen_salt();
        let orb_id_salt = gen_salt();
        let operator_id_salt = gen_salt();
        let timestamp_salt = gen_salt();
        let qr_code_salt = gen_salt();
        let orb_id = ORB_ID.as_str();
        let timestamp = self
            .capture_start
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();
        let orb_public_key_certificate = BASE64.encode(&ORB_PUBLIC_KEY);
        let left_ir_image_id = self.identification_image_ids.left_ir.to_string();
        let right_ir_image_id = self.identification_image_ids.right_ir.to_string();
        let thumbnail_image_id = self.identification_image_ids.self_custody_candidate.to_string();
        let software_version = &**ORB_OS_VERSION;
        let orb_country = self.location_data.operator_team_operating_country.clone();
        hashes.insert("signup_id".to_owned(), salted_sha256(signup_id, &signup_id_salt));
        hashes
            .insert("signup_reason".to_owned(), salted_sha256(signup_reason, &signup_reason_salt));
        hashes.insert("orb_id".to_owned(), salted_sha256(orb_id, &orb_id_salt));
        hashes.insert(
            "operator_id".to_owned(),
            salted_sha256(&operator_qr_code.user_id, &operator_id_salt),
        );
        hashes.insert("timestamp".to_owned(), salted_sha256(&timestamp, &timestamp_salt));
        hashes.insert("qr_code".to_owned(), salted_sha256(user_qr_code_string, &qr_code_salt));
        let software_version_salt = {
            let salt = gen_salt();
            hashes.insert("software_version".to_owned(), salted_sha256(software_version, &salt));
            salt
        };
        let orb_country_salt = {
            let salt = gen_salt();
            hashes.insert("orb_country".to_owned(), salted_sha256(&orb_country, &salt));
            salt
        };

        let info = InfoJson {
            signup_id,
            signup_id_salt,
            signup_reason,
            signup_reason_salt,
            orb_id,
            orb_id_salt,
            operator_id: &operator_qr_code.user_id,
            operator_id_salt,
            timestamp,
            timestamp_salt,
            qr_code: user_qr_code_string,
            qr_code_salt,
            orb_public_key_certificate,
            left_ir_image_id,
            right_ir_image_id,
            thumbnail_image_id,
            software_version,
            software_version_salt,
            orb_country,
            orb_country_salt,
        };
        let json = serde_json::to_string(&SerializeWithSortedKeys(&info))
            .wrap_err("serializing InfoJson as json")?;
        Ok(json.into_bytes())
    }

    fn make_face_embeddings_json(&self, hashes: &mut BTreeMap<String, Digest>) -> Result<Vec<u8>> {
        let embeddings = self
            .pipeline
            .face_identifier_embeddings
            .iter()
            .map(|embedding| {
                let face_identifier::types::Embedding {
                    embedding,
                    embedding_type,
                    embedding_version,
                    embedding_inference_backend,
                } = embedding;
                let embedding = embedding
                    .as_ndarray()
                    .as_standard_layout()
                    .as_slice()
                    .unwrap()
                    .iter()
                    .flat_map(|x| x.to_be_bytes())
                    .collect::<Vec<_>>();
                SerializeWithSortedKeys(Embedding {
                    embedding: BASE64.encode(&embedding),
                    embedding_type,
                    embedding_version,
                    embedding_inference_backend,
                })
            })
            .collect();
        let face_embeddings = FaceEmbeddingsJson(embeddings);
        let json = serde_json::to_string(&face_embeddings)
            .wrap_err("serializing FaceEmbeddingsJson as json")?
            .into_bytes();
        hashes.insert("face_embeddings.json".to_owned(), digest(&SHA256, &json));
        Ok(json)
    }

    fn make_iris_codes_json(&self, hashes: &mut BTreeMap<String, Digest>) -> Result<Vec<u8>> {
        let iris_codes = IrisCodesJson {
            iris_version: self.pipeline.iris_version.as_deref(),
            left_iris_code: self.pipeline.left_iris_code.as_deref(),
            left_mask_code: self.pipeline.left_mask_code.as_deref(),
            right_iris_code: self.pipeline.right_iris_code.as_deref(),
            right_mask_code: self.pipeline.right_mask_code.as_deref(),
        };
        let json = serde_json::to_string(&SerializeWithSortedKeys(&iris_codes))
            .wrap_err("serializing IrisCodesJson as json")?
            .into_bytes();
        hashes.insert("iris_codes.json".to_owned(), digest(&SHA256, &json));
        Ok(json)
    }

    fn make_iris_code_shares_jsons(
        &self,
        hashes: &mut BTreeMap<String, Digest>,
    ) -> Result<Vec<Vec<u8>>> {
        let mut iris_code_shares_jsons = Vec::new();

        // TODO: Should we produce a PCP if we don't have all the shares? This can happen if we detect fraud or some
        // other issue.
        let (
            Some(left_iris_code_shares),
            Some(left_mask_code_shares),
            Some(right_iris_code_shares),
            Some(right_mask_code_shares),
        ) = (
            &self.pipeline.left_iris_code_shares,
            &self.pipeline.left_mask_code_shares,
            &self.pipeline.right_iris_code_shares,
            &self.pipeline.right_mask_code_shares,
        )
        else {
            bail!("Missing Iris and mask code shares");
        };

        for (i, ((li, lm), (ri, rm))) in left_iris_code_shares
            .iter()
            .zip(left_mask_code_shares.iter())
            .zip(right_iris_code_shares.iter().zip(right_mask_code_shares.iter()))
            .enumerate()
        {
            let iris_code_shares = IrisCodeSharesJson {
                iris_version: self.pipeline.iris_version.as_deref(),
                iris_shares_version: IRIS_MPC_VERSION,
                left_iris_code_shares: li.clone(),
                left_mask_code_shares: lm.clone(),
                right_iris_code_shares: ri.clone(),
                right_mask_code_shares: rm.clone(),
            };
            let json = serde_json::to_string(&SerializeWithSortedKeys(&iris_code_shares))
                .wrap_err("serializing IrisCodeSharesJson as json")?
                .into_bytes();
            hashes.insert(format!("iris_code_shares_{i}.json"), digest(&SHA256, &json));

            iris_code_shares_jsons.push(json);
        }

        Ok(iris_code_shares_jsons)
    }

    fn make_backend_keys_json(&self, hashes: &mut BTreeMap<String, Digest>) -> Result<Vec<u8>> {
        let backend_keys = BackendKeysJson {
            iris: SerializeWithSortedKeys(BackendKey {
                public_key: BASE64.encode(self.credentials.backend_iris_public_key.as_ref()),
                encrypted_private_key: &self.credentials.backend_iris_encrypted_private_key,
            }),
            normalized_iris: SerializeWithSortedKeys(BackendKey {
                public_key: BASE64
                    .encode(self.credentials.backend_normalized_iris_public_key.as_ref()),
                encrypted_private_key: &self
                    .credentials
                    .backend_normalized_iris_encrypted_private_key,
            }),
            face: SerializeWithSortedKeys(BackendKey {
                public_key: BASE64.encode(self.credentials.backend_face_public_key.as_ref()),
                encrypted_private_key: &self.credentials.backend_face_encrypted_private_key,
            }),
        };
        let json = serde_json::to_string(&SerializeWithSortedKeys(&backend_keys))
            .wrap_err("serializing BackendKeysJson as json")?
            .into_bytes();
        hashes.insert("backend_keys.json".to_owned(), digest(&SHA256, &json));
        Ok(json)
    }

    fn make_tier2(&self, archive: &mut tar::Builder<Vec<u8>>) -> Result<()> {
        if let Some(face_ir) = &self.capture.face_ir {
            let mut face_ir_png = Vec::new();
            face_ir.write_png(&mut face_ir_png, FrameResolution::MAX)?;
            tar_append(archive, self.ts, "face_ir.png", &face_ir_png)?;
        }
        if let Some(thermal) = &self.capture.thermal {
            let mut thermal_png = Vec::new();
            thermal.write_png(&mut thermal_png, FrameResolution::MAX)?;
            tar_append(archive, self.ts, "thermal.png", &thermal_png)?;
        }
        Ok(())
    }

    fn make_hashes_json(
        &self,
        hashes: BTreeMap<String, Digest>,
        tier1_hash: Digest,
        tier2_hash: Digest,
    ) -> Result<Vec<u8>> {
        const ABSENT_TIER: &str =
            "0000000000000000000000000000000000000000000000000000000000000000";
        let mut hex_hashes = BTreeMap::new();
        for (key, hash) in hashes {
            hex_hashes.insert(key, HEXLOWER.encode(hash.as_ref()));
        }
        if self.credentials.pcp_version >= 3 {
            hex_hashes.insert("version".to_owned(), VERSION_V3.to_string());
            hex_hashes.insert("tier_1".to_owned(), HEXLOWER.encode(tier1_hash.as_ref()));
            hex_hashes.insert("tier_2".to_owned(), HEXLOWER.encode(tier2_hash.as_ref()));
            hex_hashes.insert("tier_3".to_owned(), ABSENT_TIER.into());
            hex_hashes.insert("tier_4".to_owned(), ABSENT_TIER.into());
            hex_hashes.insert("tier_5".to_owned(), ABSENT_TIER.into());
        } else {
            hex_hashes.insert("version".to_owned(), VERSION_V2.to_string());
        }
        let hashes_json =
            serde_json::to_string(&hex_hashes).wrap_err("serializing hashes.json")?.into_bytes();
        Ok(hashes_json)
    }
}

impl From<&Pipeline> for HyraxCommitments {
    fn from(pipeline: &Pipeline) -> Self {
        let (left_normalized_iris_image_commitment, left_normalized_iris_image_blinding_factors) =
            compute_hyrax_commitment(&pipeline.left_normalized_iris_image);
        let (left_normalized_iris_mask_commitment, left_normalized_iris_mask_blinding_factors) =
            compute_hyrax_commitment(&pipeline.left_normalized_iris_mask);
        let (right_normalized_iris_image_commitment, right_normalized_iris_image_blinding_factors) =
            compute_hyrax_commitment(&pipeline.right_normalized_iris_image);
        let (right_normalized_iris_mask_commitment, right_normalized_iris_mask_blinding_factors) =
            compute_hyrax_commitment(&pipeline.right_normalized_iris_mask);
        let (
            left_normalized_iris_image_commitment_resized,
            left_normalized_iris_image_blinding_factors_resized,
        ) = compute_hyrax_commitment(&pipeline.left_normalized_iris_image_resized);
        let (
            left_normalized_iris_mask_commitment_resized,
            left_normalized_iris_mask_blinding_factors_resized,
        ) = compute_hyrax_commitment(&pipeline.left_normalized_iris_mask_resized);
        let (
            right_normalized_iris_image_commitment_resized,
            right_normalized_iris_image_blinding_factors_resized,
        ) = compute_hyrax_commitment(&pipeline.right_normalized_iris_image_resized);
        let (
            right_normalized_iris_mask_commitment_resized,
            right_normalized_iris_mask_blinding_factors_resized,
        ) = compute_hyrax_commitment(&pipeline.right_normalized_iris_mask_resized);
        Self {
            left_normalized_iris_image_commitment,
            left_normalized_iris_image_blinding_factors,
            left_normalized_iris_mask_commitment,
            left_normalized_iris_mask_blinding_factors,
            right_normalized_iris_image_commitment,
            right_normalized_iris_image_blinding_factors,
            right_normalized_iris_mask_commitment,
            right_normalized_iris_mask_blinding_factors,
            left_normalized_iris_image_commitment_resized,
            left_normalized_iris_image_blinding_factors_resized,
            left_normalized_iris_mask_commitment_resized,
            left_normalized_iris_mask_blinding_factors_resized,
            right_normalized_iris_image_commitment_resized,
            right_normalized_iris_image_blinding_factors_resized,
            right_normalized_iris_mask_commitment_resized,
            right_normalized_iris_mask_blinding_factors_resized,
        }
    }
}

fn compute_hyrax_commitment(normalized_iris_image: impl AsRef<[u8]>) -> (Vec<u8>, Vec<u8>) {
    let blinding_factor_seed = random::<[u8; 32]>();
    let output =
        compute_commitments_binary_outputs(normalized_iris_image.as_ref(), blinding_factor_seed);
    let HyraxCommitmentOutputSerialized { commitment_serialized, blinding_factors_serialized } =
        output;
    (commitment_serialized, blinding_factors_serialized)
}

fn tar_append<T: AsRef<[u8]>>(
    archive: &mut tar::Builder<Vec<u8>>,
    ts: Duration,
    path: &str,
    data: T,
) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_device_major(0).unwrap();
    header.set_device_minor(0).unwrap();
    header.set_path(path).unwrap();
    header.set_size(data.as_ref().len().try_into().unwrap());
    header.set_mtime(ts.as_secs());
    header.set_uid(0);
    header.set_gid(0);
    header.set_mode(0o644);
    header.set_cksum();
    archive.append(&header, data.as_ref())?;
    Ok(())
}

#[cfg(not(feature = "internal-pcp-no-encryption"))]
fn encrypt<T: AsRef<[u8]>>(data: T, public_key: &PublicKey) -> Vec<u8> {
    let encrypted = sodiumoxide::crypto::sealedbox::seal(data.as_ref(), public_key);
    assert_ne!(data.as_ref(), encrypted);
    encrypted
}

#[cfg(feature = "internal-pcp-no-encryption")]
fn encrypt<T: AsRef<[u8]>>(data: T, _public_key: &PublicKey) -> Vec<u8> {
    data.as_ref().to_vec()
}

fn compress<T: AsRef<[u8]>>(data: T, ts: Duration, filename: &str) -> Result<Vec<u8>> {
    let mut encoder = GzBuilder::new()
        .filename(filename)
        .mtime(ts.as_secs().try_into().unwrap())
        .write(Vec::new(), flate2::Compression::best());
    encoder.write_all(data.as_ref())?;
    Ok(encoder.finish()?)
}

/// Generates a random string like "273bcdb626d78650a3e22920eea00573", which
/// represents 16 bytes.
fn gen_salt() -> String {
    random::<[u8; 16]>().into_iter().fold(String::new(), |mut acc, x| {
        acc.push_str(&format!("{x:02x}"));
        acc
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        agents::camera::rgb,
        backend::operator_status::Coordinates,
        secure_element::{get_private_pem, get_public_pem},
    };
    use ndarray::Array;
    use sodiumoxide::crypto::box_::gen_keypair;
    use std::{env, fs, path::Path};

    fn dummy_credentials(
        backend_iris_public_key: PublicKey,
        backend_normalized_iris_public_key: PublicKey,
        backend_face_public_key: PublicKey,
        backend_tier2_public_key: PublicKey,
        self_custody_user_public_key: PublicKey,
        pcp_version: u16,
    ) -> Credentials {
        Credentials {
            operator_qr_code: qr_scan::user::Data {
                user_id: "dummy_user_id".into(),
                ..Default::default()
            },
            user_qr_code: qr_scan::user::Data {
                user_id: "dummy_operator_id".into(),
                ..Default::default()
            },
            user_qr_code_string: "dummy_qr_code".into(),
            backend_iris_public_key,
            backend_iris_encrypted_private_key: "dummy_encrypted_key".to_string(),
            backend_normalized_iris_public_key,
            backend_normalized_iris_encrypted_private_key: "dummy_encrypted_key".to_string(),
            backend_face_public_key,
            backend_face_encrypted_private_key: "dummy_encrypted_key".to_string(),
            backend_tier2_public_key: Some(backend_tier2_public_key),
            backend_tier2_encrypted_private_key: Some("dummy_encrypted_key".to_string()),
            self_custody_user_public_key,
            pcp_version,
        }
    }

    fn dummy_pipeline() -> Pipeline {
        Pipeline {
            face_identifier_thumbnail_image: rgb::Frame::from_vec(
                vec![1; 3],
                Duration::new(1, 1),
                1,
                1,
            )
            .into_ndarray(),
            face_identifier_embeddings: vec![face_identifier::types::Embedding {
                embedding: Array::ones(10).into(),
                ..Default::default()
            }],
            face_identifier_inference_backend: "dummy inference backend".into(),
            left_normalized_iris_image: vec![0; 10],
            left_normalized_iris_mask: vec![0; 10],
            left_normalized_iris_image_resized: vec![0; 10],
            left_normalized_iris_mask_resized: vec![0; 10],
            right_normalized_iris_image: vec![0; 10],
            right_normalized_iris_mask: vec![0; 10],
            right_normalized_iris_image_resized: vec![0; 10],
            right_normalized_iris_mask_resized: vec![0; 10],
            left_iris_code_shares: Some([
                "dummy_left_iris_code_shares_0".to_owned(),
                "dummy_left_iris_code_shares_1".to_owned(),
                "dummy_left_iris_code_shares_2".to_owned(),
            ]),
            left_iris_code: Some("test_left_iris_code".to_owned()),
            left_mask_code_shares: Some([
                "dummy_left_mask_code_shares_0".to_owned(),
                "dummy_left_mask_code_shares_1".to_owned(),
                "dummy_left_mask_code_shares_2".to_owned(),
            ]),
            left_mask_code: Some("test_left_mask_code".to_owned()),
            right_iris_code_shares: Some([
                "dummy_right_iris_code_shares_0".to_owned(),
                "dummy_right_iris_code_shares_1".to_owned(),
                "dummy_right_iris_code_shares_2".to_owned(),
            ]),
            right_iris_code: Some("test_right_iris_code".to_owned()),
            right_mask_code_shares: Some([
                "dummy_right_mask_code_shares_0".to_owned(),
                "dummy_right_mask_code_shares_1".to_owned(),
                "dummy_right_mask_code_shares_2".to_owned(),
            ]),
            right_mask_code: Some("test_right_mask_code".to_owned()),
            iris_version: None,
        }
    }

    fn dummy_hyrax() -> HyraxCommitments {
        HyraxCommitments {
            left_normalized_iris_image_commitment: vec![0; 10],
            left_normalized_iris_image_blinding_factors: vec![0; 10],
            left_normalized_iris_mask_commitment: vec![0; 10],
            left_normalized_iris_mask_blinding_factors: vec![0; 10],
            right_normalized_iris_image_commitment: vec![0; 10],
            right_normalized_iris_image_blinding_factors: vec![0; 10],
            right_normalized_iris_mask_commitment: vec![0; 10],
            right_normalized_iris_mask_blinding_factors: vec![0; 10],
            left_normalized_iris_image_commitment_resized: vec![0; 10],
            left_normalized_iris_image_blinding_factors_resized: vec![0; 10],
            left_normalized_iris_mask_commitment_resized: vec![0; 10],
            left_normalized_iris_mask_blinding_factors_resized: vec![0; 10],
            right_normalized_iris_image_commitment_resized: vec![0; 10],
            right_normalized_iris_image_blinding_factors_resized: vec![0; 10],
            right_normalized_iris_mask_commitment_resized: vec![0; 10],
            right_normalized_iris_mask_blinding_factors_resized: vec![0; 10],
        }
    }

    /// Generates a dummy PCPv3 for testing purposes.
    #[test]
    fn test_generate_dummy_personal_custody_package_v3() {
        // Mock or dummy data for Package struct
        let (self_custody_user_public_key, self_custody_user_private_key) = gen_keypair();
        let (backend_iris_public_key, backend_iris_private_key) = gen_keypair();
        let (backend_face_public_key, backend_face_private_key) = gen_keypair();
        let (backend_tier2_public_key, backend_tier2_private_key) = gen_keypair();
        let (backend_normalized_iris_public_key, backend_normalized_iris_private_key) =
            gen_keypair();
        let credentials = dummy_credentials(
            backend_iris_public_key,
            backend_normalized_iris_public_key,
            backend_face_public_key,
            backend_tier2_public_key,
            self_custody_user_public_key,
            3,
        );
        let pipeline = Box::new(dummy_pipeline());
        let hyrax = dummy_hyrax();
        let package = Package {
            ts: Duration::new(1, 1),
            capture_start: SystemTime::now(),
            capture: biometric_capture::Capture::default(),
            identification_image_ids: IdentificationImages::default(),
            pipeline,
            hyrax,
            credentials,
            signup_id: "dummy_signup_id".to_string(),
            signup_reason: "DummyReason",
            location_data: LocationData {
                operator_team_operating_country: "Dummy".into(),
                operator_session_coordinates: Coordinates { latitude: 0.0f64, longitude: 0.0f64 },
                operator_stationary_location_coordinates: None,
                operation_country: Some("Dummy".into()),
                operation_city: Some("Dummy".into()),
                ip_country: Some("Dummy".into()),
                ip_city: Some("Dummy".into()),
            },
        };

        // Call the build method
        let (tier0, tier1, tier2) = package.build().expect("to be able to build the package");
        let dir = Path::new(&env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("target")
            .join("test_personal_custody_package_v3");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("tier0.tar.gz"), tier0).unwrap();
        fs::write(dir.join("tier1.tar.gz"), tier1).unwrap();
        fs::write(dir.join("tier2.tar.gz"), tier2).unwrap();
        fs::write(dir.join("user_public_key"), self_custody_user_public_key).unwrap();
        fs::write(dir.join("user_private_key"), self_custody_user_private_key).unwrap();
        fs::write(dir.join("backend_iris_public_key"), backend_iris_public_key).unwrap();
        fs::write(dir.join("backend_iris_private_key"), backend_iris_private_key).unwrap();
        fs::write(dir.join("backend_face_public_key"), backend_face_public_key).unwrap();
        fs::write(dir.join("backend_face_private_key"), backend_face_private_key).unwrap();
        fs::write(dir.join("backend_tier2_public_key"), backend_tier2_public_key).unwrap();
        fs::write(dir.join("backend_tier2_private_key"), backend_tier2_private_key).unwrap();
        fs::write(
            dir.join("backend_normalized_iris_public_key"),
            backend_normalized_iris_public_key,
        )
        .unwrap();
        fs::write(
            dir.join("backend_normalized_iris_private_key"),
            backend_normalized_iris_private_key,
        )
        .unwrap();
        fs::write(dir.join("orb_secure_element_private_secp256k1.pem"), get_private_pem().unwrap())
            .unwrap();
        fs::write(dir.join("orb_secure_element_public_secp256k1.pem"), get_public_pem().unwrap())
            .unwrap();
    }

    /// Generates a dummy PCPv2 for testing purposes.
    #[test]
    fn test_generate_dummy_personal_custody_package_v2() {
        // Mock or dummy data for Package struct
        let (self_custody_user_public_key, self_custody_user_private_key) = gen_keypair();
        let (backend_iris_public_key, backend_iris_private_key) = gen_keypair();
        let (backend_face_public_key, backend_face_private_key) = gen_keypair();
        let (backend_tier2_public_key, backend_tier2_private_key) = gen_keypair();
        let (backend_normalized_iris_public_key, backend_normalized_iris_private_key) =
            gen_keypair();
        let credentials = dummy_credentials(
            backend_iris_public_key,
            backend_normalized_iris_public_key,
            backend_face_public_key,
            backend_tier2_public_key,
            self_custody_user_public_key,
            2,
        );
        let pipeline = Box::new(dummy_pipeline());
        let hyrax = dummy_hyrax();
        let package = Package {
            ts: Duration::new(1, 1),
            capture_start: SystemTime::now(),
            capture: biometric_capture::Capture::default(),
            identification_image_ids: IdentificationImages::default(),
            pipeline,
            hyrax,
            credentials,
            signup_id: "dummy_signup_id".to_string(),
            signup_reason: "DummyReason",
            location_data: LocationData {
                operator_team_operating_country: "Dummy".into(),
                operator_session_coordinates: Coordinates { latitude: 0.0f64, longitude: 0.0f64 },
                operator_stationary_location_coordinates: None,
                operation_country: Some("Dummy".into()),
                operation_city: Some("Dummy".into()),
                ip_country: Some("Dummy".into()),
                ip_city: Some("Dummy".into()),
            },
        };

        // Call the build method
        let (tier0, tier1, tier2) = package.build().expect("to be able to build the package");
        let dir = Path::new(&env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("target")
            .join("test_personal_custody_package_v2");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("tier0.tar.gz"), tier0).unwrap();
        fs::write(dir.join("tier1.tar.gz"), tier1).unwrap();
        fs::write(dir.join("tier2.tar.gz"), tier2).unwrap();
        fs::write(dir.join("user_public_key"), self_custody_user_public_key).unwrap();
        fs::write(dir.join("user_private_key"), self_custody_user_private_key).unwrap();
        fs::write(dir.join("backend_iris_public_key"), backend_iris_public_key).unwrap();
        fs::write(dir.join("backend_iris_private_key"), backend_iris_private_key).unwrap();
        fs::write(dir.join("backend_face_public_key"), backend_face_public_key).unwrap();
        fs::write(dir.join("backend_face_private_key"), backend_face_private_key).unwrap();
        fs::write(dir.join("backend_tier2_public_key"), backend_tier2_public_key).unwrap();
        fs::write(dir.join("backend_tier2_private_key"), backend_tier2_private_key).unwrap();
        fs::write(
            dir.join("backend_normalized_iris_public_key"),
            backend_normalized_iris_public_key,
        )
        .unwrap();
        fs::write(
            dir.join("backend_normalized_iris_private_key"),
            backend_normalized_iris_private_key,
        )
        .unwrap();
        fs::write(dir.join("orb_secure_element_private_secp256k1.pem"), get_private_pem().unwrap())
            .unwrap();
        fs::write(dir.join("orb_secure_element_public_secp256k1.pem"), get_public_pem().unwrap())
            .unwrap();
    }
}
