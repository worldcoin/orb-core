//! Secure Element interface.

#![cfg_attr(test, allow(unused_imports))]

use crate::process::Command;
use data_encoding::BASE64;
use eyre::{bail, Result, WrapErr};
use std::{io::prelude::*, process::Stdio};

/// Signs this buffer with Secure Element and returns the output.
#[cfg(not(test))]
pub fn sign<T: AsRef<[u8]>>(data: T) -> Result<Vec<u8>> {
    fn inner(data: &[u8]) -> Result<Vec<u8>> {
        let encoded = BASE64.encode(data);

        tracing::info!("Running orb-sign-iris-code");
        let mut command = Command::new("/usr/bin/orb-sign-iris-code");
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        let mut child = command.spawn().wrap_err("running orb-sign-iris-code")?;

        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(encoded.as_bytes())?;
        drop(stdin);

        let output = child.wait_with_output().wrap_err("waiting for orb-sign-iris-code")?;
        let success = output.status.success();
        for line in String::from_utf8_lossy(&output.stderr).lines() {
            if success {
                tracing::trace!("orb-sign-iris-code {}", line);
            } else {
                tracing::error!("orb-sign-iris-code {}", line);
            }
        }
        if !success {
            if let Some(code) = output.status.code() {
                bail!("orb-sign-iris-code exited with non-zero exit code: {code}");
            } else {
                bail!("orb-sign-iris-code terminated by signal");
            }
        }
        BASE64.decode(&output.stdout).wrap_err("decoding orb-sign-iris-code output")
    }

    inner(data.as_ref())
}

#[cfg(test)]
static SIGNING_KEY: once_cell::sync::Lazy<
    std::sync::Arc<std::sync::Mutex<openssl::ec::EcKey<openssl::pkey::Private>>>,
> = once_cell::sync::Lazy::new(|| {
    let group = openssl::ec::EcGroup::from_curve_name(openssl::nid::Nid::SECP256K1).unwrap();
    let ec_key = openssl::ec::EcKey::generate(&group).unwrap();
    std::sync::Arc::new(std::sync::Mutex::new(ec_key))
});

#[cfg(test)]
pub fn sign<T: AsRef<[u8]>>(data: T) -> Result<Vec<u8>> {
    let pkey = SIGNING_KEY.lock().unwrap();
    Ok(openssl::ecdsa::EcdsaSig::sign(data.as_ref(), &*pkey).unwrap().to_der().unwrap())
}

#[cfg(test)]
pub fn get_public_pem() -> Result<String> {
    let pkey = SIGNING_KEY.lock().unwrap();
    Ok(String::from_utf8(pkey.public_key_to_pem()?)?)
}

#[cfg(test)]
pub fn get_private_pem() -> Result<String> {
    let pkey = SIGNING_KEY.lock().unwrap();
    Ok(String::from_utf8(pkey.private_key_to_pem()?)?)
}
