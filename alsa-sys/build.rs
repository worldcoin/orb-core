use color_eyre::eyre::Result;
use std::{env, path::PathBuf};

fn main() -> Result<()> {
    color_eyre::install()?;
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rustc-link-lib=asound");

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_args(env::var("EXTRA_CLANG_CFLAGS")?.split_ascii_whitespace())
        .clang_args(env::var("NIX_CFLAGS_COMPILE")?.split_ascii_whitespace())
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .derive_debug(true)
        .impl_debug(true)
        .generate()?;

    let out_path = PathBuf::from(env::var("OUT_DIR")?);
    bindings.write_to_file(out_path.join("bindings.rs"))?;
    Ok(())
}
