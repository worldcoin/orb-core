#![warn(clippy::pedantic)]

use color_eyre::eyre::{Result, WrapErr};
use std::{fs, path::Path, process::Command, str};
use toml::Value;

fn main() -> Result<()> {
    // Save git commit hash at compile-time as environment variable
    println!("cargo:rerun-if-changed=.git/HEAD");
    let git_version = Path::new("git_version");
    let git_version = if git_version.exists() {
        fs::read_to_string(git_version).wrap_err("failed to read git_version")?
    } else {
        str::from_utf8(
            &Command::new("git")
                .arg("describe")
                .arg("--tags")
                .arg("--always")
                .arg("--dirty=-modified")
                .output()?
                .stdout,
        )?
        .trim_end()
        .to_string()
    };
    println!("cargo:rustc-env=GIT_VERSION={git_version:0>4}");

    let cargo_lock = fs::read_to_string("Cargo.lock").expect("Cargo.lock to exist");
    let parsed_lock: Value = toml::from_str(&cargo_lock).expect("Cargo.lock to be valid TOML");
    let iris_mpc_package = parsed_lock["package"]
        .as_array()
        .expect("to find packages in Cargo.lock")
        .iter()
        .find(|p| p["name"].as_str() == Some("iris-mpc-common"))
        .expect("to find iris-mpc-common in Cargo.lock");
    let iris_mpc_rev = iris_mpc_package["source"]
        .as_str()
        .expect("to find source for iris-mpc-common")
        .split('#')
        .last()
        .expect("to extract revision from source");

    println!("cargo:rustc-env=IRIS_MPC_VERSION={iris_mpc_rev}");
    println!("cargo:rerun-if-changed=Cargo.toml");

    Ok(())
}
