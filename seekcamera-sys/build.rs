use color_eyre::eyre::{eyre, Result, WrapErr};
use std::{env, path::PathBuf};

fn main() -> Result<()> {
    color_eyre::install()?;

    let sdk_path = env::var_os("SEEK_SDK_PATH")
        .map(PathBuf::from)
        .ok_or_else(|| eyre!("$SEEK_SDK_PATH env var is not set"))?
        .canonicalize()
        .wrap_err("failed to canonicalize `SEEK_SDK_PATH`. Does the folder exist?")?
        .join(match env::var("TARGET").unwrap().as_str() {
            "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
            "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
            target => panic!("unsupported target architecture: {target}"),
        });

    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rustc-link-lib=seekcamera");
    println!("cargo:rustc-link-search={}", sdk_path.join("lib").display());

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_args(env::var("EXTRA_CLANG_CFLAGS")?.split_ascii_whitespace())
        .clang_args(env::var("NIX_CFLAGS_COMPILE")?.split_ascii_whitespace())
        .clang_arg(format!("-I{}", sdk_path.join("include").display()))
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .derive_debug(true)
        .impl_debug(true)
        .generate()?;

    let out_path = PathBuf::from(env::var("OUT_DIR")?);
    bindings.write_to_file(out_path.join("bindings.rs"))?;
    Ok(())
}
