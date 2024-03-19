#![warn(clippy::pedantic)]

use color_eyre::eyre::{Result, WrapErr};
use std::{fs, path::Path, process::Command, str};

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

    Ok(())
}
