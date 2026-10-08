//! On-disk cache under `${XDG_CACHE_HOME:-~/.cache}/ffetch`.
//!
//! Processed `--image` logos are stored as `images/<key>.rgba`. The key hashes
//! the image's canonical path, size and mtime, so editing or replacing the file
//! gives it a new entry. The cache is never essential: a missing or malformed
//! entry is recomputed, and a failed write is ignored.

use std::{
    env, fs, io,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
    process,
};

use crate::image::{self, Image};

/// Starts every image entry. Bump the digit when the layout or the image
/// pipeline changes, so stale entries stop matching.
const MAGIC: &[u8; 8] = b"FFIMAGE1";
/// Magic, width and height (u32 LE), a background flag and the background RGB.
const HEADER_LEN: usize = 8 + 4 + 4 + 1 + 3;

/// `${XDG_CACHE_HOME:-$HOME/.cache}/ffetch`, or `None` without a usable base.
pub fn dir() -> Option<PathBuf> {
    // The XDG spec says to ignore relative paths.
    let xdg = env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute());
    let base = xdg.or_else(|| {
        let home = env::var_os("HOME").filter(|h| !h.is_empty())?;
        Some(PathBuf::from(home).join(".cache"))
    })?;
    Some(base.join("ffetch"))
}

/// Processes the PNG at `path` (see `image::process`), reusing the entry in the
/// cache directory `dir` while the file is unchanged.
pub fn image(path: &Path, keep_background: bool, dir: Option<&Path>) -> Result<Image, String> {
    let path = fs::canonicalize(path).map_err(|e| e.to_string())?;
    let meta = fs::metadata(&path).map_err(|e| e.to_string())?;
    let key = image_key(
        &path,
        meta.len(),
        (meta.mtime(), meta.mtime_nsec()),
        keep_background,
    );
    let entry = dir.map(|d| d.join("images").join(format!("{key:016x}.rgba")));

    if let Some(img) = entry.as_deref().and_then(read_image) {
        return Ok(img);
    }
    let img = image::process(&path, keep_background)?;
    if let Some(entry) = &entry {
        // Failing to cache only costs the processing time again next run.
        let _ = write_image(entry, &img);
    }
    Ok(img)
}

/// Cache key for a processed image, covering everything that affects the result.
fn image_key(path: &Path, size: u64, (secs, nanos): (i64, i64), keep_background: bool) -> u64 {
    fnv1a(&[
        MAGIC,
        env!("CARGO_PKG_VERSION").as_bytes(),
        path.as_os_str().as_bytes(),
        &size.to_le_bytes(),
        &secs.to_le_bytes(),
        &nanos.to_le_bytes(),
        &[keep_background as u8],
    ])
}

