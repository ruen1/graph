use image::{ImageEncoder, ImageFormat};
use sha2::{Digest, Sha256};
use std::io::Cursor;

#[derive(Debug)]
pub struct MediaFile {
    pub file_name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum MediaError {
    UnsupportedType,
    InvalidData,
    MimeMismatch,
}

struct ProcessedMedia {
    bytes: Vec<u8>,
    extension: &'static str,
}

pub fn inspect_media_with_options(
    data: &[u8],
    declared_type: &str,
    strip_exif: bool,
) -> Result<MediaFile, MediaError> {
    let declared_type = declared_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let hash = Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let processed = match declared_type.as_str() {
        "image/jpeg" => validate_image(data, ImageFormat::Jpeg, "jpg", strip_exif)?,
        "image/png" => validate_image(data, ImageFormat::Png, "png", strip_exif)?,
        "image/gif" => validate_image(data, ImageFormat::Gif, "gif", false)?,
        "image/webp" => validate_image(data, ImageFormat::WebP, "webp", strip_exif)?,
        "video/mp4" => {
            validate_mp4(data)?;
            ProcessedMedia {
                bytes: data.to_vec(),
                extension: "mp4",
            }
        }
        _ => return Err(MediaError::UnsupportedType),
    };
    Ok(MediaFile {
        file_name: format!("{hash}.{}", processed.extension),
        bytes: processed.bytes,
    })
}

fn validate_image(
    data: &[u8],
    expected: ImageFormat,
    extension: &'static str,
    strip_exif: bool,
) -> Result<ProcessedMedia, MediaError> {
    let actual = image::guess_format(data).map_err(|_| MediaError::InvalidData)?;
    if actual != expected {
        return Err(MediaError::MimeMismatch);
    }
    let image =
        image::load_from_memory_with_format(data, expected).map_err(|_| MediaError::InvalidData)?;
    let bytes = if strip_exif {
        reencode_image(image, expected)?
    } else {
        data.to_vec()
    };
    Ok(ProcessedMedia { bytes, extension })
}

fn reencode_image(image: image::DynamicImage, format: ImageFormat) -> Result<Vec<u8>, MediaError> {
    let mut bytes = Vec::new();
    if format == ImageFormat::Jpeg {
        let image = image.to_rgb8();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90)
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|_| MediaError::InvalidData)?;
    } else {
        image
            .write_to(&mut Cursor::new(&mut bytes), format)
            .map_err(|_| MediaError::InvalidData)?;
    }
    Ok(bytes)
}

fn validate_mp4(data: &[u8]) -> Result<(), MediaError> {
    if data.len() < 12 || &data[4..8] != b"ftyp" {
        return Err(MediaError::InvalidData);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inspect_media(data: &[u8], declared_type: &str) -> Result<MediaFile, MediaError> {
        inspect_media_with_options(data, declared_type, true)
    }

    fn encoded_image(format: ImageFormat) -> Vec<u8> {
        let image = image::ImageBuffer::<image::Rgb<u8>, Vec<u8>>::new(1, 1);
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), format)
            .unwrap();
        bytes
    }

    fn with_exif(bytes: &[u8], marker: &[u8]) -> Vec<u8> {
        let mut segment = vec![0xFF, 0xE1];
        segment.extend_from_slice(&(marker.len() as u16 + 2).to_be_bytes());
        segment.extend_from_slice(marker);
        let mut output = bytes[..2].to_vec();
        output.extend_from_slice(&segment);
        output.extend_from_slice(&bytes[2..]);
        output
    }

    #[test]
    fn identical_images_have_identical_names() {
        let bytes = encoded_image(ImageFormat::Png);
        let first = inspect_media(&bytes, "image/png").unwrap();
        let second = inspect_media(&bytes, "image/png").unwrap();
        assert_eq!(first.file_name, second.file_name);
        assert_eq!(first.file_name.len(), 68);
        assert!(first.file_name.ends_with(".png"));
    }

    #[test]
    fn different_images_have_different_names() {
        let first = inspect_media(&encoded_image(ImageFormat::Png), "image/png").unwrap();
        let second = inspect_media(&encoded_image(ImageFormat::Jpeg), "image/jpeg").unwrap();
        assert_ne!(first.file_name, second.file_name);
    }

    #[test]
    fn rejects_pdf_and_truncated_images() {
        assert_eq!(
            inspect_media(b"%PDF-1.4\n", "image/png").unwrap_err(),
            MediaError::InvalidData
        );
        let bytes = encoded_image(ImageFormat::Png);
        assert_eq!(
            inspect_media(&bytes[..16], "image/png").unwrap_err(),
            MediaError::InvalidData
        );
    }

    #[test]
    fn rejects_mismatched_mime_types() {
        let jpeg = encoded_image(ImageFormat::Jpeg);
        assert_eq!(
            inspect_media(&jpeg, "image/png").unwrap_err(),
            MediaError::MimeMismatch
        );
    }

    #[test]
    fn rejects_non_mp4_video() {
        assert_eq!(
            inspect_media(b"not an mp4", "video/mp4").unwrap_err(),
            MediaError::InvalidData
        );
    }

    #[test]
    fn exif_is_removed_without_changing_original_hash() {
        let jpeg = encoded_image(ImageFormat::Jpeg);
        let first = with_exif(&jpeg, b"Exif\0\0GPS_LOCATION_A");
        let second = with_exif(&jpeg, b"Exif\0\0CAMERA_MODEL_B");
        let first_media = inspect_media_with_options(&first, "image/jpeg", true).unwrap();
        let second_media = inspect_media_with_options(&second, "image/jpeg", true).unwrap();
        assert_ne!(first_media.file_name, second_media.file_name);
        let stripped = inspect_media_with_options(&first, "image/jpeg", true).unwrap();
        let preserved = inspect_media_with_options(&first, "image/jpeg", false).unwrap();
        assert_eq!(stripped.file_name, preserved.file_name);
        assert!(
            !first_media
                .bytes
                .windows(b"GPS_LOCATION_A".len())
                .any(|window| window == b"GPS_LOCATION_A")
        );
    }

    #[test]
    fn strip_exif_can_be_disabled() {
        let jpeg = encoded_image(ImageFormat::Jpeg);
        let bytes = with_exif(&jpeg, b"Exif\0\0GPS_LOCATION_A");
        let media = inspect_media_with_options(&bytes, "image/jpeg", false).unwrap();
        let expected = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(media.file_name, format!("{expected}.jpg"));
    }

    #[test]
    fn gif_bytes_are_not_reencoded() {
        let bytes = encoded_image(ImageFormat::Gif);
        let media = inspect_media_with_options(&bytes, "image/gif", true).unwrap();
        let expected = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(media.file_name, format!("{expected}.gif"));
    }
}
