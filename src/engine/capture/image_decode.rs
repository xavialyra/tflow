use image::{DynamicImage, ImageDecoder, ImageReader, Limits};
use std::fs::{File, OpenOptions};
use std::io::BufReader;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

pub(crate) const MAX_IMAGE_WIDTH: u32 = 8192;
pub(crate) const MAX_IMAGE_HEIGHT: u32 = 8192;
pub(crate) const MAX_IMAGE_DECODE_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const MAX_IMAGE_ENCODED_BYTES: u64 = 64 * 1024 * 1024;

type DecodeResult = std::result::Result<DynamicImage, String>;

pub(crate) fn decode_image(path: &Path) -> DecodeResult {
    let file = open_image_file(path)?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_WIDTH);
    limits.max_image_height = Some(MAX_IMAGE_HEIGHT);
    limits.max_alloc = Some(MAX_IMAGE_DECODE_BYTES);
    let mut reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    reader.limits(limits);
    let decoder = reader.into_decoder().map_err(|error| {
        format!("image exceeds configured dimensions or allocation limits: {error}")
    })?;
    let (width, height) = decoder.dimensions();
    validate_dimensions(width, height)?;
    validate_decoded_bytes(decoder.total_bytes())?;
    DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())
}

fn open_image_file(path: &Path) -> std::result::Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "image path {} is not a regular file",
            path.display()
        ));
    }
    if metadata.len() > MAX_IMAGE_ENCODED_BYTES {
        return Err(format!(
            "image source {} is {} bytes, exceeding the {}-byte encoded limit",
            path.display(),
            metadata.len(),
            MAX_IMAGE_ENCODED_BYTES
        ));
    }
    Ok(file)
}

fn validate_dimensions(width: u32, height: u32) -> std::result::Result<(), String> {
    if width > MAX_IMAGE_WIDTH || height > MAX_IMAGE_HEIGHT {
        return Err(format!(
            "image dimensions {width}x{height} exceed the {MAX_IMAGE_WIDTH}x{MAX_IMAGE_HEIGHT} limit"
        ));
    }
    Ok(())
}

fn validate_decoded_bytes(decoded_bytes: u64) -> std::result::Result<(), String> {
    if decoded_bytes > MAX_IMAGE_DECODE_BYTES {
        return Err(format!(
            "image requires {decoded_bytes} decoded bytes, exceeding the {MAX_IMAGE_DECODE_BYTES}-byte limit"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;
    use std::fs::{self, File};
    use std::os::unix::ffi::OsStrExt;
    fn temporary_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("tflow-image-{}-{name}", std::process::id()))
    }

    #[test]
    fn decodes_a_valid_image_through_the_real_decoder() {
        let path = temporary_path("valid.png");
        DynamicImage::new_rgba8(2, 3).save(&path).unwrap();
        let decoded = decode_image(&path).unwrap();
        assert_eq!(decoded.dimensions(), (2, 3));
        fs::remove_file(path).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejects_nonregular_and_oversized_encoded_sources_before_decoding() {
        use std::ffi::CString;

        let oversized = temporary_path("oversized-source.bin");
        let file = File::create(&oversized).unwrap();
        file.set_len(MAX_IMAGE_ENCODED_BYTES + 1).unwrap();
        let error = decode_image(&oversized).unwrap_err();
        assert!(error.contains("encoded limit"), "error: {error}");
        fs::remove_file(&oversized).unwrap();

        let fifo = temporary_path("source.fifo");
        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        let error = decode_image(&fifo).unwrap_err();
        assert!(error.contains("not a regular file"), "error: {error}");
        fs::remove_file(fifo).unwrap();
    }

    #[test]
    fn rejects_oversized_and_corrupt_images_with_readable_errors() {
        let oversized = temporary_path("oversized.bmp");
        let corrupt = temporary_path("corrupt.png");
        let write_bmp_header = |path: &Path, width: u32, height: u32| {
            let mut header = vec![0_u8; 54];
            header[..2].copy_from_slice(b"BM");
            header[2..6].copy_from_slice(&54_u32.to_le_bytes());
            header[10..14].copy_from_slice(&54_u32.to_le_bytes());
            header[14..18].copy_from_slice(&40_u32.to_le_bytes());
            header[18..22].copy_from_slice(&width.to_le_bytes());
            header[22..26].copy_from_slice(&height.to_le_bytes());
            header[26..28].copy_from_slice(&1_u16.to_le_bytes());
            header[28..30].copy_from_slice(&24_u16.to_le_bytes());
            fs::write(path, header).unwrap();
        };
        write_bmp_header(&oversized, MAX_IMAGE_WIDTH + 1, 1);
        fs::write(&corrupt, b"not a png").unwrap();

        let oversized_error = decode_image(&oversized).unwrap_err();
        assert!(oversized_error.contains("limit"), "{oversized_error}");
        let allocation_error = validate_decoded_bytes(MAX_IMAGE_DECODE_BYTES + 1).unwrap_err();
        assert!(
            allocation_error.contains("decoded bytes"),
            "{allocation_error}"
        );
        let corrupt_error = decode_image(&corrupt).unwrap_err();
        assert!(!corrupt_error.is_empty());

        fs::remove_file(oversized).unwrap();
        fs::remove_file(corrupt).unwrap();
    }
}
