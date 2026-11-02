//! Embed `assets/GPNG.ico` as the Windows executable icon.
//!
//! The file may be a real ICO or a PNG saved with a `.ico` extension.

fn main() {
    let src = std::path::Path::new("assets").join("GPNG.ico");
    println!("cargo:rerun-if-changed={}", src.display());

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }
    if !src.is_file() {
        println!("cargo:warning=assets/GPNG.ico missing — skipping exe icon");
        return;
    }

    let bytes = match std::fs::read(&src) {
        Ok(b) => b,
        Err(e) => {
            println!("cargo:warning=could not read {}: {e}", src.display());
            return;
        }
    };

    let img = match open_icon_bytes(&bytes) {
        Ok(i) => i,
        Err(e) => {
            println!("cargo:warning=could not decode {}: {e}", src.display());
            return;
        }
    };

    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("GPNG.ico");
    if let Err(e) = img.save_with_format(&out, image::ImageFormat::Ico) {
        println!("cargo:warning=could not write build ICO: {e}");
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon(out.to_str().expect("utf-8 OUT_DIR"));
    if let Err(e) = res.compile() {
        println!("cargo:warning=winresource: {e}");
    }
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
