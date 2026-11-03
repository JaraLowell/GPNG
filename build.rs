//! Embed the Windows icon and VERSIONINFO (Explorer → Properties → Details).
//!
//! String fields live in `[package.metadata.winresource]` in `Cargo.toml`.
//! The icon file may be a real ICO or a PNG saved with a `.ico` extension.

fn main() {
    let src = std::path::Path::new("assets").join("GPNG.ico");
    println!("cargo:rerun-if-changed={}", src.display());
    println!("cargo:rerun-if-changed=Cargo.toml");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    let mut res = winresource::WindowsResource::new();
    // English (US) → string block 040904B0, which Explorer looks up.
    res.set_language(0x0409);
    if let Some(icon) = prepare_icon(&src) {
        res.set_icon(&icon);
    }
    if let Err(e) = res.compile() {
        println!("cargo:warning=winresource: {e}");
    }
}

/// Decode `assets/GPNG.ico` and write a real ICO for the resource compiler.
fn prepare_icon(src: &std::path::Path) -> Option<String> {
    if !src.is_file() {
        println!("cargo:warning=assets/GPNG.ico missing — skipping exe icon");
        return None;
    }
    let bytes = match std::fs::read(src) {
        Ok(b) => b,
        Err(e) => {
            println!("cargo:warning=could not read {}: {e}", src.display());
            return None;
        }
    };
    let img = match open_icon_bytes(&bytes) {
        Ok(i) => i,
        Err(e) => {
            println!("cargo:warning=could not decode {}: {e}", src.display());
            return None;
        }
    };
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("GPNG.ico");
    if let Err(e) = img.save_with_format(&out, image::ImageFormat::Ico) {
        println!("cargo:warning=could not write build ICO: {e}");
        return None;
    }
    Some(out.to_str().expect("utf-8 OUT_DIR").to_string())
}

fn open_icon_bytes(bytes: &[u8]) -> Result<image::DynamicImage, image::ImageError> {
    // Prefer magic-byte detection — asset is often a PNG named `.ico`.
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return image::load_from_memory_with_format(bytes, image::ImageFormat::Png);
    }
    if bytes.len() >= 4 && bytes[0] == 0 && bytes[1] == 0 && bytes[2] == 1 && bytes[3] == 0 {
        return image::load_from_memory_with_format(bytes, image::ImageFormat::Ico);
    }
    image::load_from_memory(bytes)
}
