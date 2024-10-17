use color_eyre::eyre::{eyre, Result, WrapErr};
use std::{env, path::PathBuf};

fn main() -> Result<()> {
    color_eyre::install()?;

    let sdk_path = env::var_os("ROYALE_SDK_PATH")
        .map(PathBuf::from)
        .ok_or_else(|| eyre!("$ROYALE_SDK_PATH env var is not set"))?
        .canonicalize()
        .wrap_err("failed to canonicalize `ROYALE_SDK_PATH`. Does the folder exist?")?;

    println!("cargo:rerun-if-changed=wrapper.hpp");
    println!("cargo:rerun-if-changed=wrapper.cpp");
    if env::var("TARGET").unwrap().as_str() == "aarch64-unknown-linux-gnu" {
        cc::Build::new()
            .file("wrapper.cpp")
            .include(sdk_path.join("include"))
            .flag("-O2")
            .cpp(true)
            .compile("royale_wrapper");
        println!("cargo:rustc-link-lib=royale");
        println!("cargo:rustc-link-lib=spectre");
        println!("cargo:rustc-link-search={}", sdk_path.join("bin").display());
        println!("cargo:rustc-link-lib=static=royale_wrapper");
    }

    let bindings = bindgen::Builder::default()
        .header("wrapper.hpp")
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
