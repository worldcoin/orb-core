//! User ID validation endpoint.

use crate::{
    backend::endpoints::SIGNUP_BACKEND_URL,
    identification::{get_orb_token, ORB_ID},
    plans::{qr_scan, OperatorData},
};
use data_encoding::BASE64;
use eyre::{eyre, Result, WrapErr};
use serde::Deserialize;
use sodiumoxide::crypto::box_::PublicKey;
use std::str;

#[cfg(feature = "internal-data-acquisition")]
use orb_qr_link::DataPolicy;

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
    /// Backend-encryption public key for PCP Tier 2.
    pub backend_tier2_public_key: Option<PublicKey>,
    /// Backend-encryption private key for PCP Tier 2.
    pub backend_tier2_encrypted_private_key: Option<String>,
    /// User's key stored in the app.
    pub self_custody_user_public_key: Option<PublicKey>,
    /// Identity commitment.
    pub id_commitment: Option<String>,
    /// User's biometric data policy.
    #[cfg(feature = "internal-data-acquisition")]
    pub data_policy: DataPolicy,
    /// Personal Custody Package version.
    pub pcp_version: u16,
    /// Whether the orb should perform app-centric signups.
    pub user_centric_signup: bool,
    /// The Orb Relay id which we will use to send information. New apps should always report this.
    pub orb_relay_app_id: Option<String>,
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
    tier2: Option<BackendKey>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct BackendKey {
    public_key: String,
    encrypted_private_key: String,
}

#[cfg(feature = "skip-user-qr-validation")]
#[allow(clippy::unused_async)]
async fn do_request(
    _qr_code: &qr_scan::user::Data,
    _operator_data: &OperatorData,
    _use_full_operator_qr: bool,
    _use_only_operator_location: bool,
) -> Result<Response> {
    let (public_key, _) = sodiumoxide::crypto::box_::gen_keypair();
    let backend_keys = Some(BackendKeys {
        iris: BackendKey {
            public_key: BASE64.encode(public_key.as_ref()),
            encrypted_private_key: "test".to_string(),
        },
        normalized_iris: BackendKey {
            public_key: BASE64.encode(public_key.as_ref()),
            encrypted_private_key: "test".to_string(),
        },
        face: BackendKey {
            public_key: BASE64.encode(public_key.as_ref()),
            encrypted_private_key: "test".to_string(),
        },
        tier2: None,
    });
    let authenticated_app_data = Some(orb_qr_link::UserData {
        identity_commitment: "test".to_string(),
        self_custody_public_key: BASE64.encode(public_key.as_ref()),
        data_policy: orb_qr_link::DataPolicy::OptOut,
        pcp_version: 2,
        user_centric_signup: true,
        orb_relay_app_id: Some(format!("test-skip-user-qr-validation-{}", ORB_ID.to_string())),
    });

    Ok(Response { valid: true, reason: None, backend_keys, authenticated_app_data })
}

#[cfg(not(feature = "skip-user-qr-validation"))]
async fn do_request(
    qr_code: &qr_scan::user::Data,
    operator_data: &OperatorData,
    use_full_operator_qr: bool,
    use_only_operator_location: bool,
) -> Result<Response> {
    let request = if use_only_operator_location {
        super::client()?
            .get(format!("{}/api/v2/session/{}/status", *SIGNUP_BACKEND_URL, qr_code.user_id,))
            .query(&[
                ("lat", operator_data.location_data.session_coordinates.latitude),
                ("lon", operator_data.location_data.session_coordinates.longitude),
            ])
    } else if use_full_operator_qr {
        super::client()?
            .get(format!("{}/api/v2/session/{}/status", *SIGNUP_BACKEND_URL, qr_code.user_id))
            .query(&[("operator_id", &operator_data.qr_code.user_id)])
    } else {
        super::client()?
            .get(format!("{}/api/v1/user/{}/status", *SIGNUP_BACKEND_URL, qr_code.user_id))
    }
    .basic_auth(&*ORB_ID, Some(get_orb_token()?));

    Ok(match request.send().await?.error_for_status() {
        Ok(response) => response.json().await?,
        Err(err) => {
            tracing::error!("Received error response {err:?}");
            return Err(err.into());
        }
    })
}

