//! User ID validation endpoint.

use crate::{
    backend::endpoints::SIGNUP_BACKEND_URL,
    identification::{get_orb_token, ORB_ID},
    plans::qr_scan,
};
use data_encoding::BASE64;
use eyre::{eyre, Result, WrapErr};
use orb_qr_link::DataPolicy;
use serde::Deserialize;
use sodiumoxide::crypto::box_::PublicKey;
use std::str;

/// User ID status returned from the backend.
#[derive(Clone, Default)]
pub struct UserData {
    /// Backend-encryption public key for iris images.
    pub backend_iris_public_key: Option<PublicKey>,
    /// Backend-encryption private key for iris images.
    pub backend_iris_encrypted_private_key: Option<String>,
    /// Backend-encryption public key for normalized iris images.
    pub backend_normalized_iris_public_key: Option<PublicKey>,
    /// Backend-encryption private key for normalized iris images.
    pub backend_normalized_iris_encrypted_private_key: Option<String>,
    /// Backend-encryption public key for face images.
    pub backend_face_public_key: Option<PublicKey>,
    /// Backend-encryption private key for face images.
    pub backend_face_encrypted_private_key: Option<String>,
    /// User's key stored in the app.
    pub self_custody_user_public_key: Option<PublicKey>,
    /// Identity commitment.
    pub id_commitment: Option<String>,
    /// User's biometric data policy.
    pub data_policy: DataPolicy,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Response {
    valid: bool,
    reason: Option<String>,
    backend_keys: Option<BackendKeys>,
    authenticated_app_data: Option<orb_qr_link::UserData>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct BackendKeys {
    iris: BackendKey,
    normalized_iris: BackendKey,
    face: BackendKey,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct BackendKey {
    public_key: String,
    encrypted_private_key: String,
}

/// Makes a validation request.
pub async fn request(qr_code: &qr_scan::user::Data) -> Result<Option<UserData>> {
    let request = super::client()?
        .get(format!("{}/api/v1/user/{}/status", *SIGNUP_BACKEND_URL, qr_code.user_id))
        .basic_auth(&*ORB_ID, Some(get_orb_token()?));
    let Response { valid, reason, backend_keys, authenticated_app_data } =
        match request.send().await?.error_for_status() {
            Ok(response) => response.json().await?,
            Err(err) => {
                tracing::error!("Received error response {err:?}");
                return Err(err.into());
            }
        };
    if !valid {
        tracing::info!(
            "User QR-code invalid: {qr_code:?}, reason: {:?}",
            reason.as_deref().unwrap_or("<empty>")
        );
        return Ok(None);
    }
    if let (Some(backend_keys), Some(user_data)) = (backend_keys, authenticated_app_data) {
        // Using the new QR-code format.
        let Some(user_data_hash) = &qr_code.user_data_hash else {
            tracing::error!(
                "image_self_custody is provided by backend, but got no user_data_hash from QR-code"
            );
            return Ok(None);
        };
        if !user_data.verify(user_data_hash) {
            tracing::error!("user_data verification failure");
            return Ok(None);
        }
        let BackendKeys {
            iris:
                BackendKey {
                    public_key: backend_iris_public_key,
                    encrypted_private_key: backend_iris_encrypted_private_key,
                },
            normalized_iris:
                BackendKey {
                    public_key: backend_normalized_iris_public_key,
                    encrypted_private_key: backend_normalized_iris_encrypted_private_key,
                },
            face:
                BackendKey {
                    public_key: backend_face_public_key,
                    encrypted_private_key: backend_face_encrypted_private_key,
                },
        } = backend_keys;
        let orb_qr_link::UserData {
            identity_commitment,
            self_custody_public_key: user_public_key,
            data_policy,
        } = user_data;
        let backend_iris_public_key = decode_public_key(&backend_iris_public_key)
            .wrap_err("decoding backend_iris_public_key")?;
        let backend_normalized_iris_public_key =
            decode_public_key(&backend_normalized_iris_public_key)
                .wrap_err("decoding backend_normalized_iris_public_key")?;
        let backend_face_public_key = decode_public_key(&backend_face_public_key)
            .wrap_err("decoding backend_face_public_key")?;
        let user_public_key =
            decode_public_key(&user_public_key).wrap_err("decoding user_public_key")?;
        Ok(Some(UserData {
            backend_iris_public_key: Some(backend_iris_public_key),
            backend_iris_encrypted_private_key: Some(backend_iris_encrypted_private_key),
            backend_normalized_iris_public_key: Some(backend_normalized_iris_public_key),
            backend_normalized_iris_encrypted_private_key: Some(
                backend_normalized_iris_encrypted_private_key,
            ),
            backend_face_public_key: Some(backend_face_public_key),
            backend_face_encrypted_private_key: Some(backend_face_encrypted_private_key),
            self_custody_user_public_key: Some(user_public_key),
            id_commitment: Some(identity_commitment),
            data_policy,
        }))
    } else {
        // Using an old QR-code format.
        if qr_code.user_data_hash.is_some() {
            tracing::error!(
                "user_data_hash is provided by QR-code, but got no user_data from backend"
            );
            return Ok(None);
        }
        let data_policy =
            qr_code.data_policy.map(Into::into).ok_or_else(|| eyre!("missing data-policy"))?;
        Ok(Some(UserData {
            backend_iris_public_key: None,
            backend_iris_encrypted_private_key: None,
            backend_normalized_iris_public_key: None,
            backend_normalized_iris_encrypted_private_key: None,
            backend_face_public_key: None,
            backend_face_encrypted_private_key: None,
            self_custody_user_public_key: None,
            id_commitment: None,
            data_policy,
        }))
    }
}

fn decode_public_key(payload: &str) -> Result<PublicKey> {
    let bytes = BASE64.decode(payload.as_bytes())?;
    let bytes = <[u8; 32]>::try_from(bytes)
        .map_err(|x| eyre!("incompatible public key length: {}", x.len()))?;
    Ok(PublicKey(bytes))
}
