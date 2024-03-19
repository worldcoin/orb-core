use bindgen::callbacks::ParseCallbacks;
use color_eyre::eyre::Result;
use std::{env, path::PathBuf};

#[derive(Debug)]
pub struct ConstifyMacro {}

impl ParseCallbacks for ConstifyMacro {
    fn item_name(&self, original_item_name: &str) -> Option<String> {
        Some(original_item_name.trim_start_matches("__CONSTIFY_MACRO_").to_owned())
    }
}

fn main() -> Result<()> {
    color_eyre::install()?;
    println!("cargo:rerun-if-changed=wrapper.h");

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_args(env::var("EXTRA_CLANG_CFLAGS")?.split_ascii_whitespace())
        .clang_args(env::var("NIX_CFLAGS_COMPILE")?.split_ascii_whitespace())
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        .parse_callbacks(Box::new(ConstifyMacro {}))
        .derive_debug(true)
        // Disabled impl_debug because it causes hard compilation errors in rust 1.69.
        // see https://github.com/rust-lang/rust-bindgen/issues/2221
        // .impl_debug(true)
        .generate()?;

    let out_path = PathBuf::from(env::var("OUT_DIR")?);
    bindings.write_to_file(out_path.join("bindings.rs"))?;
    Ok(())
}