/// Makes a validation request.
#[allow(clippy::too_many_lines)]
pub async fn request(
    qr_code: &qr_scan::user::Data,
    operator_data: &OperatorData,
    use_full_operator_qr: bool,
    use_only_operator_location: bool,
) -> Result<Option<UserData>> {
    let Response { valid, reason, backend_keys, authenticated_app_data } =
        do_request(qr_code, operator_data, use_full_operator_qr, use_only_operator_location)
            .await?;
    if !valid {
        tracing::info!(
            "User QR-code invalid: {qr_code:?}, reason: {:?}",
            reason.as_deref().unwrap_or("<empty>")
        );
        return Ok(None);
    }
    if let (Some(backend_keys), Some(user_data)) = (backend_keys, authenticated_app_data) {
        tracing::info!("User QR-data: {user_data:?}");

        #[cfg(not(feature = "skip-user-qr-validation"))]
        {
            let Some(user_data_hash) = &qr_code.user_data_hash else {
                tracing::error!(
                    "image_self_custody is provided by backend, but got no user_data_hash from \
                     QR-code"
                );
                return Ok(None);
            };
            if !user_data.verify(user_data_hash) {
                tracing::error!("user_data verification failure");
                return Ok(None);
            }
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
            tier2: backend_tier2,
        } = backend_keys;
        let backend_tier2_public_key =
            backend_tier2.as_ref().map(|backend_tier2| backend_tier2.public_key.as_str());
        let backend_tier2_encrypted_private_key =
            backend_tier2.as_ref().map(|backend_tier2| backend_tier2.encrypted_private_key.clone());
        let orb_qr_link::UserData {
            identity_commitment,
            self_custody_public_key: user_public_key,
            #[cfg(feature = "internal-data-acquisition")]
            data_policy,
            pcp_version,
            user_centric_signup,
            orb_relay_app_id,
            ..
        } = user_data;
        let backend_iris_public_key = decode_public_key(&backend_iris_public_key)
            .wrap_err("decoding backend_iris_public_key")?;
        let backend_normalized_iris_public_key =
            decode_public_key(&backend_normalized_iris_public_key)
                .wrap_err("decoding backend_normalized_iris_public_key")?;
        let backend_face_public_key = decode_public_key(&backend_face_public_key)
            .wrap_err("decoding backend_face_public_key")?;
        let backend_tier2_public_key = backend_tier2_public_key
            .map(decode_public_key)
            .transpose()
            .wrap_err("decoding backend_tier2_public_key")?;
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
            backend_tier2_public_key,
            backend_tier2_encrypted_private_key,
            self_custody_user_public_key: Some(user_public_key),
            id_commitment: Some(identity_commitment),
            #[cfg(feature = "internal-data-acquisition")]
            data_policy,
            pcp_version,
            user_centric_signup,
            orb_relay_app_id,
        }))
    } else {
        // Using an old QR-code format.
        if qr_code.user_data_hash.is_some() {
            tracing::error!(
                "user_data_hash is provided by QR-code, but got no user_data from backend"
            );
            return Ok(None);
        }
        Ok(Some(UserData {
            backend_iris_public_key: None,
            backend_iris_encrypted_private_key: None,
            backend_normalized_iris_public_key: None,
            backend_normalized_iris_encrypted_private_key: None,
            backend_face_public_key: None,
            backend_face_encrypted_private_key: None,
            backend_tier2_public_key: None,
            backend_tier2_encrypted_private_key: None,
            self_custody_user_public_key: None,
            id_commitment: None,
            #[cfg(feature = "internal-data-acquisition")]
            data_policy: DataPolicy::FullDataOptIn,
            pcp_version: 0,
            user_centric_signup: false,
            orb_relay_app_id: None,
        }))
    }
}

fn decode_public_key(payload: &str) -> Result<PublicKey> {
    let bytes = BASE64.decode(payload.as_bytes())?;
    let bytes = <[u8; 32]>::try_from(bytes)
        .map_err(|x| eyre!("incompatible public key length: {}", x.len()))?;
    Ok(PublicKey(bytes))
}