/// 64-bit FNV-1a. Unlike std's `DefaultHasher`, it is stable across Rust versions.
fn fnv1a(parts: &[&[u8]]) -> u64 {
    parts
        .iter()
        .flat_map(|p| p.iter())
        .fold(0xcbf2_9ce4_8422_2325, |h, &b| {
            (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
        })
}

/// Reads an image entry; `None` if it is missing or malformed.
fn read_image(entry: &Path) -> Option<Image> {
    let data = fs::read(entry).ok()?;
    let (header, pixels) = data.split_at_checked(HEADER_LEN)?;
    if &header[..8] != MAGIC {
        return None;
    }
    let dim = |i: usize| Some(u32::from_le_bytes(header.get(i..i + 4)?.try_into().ok()?) as usize);
    let (width, height) = (dim(8)?, dim(12)?);
    let background = match header[16] {
        0 => None,
        1 => Some([header[17], header[18], header[19]]),
        _ => return None,
    };
    let len = width.checked_mul(height)?.checked_mul(4)?;
    (width > 0 && height > 0 && len == pixels.len()).then(|| Image {
        width,
        height,
        pixels: pixels.to_vec(),
        background,
    })
}

/// Writes an image entry atomically: to a temporary file, then renamed into place.
fn write_image(entry: &Path, img: &Image) -> io::Result<()> {
    let dir = entry.parent().ok_or(io::ErrorKind::InvalidInput)?;
    fs::create_dir_all(dir)?;
    let (Ok(w), Ok(h)) = (u32::try_from(img.width), u32::try_from(img.height)) else {
        return Err(io::ErrorKind::InvalidInput.into());
    };
    let mut data = Vec::with_capacity(HEADER_LEN + img.pixels.len());
    data.extend_from_slice(MAGIC);
    data.extend_from_slice(&w.to_le_bytes());
    data.extend_from_slice(&h.to_le_bytes());
    data.push(img.background.is_some() as u8);
    data.extend_from_slice(&img.background.unwrap_or_default());
    data.extend_from_slice(&img.pixels);

    let tmp = entry.with_extension(format!("tmp{}", process::id()));
    let result = fs::write(&tmp, &data).and_then(|()| fs::rename(&tmp, entry));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::{
        fs::File,
        time::{Duration, SystemTime},
    };

    use super::*;
    use crate::image::tests::png;

    /// A fresh, empty directory for one test.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("ffetch-test-{}-{name}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_image(background: Option<[u8; 3]>) -> Image {
        Image {
            width: 3,
            height: 2,
            pixels: (0..24).collect(),
            background,
        }
    }

    #[test]
    fn fnv1a_reference_values() {
        assert_eq!(fnv1a(&[]), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(&[b"a"]), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(&[b"foo", b"bar"]), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn key_changes_with_every_input() {
        let path = Path::new("/home/user/logo.png");
        let base = image_key(path, 1000, (1_700_000_000, 5), false);
        assert_eq!(base, image_key(path, 1000, (1_700_000_000, 5), false));
        assert_ne!(base, image_key(path, 1001, (1_700_000_000, 5), false));
        assert_ne!(base, image_key(path, 1000, (1_700_000_001, 5), false));
        assert_ne!(base, image_key(path, 1000, (1_700_000_000, 6), false));
        assert_ne!(base, image_key(path, 1000, (1_700_000_000, 5), true));
        assert_ne!(
            base,
            image_key(
                Path::new("/home/user/logo2.png"),
                1000,
                (1_700_000_000, 5),
                false
            )
        );
    }

    #[test]
    fn entry_round_trip() {
        let dir = temp_dir("round-trip");
        for background in [None, Some([195, 62, 88])] {
            let entry = dir.join("images").join("entry.rgba");
            let img = sample_image(background);
            write_image(&entry, &img).unwrap();
            assert_eq!(read_image(&entry), Some(img));
        }
        // Only the entry is left behind, no temporary files.
        assert_eq!(fs::read_dir(dir.join("images")).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_entries_are_misses() {
        let dir = temp_dir("malformed");
        let entry = dir.join("entry.rgba");
        assert_eq!(read_image(&entry), None);
        write_image(&entry, &sample_image(None)).unwrap();
        let good = fs::read(&entry).unwrap();

        let truncated = &good[..good.len() - 1];
        let mut bad_magic = good.clone();
        bad_magic[0] = b'X';
        let mut bad_flag = good.clone();
        bad_flag[16] = 7;
        let mut zero_width = good.clone();
        zero_width[8..12].copy_from_slice(&0u32.to_le_bytes());
        for data in [truncated, &bad_magic, &bad_flag, &zero_width, &good[..5]] {
            fs::write(&entry, data).unwrap();
            assert_eq!(read_image(&entry), None);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn image_is_cached_until_the_file_changes() {
        let dir = temp_dir("image");
        let file = dir.join("logo.png");
        let rgba: Vec<u8> = [255, 0, 0, 255].repeat(16);
        fs::write(&file, png(4, 4, &rgba)).unwrap();
        let cache = dir.join("cache");

        let first = image(&file, false, Some(&cache)).unwrap();
        let entries: Vec<PathBuf> = fs::read_dir(cache.join("images"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(read_image(&entries[0]), Some(first.clone()));

        // A doctored entry is served as long as the file is unchanged...
        let doctored = sample_image(None);
        write_image(&entries[0], &doctored).unwrap();
        assert_eq!(image(&file, false, Some(&cache)).unwrap(), doctored);
        // ...but not with different processing options...
        assert_ne!(image(&file, true, Some(&cache)).unwrap(), doctored);
        // ...or once the file is touched.
        let later = SystemTime::now() + Duration::from_secs(60);
        File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert_eq!(image(&file, false, Some(&cache)).unwrap(), first);

        // Without a cache directory it still works.
        assert_eq!(image(&file, false, None).unwrap(), first);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_file_is_an_error() {
        let dir = temp_dir("missing");
        assert!(image(&dir.join("nope.png"), false, Some(&dir)).is_err());
        assert!(
            image(&dir, false, Some(&dir)).is_err(),
            "a directory is not an image"
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
