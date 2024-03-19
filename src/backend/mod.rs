//! Communication with the Orb backend.

pub mod config;
pub mod endpoints;
pub mod operator_status;
pub mod presigned_url;
pub mod s3_region;
pub mod signup_poll;
pub mod signup_post;
pub mod status;
pub mod upload_debug_report;
pub mod upload_image;
pub mod upload_self_custody_images;
pub mod user_status;

use crate::sound::{self, Melody};
use eyre::{eyre, Error, Result};
use hex_literal::hex;
use once_cell::sync::OnceCell;
use orb_sound::SoundFuture;
use ring::digest::{Context, SHA256};
use std::{fs, time::Duration};

const APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

static AWS_CA_CERT: OnceCell<reqwest::Certificate> = OnceCell::new();
static GTS_CA_CERT: OnceCell<reqwest::Certificate> = OnceCell::new();

/// Initializes the pinned root certificates.
///
/// # Panics
///
/// * If the function has been already called before
/// * If the hash of the stored certificate is different than the hardcoded in
/// binary
pub fn init_cert() -> Result<()> {
    let aws_cert = fs::read("/etc/ssl/AmazonRootCA1.pem")?;

    // Verify that the certificate has not been replaced
    let mut context = Context::new(&SHA256);
    context.update(&aws_cert);
    let mut digest = context.finish();
    assert_eq!(
        digest.as_ref(),
        hex!("2c43952ee9e000ff2acc4e2ed0897c0a72ad5fa72c3d934e81741cbd54f05bd1").as_slice(),
    );

    AWS_CA_CERT
        .set(reqwest::Certificate::from_pem(&aws_cert)?)
        .map_err(|_| eyre!("init_cert called twice"))?;

    // Initialize Google Trust Services Root CA
    let gts_cert = fs::read("/etc/ssl/gtsr1.pem")?;
    context = Context::new(&SHA256);
    context.update(&gts_cert);
    digest = context.finish();
    assert_eq!(
        digest.as_ref(),
        hex!("4195ea007a7ef8d3e2d338e8d9ff0083198e36bfa025442ddf41bb5213904fc2").as_slice(),
    );

    GTS_CA_CERT
        .set(reqwest::Certificate::from_pem(&gts_cert)?)
        .map_err(|_| eyre!("init_cert called twice"))?;
    Ok(())
}

/// Creates a new HTTP client.
///
/// # Panics
///
/// If [`init_cert`] hasn't been called yet.
pub fn client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(APP_USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .tls_built_in_root_certs(false)
        .add_root_certificate(
            AWS_CA_CERT.get().expect("the AWS root certificate is not initialized").clone(),
        )
        .add_root_certificate(
            GTS_CA_CERT.get().expect("the GTS root certificate is not initialized").clone(),
        )
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

/// Plays network error sound.
pub fn error_sound(sound: &mut dyn sound::Player, err: &Error) -> Result<SoundFuture> {
    let log_msg = if matches!(err.downcast_ref::<reqwest::Error>(), Some(err) if err.is_decode()) {
        "Decoding network response failed"
    } else {
        "Network request failed"
    };
    tracing::error!("{log_msg}: {err:?}", log_msg = log_msg, err = err);
    let fut = sound.build(sound::Type::Melody(Melody::SoundError))?.push()?;
    Ok(fut)
}
