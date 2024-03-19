//! Upload identification images for self-custody.

use super::{biometric_capture, biometric_pipeline, qr_scan};
use crate::{
    agents::{
        camera::{Frame as _, FrameResolution},
        python::{face_identifier, iris::NormalizedIris},
    },
    backend::{self, signup_post::SignupReason, user_status::UserData},
    brokers::Orb,
    identification::ORB_ID,
    secure_element::sign,
};
use data_encoding::{BASE64, HEXLOWER};
use eyre::{bail, ensure, Result, WrapErr};
use flate2::GzBuilder;
use orb_wld_data_id::SignupId;
use ring::digest::{digest, SHA256};
use rs_merkle::{algorithms::Sha256, Hasher, MerkleTree};
use serde::Serialize;
use sodiumoxide::crypto::{box_::PublicKey, sealedbox};
use std::{
    io::prelude::*,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use tokio::task;

const VERSION: &str = "1.0";

/// Upload identification images for self-custody plan.
#[allow(missing_docs)]
pub struct Plan {
    pub capture_start: SystemTime,
    pub signup_id: SignupId,
    pub capture: biometric_capture::Capture,
    pub pipeline: biometric_pipeline::Pipeline,
    pub operator_qr_code: qr_scan::user::Data,
    pub user_qr_code: qr_scan::user::Data,
    pub user_data: UserData,
    pub signup_reason: SignupReason,
}

#[derive(Serialize)]
struct Bundle<'a> {
    version: &'a str,
    embeddings: Vec<Embedding<'a>>,
    inference_backend: &'a str,
    signup_id: &'a str,
    orb_id: &'a str,
    operator_id: &'a str,
    timestamp: &'a str,
    backend_keys: BackendKeys<'a>,
    signup_reason: &'a str,
}

#[derive(Serialize)]
struct BackendKeys<'a> {
    iris: BackendKey<'a>,
    normalized_iris: BackendKey<'a>,
    face: BackendKey<'a>,
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
    capture_start: SystemTime,
    capture: biometric_capture::Capture,
    bundle: face_identifier::Bundle,
    left_normalized_iris_image: NormalizedIris,
    right_normalized_iris_image: NormalizedIris,
    backend_iris_public_key: PublicKey,
    backend_iris_encrypted_private_key: String,
    backend_normalized_iris_public_key: PublicKey,
    backend_normalized_iris_encrypted_private_key: String,
    backend_face_public_key: PublicKey,
    backend_face_encrypted_private_key: String,
    user_public_key: PublicKey,
    operator_id: &'a str,
    signup_id: String,
    signup_reason: &'a str,
}

impl Plan {
    /// Runs the user enrollment plan.
    ///
    /// # Panics
    ///
    /// * If `user_data.self_custody_user_public_key` is `None`
    /// * If `user_data.backend_iris_public_key` is `None`
    /// * If `user_data.backend_iris_encrypted_private_key` is `None`
    /// * If `user_data.backend_normalized_iris_public_key` is `None`
    /// * If `user_data.backend_normalized_iris_encrypted_private_key` is `None`
    /// * If `user_data.backend_face_public_key` is `None`
    /// * If `user_data.backend_face_encrypted_private_key` is `None`
    /// * If `pipeline.face_identifier_bundle` is `Err`
    /// * If `pipeline.face_identifier_bundle.thumbnail` is `None`
    /// * If `pipeline.face_identifier_bundle.embeddings` is `None`
    /// * If `pipeline.face_identifier_bundle.inference_backend` is `None`
    /// * If `pipeline.v2.eye_left.iris_normalized_image` is `None`
    /// * If `pipeline.v2.eye_right.iris_normalized_image` is `None`
    pub async fn run(self, _orb: &mut Orb) -> Result<()> {
        let Self {
            capture_start,
            signup_id,
            capture,
            pipeline,
            operator_qr_code,
            user_qr_code,
            user_data,
            signup_reason,
        } = self;
        let user_public_key =
            user_data.self_custody_user_public_key.expect("to be provided by the backend");
        let backend_iris_public_key =
            user_data.backend_iris_public_key.expect("to be provided by the backend");
        let backend_iris_encrypted_private_key =
            user_data.backend_iris_encrypted_private_key.expect("to be provided by the backend");
        let backend_normalized_iris_public_key =
            user_data.backend_normalized_iris_public_key.expect("to be provided by the backend");
        let backend_normalized_iris_encrypted_private_key = user_data
            .backend_normalized_iris_encrypted_private_key
            .expect("to be provided by the backend");
        let backend_face_public_key =
            user_data.backend_face_public_key.expect("to be provided by the backend");
        let backend_face_encrypted_private_key =
            user_data.backend_face_encrypted_private_key.expect("to be provided by the backend");
        let bundle = pipeline.face_identifier_bundle.expect("to be provided by the pipeline");
        let left_normalized_iris_image =
            pipeline.v2.eye_left.iris_normalized_image.expect("to be provided by the pipeline");
        let right_normalized_iris_image =
            pipeline.v2.eye_right.iris_normalized_image.expect("to be provided by the pipeline");

        let (package, checksum, signup_id) = task::spawn_blocking(move || {
            Package {
                capture_start,
                capture,
                bundle,
                left_normalized_iris_image,
                right_normalized_iris_image,
                backend_iris_public_key,
                backend_iris_encrypted_private_key,
                backend_normalized_iris_public_key,
                backend_normalized_iris_encrypted_private_key,
                backend_face_public_key,
                backend_face_encrypted_private_key,
                user_public_key,
                operator_id: &operator_qr_code.user_id,
                signup_id: signup_id.to_string(),
                signup_reason: signup_reason.to_screaming_snake_case(),
            }
            .build()
            .map(|package| {
                let checksum = digest(&SHA256, &package);
                (package, checksum, signup_id)
            })
        })
        .await??;

        backend::upload_self_custody_images::request(
            &signup_id,
            &user_qr_code.user_id,
            &HEXLOWER.encode(checksum.as_ref()),
            package,
        )
        .await?;

        Ok(())
    }
}

