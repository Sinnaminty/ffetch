//! Preprocesses the logo at compile time.
//!
//! The pipeline itself lives in `src/image.rs`, which the binary also uses for
//! `--image`. The binary embeds the resulting RGBA buffer and turns it into
//! ASCII at runtime, so the logo can be rendered at any size.
//!
//! Set `FFETCH_LOGO=/path/to/image.png` at build time to use a different image.

#[path = "src/image.rs"]
mod image;

use std::{env, fs, path::PathBuf};

fn main() {
    let path = env::var("FFETCH_LOGO").unwrap_or_else(|_| "assets/logo.png".into());
    println!("cargo:rerun-if-env-changed=FFETCH_LOGO");
    println!("cargo:rerun-if-changed={path}");
    println!("cargo:rerun-if-changed=src/image.rs");

    let logo = image::process(path.as_ref(), false)
        .unwrap_or_else(|e| panic!("cannot load logo {path}: {e}"));

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out.join("logo.rgba"), &logo.pixels).unwrap();
    let background = match logo.background {
        Some([r, g, b]) => format!("Some(Rgb({r}, {g}, {b}))"),
        None => "None".into(),
    };
    let src = format!(
        "pub const WIDTH: usize = {};\n\
         pub const HEIGHT: usize = {};\n\
         pub const BACKGROUND: Option<Rgb> = {background};\n\
         pub static PIXELS: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/logo.rgba\"));\n",
        logo.width, logo.height,
    );
    fs::write(out.join("logo.rs"), src).unwrap();
}