impl Package<'_> {
    fn build(&self) -> Result<Vec<u8>> {
        let mut archive = tar::Builder::new(Vec::new());
        let ts = UNIX_EPOCH.elapsed()?;
        let (mut iris_tar, iris_merkle) = self.make_iris_tar_and_merkle(ts)?;
        iris_tar = encrypt(iris_tar, &self.backend_iris_public_key);
        let (mut normalized_iris_tar, normalized_iris_merkle) =
            self.make_normalized_iris_tar_and_merkle(ts)?;
        normalized_iris_tar =
            encrypt(normalized_iris_tar, &self.backend_normalized_iris_public_key);
        let (mut face_tar, face_merkle) = self.make_face_tar_and_merkle(ts)?;
        face_tar = encrypt(face_tar, &self.backend_face_public_key);
        let bundle_json = self.make_bundle_json()?;
        let iris_merkle_root = iris_merkle.root().expect("to be populated");
        let normalized_iris_merkle_root = normalized_iris_merkle.root().expect("to be populated");
        let face_merkle_root = face_merkle.root().expect("to be populated");
        tar_append(&mut archive, ts, "iris.sign", sign(iris_merkle_root)?)?;
        tar_append(&mut archive, ts, "iris.tar", iris_tar)?;
        tar_append(&mut archive, ts, "normalized_iris.sign", sign(normalized_iris_merkle_root)?)?;
        tar_append(&mut archive, ts, "normalized_iris.tar", normalized_iris_tar)?;
        tar_append(&mut archive, ts, "face.sign", sign(face_merkle_root)?)?;
        tar_append(&mut archive, ts, "face.tar", face_tar)?;
        tar_append(&mut archive, ts, "bundle.sign", sign(digest(&SHA256, &bundle_json))?)?;
        tar_append(&mut archive, ts, "bundle.json", bundle_json)?;
        let compressed = compress(archive.into_inner()?, ts, "package.tar.gz")?;
        let encrypted = encrypt(compressed, &self.user_public_key);
        Ok(encrypted)
    }

    fn make_iris_tar_and_merkle(&self, ts: Duration) -> Result<(Vec<u8>, MerkleTree<Sha256>)> {
        let mut archive = tar::Builder::new(Vec::new());
        let mut left_ir_png = Vec::new();
        let mut right_ir_png = Vec::new();
        self.capture.eye_left.ir_frame.write_png(&mut left_ir_png, FrameResolution::MAX)?;
        self.capture.eye_right.ir_frame.write_png(&mut right_ir_png, FrameResolution::MAX)?;
        tar_append(&mut archive, ts, "left_ir.png", &left_ir_png)?;
        tar_append(&mut archive, ts, "right_ir.png", &right_ir_png)?;
        let archive = archive.into_inner()?;

        let images = vec![left_ir_png, right_ir_png];
        let mut leaves = images.iter().map(Vec::as_slice).map(Sha256::hash).collect::<Vec<_>>();
        leaves.sort_unstable();
        let merkle = MerkleTree::from_leaves(&leaves);

        Ok((archive, merkle))
    }

    fn make_normalized_iris_tar_and_merkle(
        &self,
        ts: Duration,
    ) -> Result<(Vec<u8>, MerkleTree<Sha256>)> {
        let mut archive = tar::Builder::new(Vec::new());
        let left_normalized_image = self.left_normalized_iris_image.serialized_image();
        let left_normalized_mask = self.left_normalized_iris_image.serialized_mask();
        let right_normalized_image = self.right_normalized_iris_image.serialized_image();
        let right_normalized_mask = self.right_normalized_iris_image.serialized_mask();
        tar_append(&mut archive, ts, "left_normalized_image.bin", &left_normalized_image)?;
        tar_append(&mut archive, ts, "left_normalized_mask.bin", &left_normalized_mask)?;
        tar_append(&mut archive, ts, "right_normalized_image.bin", &right_normalized_image)?;
        tar_append(&mut archive, ts, "right_normalized_mask.bin", &right_normalized_mask)?;
        let archive = archive.into_inner()?;

        let items = vec![
            left_normalized_image,
            left_normalized_mask,
            right_normalized_image,
            right_normalized_mask,
        ];
        let mut leaves = items.iter().map(Vec::as_slice).map(Sha256::hash).collect::<Vec<_>>();
        leaves.sort_unstable();
        let merkle = MerkleTree::from_leaves(&leaves);

        Ok((archive, merkle))
    }

    fn make_face_tar_and_merkle(&self, ts: Duration) -> Result<(Vec<u8>, MerkleTree<Sha256>)> {
        let mut archive = tar::Builder::new(Vec::new());
        let thumbnail_png = self.make_face_thumbnail_png()?;
        tar_append(&mut archive, ts, "thumbnail.png", &thumbnail_png)?;
        let archive = archive.into_inner()?;

        let images = vec![thumbnail_png];
        let mut leaves = images.iter().map(Vec::as_slice).map(Sha256::hash).collect::<Vec<_>>();
        leaves.sort_unstable();
        let merkle = MerkleTree::from_leaves(&leaves);

        Ok((archive, merkle))
    }

    fn make_face_thumbnail_png(&self) -> Result<Vec<u8>> {
        let thumbnail = self.bundle.thumbnail.as_ref().expect("to be provided by the pipeline");
        let image = thumbnail.image.as_ref().expect("to be provided by the pipeline").as_ndarray();
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

    fn make_bundle_json(&self) -> Result<Vec<u8>> {
        let face_identifier::Bundle { embeddings, inference_backend, thumbnail: _, error: _ } =
            &self.bundle;
        let embeddings = embeddings
            .as_ref()
            .expect("to be provided by the pipeline")
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
                Embedding {
                    embedding: BASE64.encode(&embedding),
                    embedding_type,
                    embedding_version,
                    embedding_inference_backend,
                }
            })
            .collect();
        let backend_keys = BackendKeys {
            iris: BackendKey {
                public_key: BASE64.encode(self.backend_iris_public_key.as_ref()),
                encrypted_private_key: &self.backend_iris_encrypted_private_key,
            },
            normalized_iris: BackendKey {
                public_key: BASE64.encode(self.backend_normalized_iris_public_key.as_ref()),
                encrypted_private_key: &self.backend_normalized_iris_encrypted_private_key,
            },
            face: BackendKey {
                public_key: BASE64.encode(self.backend_face_public_key.as_ref()),
                encrypted_private_key: &self.backend_face_encrypted_private_key,
            },
        };
        let bundle = Bundle {
            version: VERSION,
            embeddings,
            inference_backend: inference_backend.as_ref().expect("to be provided by the pipeline"),
            signup_id: &self.signup_id,
            orb_id: ORB_ID.as_str(),
            operator_id: self.operator_id,
            timestamp: &OffsetDateTime::from(self.capture_start).format(&Rfc3339)?,
            backend_keys,
            signup_reason: self.signup_reason,
        };
        let json = serde_json::to_string_pretty(&bundle).wrap_err("serializing bundle as json")?;
        Ok(json.into_bytes())
    }
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

fn encrypt<T: AsRef<[u8]>>(data: T, public_key: &PublicKey) -> Vec<u8> {
    let encrypted = sealedbox::seal(data.as_ref(), public_key);
    assert_ne!(data.as_ref(), encrypted);
    encrypted
}

fn compress<T: AsRef<[u8]>>(data: T, ts: Duration, filename: &str) -> Result<Vec<u8>> {
    let mut encoder = GzBuilder::new()
        .filename(filename)
        .mtime(ts.as_secs().try_into().unwrap())
        .write(Vec::new(), flate2::Compression::best());
    encoder.write_all(data.as_ref())?;
    Ok(encoder.finish()?)
}
