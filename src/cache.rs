//! On-disk cache under `${XDG_CACHE_HOME:-~/.cache}/ffetch`.
//!
//! Processed `--image` logos are stored as `images/<key>.rgba`. The key hashes
//! the image's canonical path, size and mtime, so editing or replacing the file
//! gives it a new entry. Slow facts that only change across reboots are kept in
//! `facts.json` (see `Facts`). The cache is never essential: a missing or
//! malformed entry is recomputed, and a failed write is ignored.

use std::{
    collections::BTreeMap,
    env, fs, io,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
    process,
    sync::{Mutex, PoisonError},
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
    write_atomic(entry, &data)
}

/// Writes `path` atomically: to a temporary file, then renamed into place, so
/// readers never see a half-written file.
fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    fs::create_dir_all(path.parent().ok_or(io::ErrorKind::InvalidInput)?)?;
    let tmp = path.with_extension(format!("tmp{}", process::id()));
    let result = fs::write(&tmp, data).and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Fact name -> value; `None` records a lookup that found nothing.
type FactMap = BTreeMap<String, Option<String>>;

/// Facts that hold until the next boot, such as the Windows version on WSL,
/// kept in `facts.json` as `{ "boot_id": "…", "facts": { "<key>": "<value>" } }`.
/// A lookup that found nothing is stored as `null`, so machines without the
/// tools don't retry it on every run.
///
/// The file is read once before the modules run and written once after they
/// finish, so the module threads only share this struct, never the file.
pub struct Facts {
    file: Option<PathBuf>,
    boot_id: Option<String>,
    /// Facts from the file, when it is for this boot and not being refreshed.
    known: FactMap,
    /// Facts computed during this run.
    fresh: Mutex<FactMap>,
}

impl Facts {
    /// The facts cached in `file` for the boot `boot_id` (the contents of
    /// `/proc/sys/kernel/random/boot_id`). With `refresh`, each fact is
    /// computed again (and the result saved). Without a file nothing is kept.
    pub fn open(file: Option<PathBuf>, boot_id: Option<String>, refresh: bool) -> Facts {
        // Without a boot ID there's no telling when a fact goes stale, so
        // nothing is cached.
        let boot_id = boot_id
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty());
        let known = match (&file, &boot_id) {
            (Some(file), Some(boot_id)) if !refresh => fs::read_to_string(file)
                .ok()
                .and_then(|text| parse_facts(&text))
                .filter(|(id, _)| id == boot_id)
                .map(|(_, facts)| facts)
                .unwrap_or_default(),
            _ => FactMap::new(),
        };
        Facts {
            file,
            boot_id,
            known,
            fresh: Mutex::default(),
        }
    }

    /// The fact `key`: the cached value (which may be a cached `None`), or else
    /// `compute()`, which is remembered for `save`.
    pub fn get(&self, key: &str, compute: impl FnOnce() -> Option<String>) -> Option<String> {
        if let Some(value) = self.known.get(key) {
            return value.clone();
        }
        let value = compute();
        self.fresh
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.to_string(), value.clone());
        value
    }

    /// Writes the cached facts plus the ones computed this run, if there are any.
    pub fn save(&self) -> io::Result<()> {
        let fresh = self.fresh.lock().unwrap_or_else(PoisonError::into_inner);
        let (Some(file), Some(boot_id)) = (&self.file, &self.boot_id) else {
            return Ok(());
        };
        if fresh.is_empty() {
            return Ok(());
        }
        let mut facts = self.known.clone();
        facts.extend(fresh.iter().map(|(k, v)| (k.clone(), v.clone())));
        write_atomic(file, facts_json(boot_id, &facts).as_bytes())
    }
}

/// The facts file. Keys are sorted, so the same facts give the same file.
fn facts_json(boot_id: &str, facts: &FactMap) -> String {
    let entries: Vec<String> = facts
        .iter()
        .map(|(key, value)| {
            let value = value.as_deref().map_or("null".into(), json_string);
            format!("    {}: {value}", json_string(key))
        })
        .collect();
    let facts = match entries.is_empty() {
        true => "{}".to_string(),
        false => format!("{{\n{}\n  }}", entries.join(",\n")),
    };
    format!(
        "{{\n  \"boot_id\": {},\n  \"facts\": {facts}\n}}\n",
        json_string(boot_id)
    )
}

/// `s` as a JSON string literal.
fn json_string(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            '\n' => out += "\\n",
            '\r' => out += "\\r",
            '\t' => out += "\\t",
            c if c < ' ' => out += &format!("\\u{:04x}", c as u32),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The boot ID and facts from a facts file; `None` unless it has the shape
/// `facts_json` writes (in any key order or spacing).
fn parse_facts(text: &str) -> Option<(String, FactMap)> {
    let mut json = Json(text);
    let (mut boot_id, mut facts) = (None, FactMap::new());
    json.object(|json, key| match key.as_str() {
        "boot_id" => {
            boot_id = Some(json.string()?);
            Some(())
        }
        "facts" => json.object(|json, key| {
            let value = if json.eat("null") {
                None
            } else {
                Some(json.string()?)
            };
            facts.insert(key, value);
            Some(())
        }),
        _ => None,
    })?;
    json.0.trim().is_empty().then_some((boot_id?, facts))
}

/// A cursor over the little JSON the facts file uses: objects, strings and `null`.
struct Json<'a>(&'a str);

impl Json<'_> {
    /// Skips whitespace, then consumes `token` if it is next.
    fn eat(&mut self, token: &str) -> bool {
        let rest = self.0.trim_start().strip_prefix(token);
        if let Some(rest) = rest {
            self.0 = rest;
        }
        rest.is_some()
    }

    /// An object, calling `field` with each key to parse the value after it.
    fn object(&mut self, mut field: impl FnMut(&mut Self, String) -> Option<()>) -> Option<()> {
        if !self.eat("{") {
            return None;
        }
        if self.eat("}") {
            return Some(());
        }
        loop {
            let key = self.string()?;
            if !self.eat(":") {
                return None;
            }
            field(self, key)?;
            if self.eat("}") {
                return Some(());
            }
            if !self.eat(",") {
                return None;
            }
        }
    }

    /// A string, unescaped. `\u` escapes of UTF-16 surrogates are not supported;
    /// the writer never produces them.
    fn string(&mut self) -> Option<String> {
        if !self.eat("\"") {
            return None;
        }
        let text = self.0;
        let mut chars = text.char_indices();
        let mut out = String::new();
        loop {
            let (i, c) = chars.next()?;
            out.push(match c {
                '"' => {
                    self.0 = &text[i + 1..];
                    return Some(out);
                }
                '\\' => match chars.next()?.1 {
                    c @ ('"' | '\\' | '/') => c,
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    'u' => {
                        let code = (0..4)
                            .try_fold(0, |n, _| Some(n * 16 + chars.next()?.1.to_digit(16)?))?;
                        char::from_u32(code)?
                    }
                    _ => return None,
                },
                c if c < ' ' => return None,
                c => c,
            });
        }
    }
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

    /// Facts in `dir`, with a fake boot ID.
    fn facts(dir: &Path, boot_id: &str, refresh: bool) -> Facts {
        Facts::open(
            Some(dir.join("facts.json")),
            Some(format!("{boot_id}\n")),
            refresh,
        )
    }

    fn uncached() -> Option<String> {
        panic!("the fact should have come from the cache")
    }

    #[test]
    fn facts_round_trip_including_negative_results() {
        let dir = temp_dir("facts-round-trip");
        let first = facts(&dir, "boot-a", false);
        assert_eq!(
            first.get("windows", || Some("Windows 11".into())),
            Some("Windows 11".into())
        );
        assert_eq!(first.get("gpu", || None), None);
        first.save().unwrap();
        let text = fs::read_to_string(dir.join("facts.json")).unwrap();
        assert!(text.contains("\"gpu\": null"), "{text}");

        let second = facts(&dir, "boot-a", false);
        assert_eq!(second.get("windows", uncached), Some("Windows 11".into()));
        assert_eq!(
            second.get("gpu", uncached),
            None,
            "negative results are cached"
        );
        // A new fact is merged in; the old ones are kept.
        assert_eq!(second.get("other", || Some("x".into())), Some("x".into()));
        second.save().unwrap();
        let third = facts(&dir, "boot-a", false);
        assert_eq!(third.get("windows", uncached), Some("Windows 11".into()));
        assert_eq!(third.get("other", uncached), Some("x".into()));
        // Only the file is left behind, no temporary files.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn facts_from_another_boot_are_stale() {
        let dir = temp_dir("facts-boot");
        let old = facts(&dir, "boot-a", false);
        old.get("windows", || Some("Windows 10".into()));
        old.get("gpu", || Some("GPU".into()));
        old.save().unwrap();

        let new = facts(&dir, "boot-b", false);
        assert_eq!(
            new.get("windows", || Some("Windows 11".into())),
            Some("Windows 11".into())
        );
        new.save().unwrap();
        let text = fs::read_to_string(dir.join("facts.json")).unwrap();
        assert!(
            text.contains("boot-b") && !text.contains("boot-a"),
            "{text}"
        );
        assert!(!text.contains("GPU"), "stale facts are dropped: {text}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn refresh_recomputes_cached_facts() {
        let dir = temp_dir("facts-refresh");
        let old = facts(&dir, "boot-a", false);
        old.get("windows", || None);
        old.save().unwrap();

        let refreshed = facts(&dir, "boot-a", true);
        assert_eq!(
            refreshed.get("windows", || Some("Windows 11".into())),
            Some("Windows 11".into())
        );
        refreshed.save().unwrap();
        assert_eq!(
            facts(&dir, "boot-a", false).get("windows", uncached),
            Some("Windows 11".into())
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn facts_file_is_only_written_when_something_was_computed() {
        let dir = temp_dir("facts-no-write");
        facts(&dir, "boot-a", false).save().unwrap();
        assert!(!dir.join("facts.json").exists());

        let first = facts(&dir, "boot-a", false);
        first.get("windows", || Some("Windows 11".into()));
        first.save().unwrap();
        let before = fs::metadata(dir.join("facts.json"))
            .unwrap()
            .modified()
            .unwrap();
        let cached = facts(&dir, "boot-a", false);
        cached.get("windows", uncached);
        cached.save().unwrap();
        let after = fs::metadata(dir.join("facts.json"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after);

        // Without a boot ID or a cache directory nothing is cached or written.
        for unusable in [
            Facts::open(Some(dir.join("other.json")), None, false),
            Facts::open(Some(dir.join("other.json")), Some(" \n".into()), false),
            Facts::open(None, Some("boot-a".into()), false),
        ] {
            assert_eq!(
                unusable.get("windows", || Some("x".into())),
                Some("x".into())
            );
            unusable.save().unwrap();
        }
        assert!(!dir.join("other.json").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_facts_files_are_treated_as_empty() {
        let dir = temp_dir("facts-corrupt");
        let file = dir.join("facts.json");
        let good = r#"{"boot_id": "boot-a", "facts": {"windows": "Windows 11"}}"#;
        assert!(parse_facts(good).is_some());
        for bad in [
            "",
            "not json",
            &good[..good.len() - 1],
            r#"{"boot_id": "boot-a", "facts": {"windows": 11}}"#,
            r#"{"boot_id": "boot-a", "facts": {"windows": "Windows 11",}}"#,
            r#"{"boot_id": "boot-a", "facts": {"windows": "Windows 11"}} trailing"#,
            r#"{"boot_id": "boot-a", "facts": {"windows": "bad \q escape"}}"#,
            "{\"boot_id\": \"boot-a\", \"facts\": {\"windows\": \"raw\ncontrol\"}}",
            r#"{"boot_id": "boot-a", "facts": {"windows": "\ud800"}}"#,
            r#"{"boot_id": "boot-a", "extra": "x", "facts": {}}"#,
            r#"{"facts": {"windows": "Windows 11"}}"#,
        ] {
            assert_eq!(parse_facts(bad), None, "{bad:?}");
            fs::write(&file, bad).unwrap();
            let corrupt = facts(&dir, "boot-a", false);
            assert_eq!(
                corrupt.get("windows", || Some("fresh".into())),
                Some("fresh".into())
            );
            corrupt.save().unwrap();
            assert_eq!(
                facts(&dir, "boot-a", false).get("windows", uncached),
                Some("fresh".into())
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn facts_json_escapes_and_unescapes() {
        assert_eq!(json_string("plain"), r#""plain""#);
        assert_eq!(
            json_string("a\"b\\c\nd\te\u{1}f\u{1f}"),
            r#""a\"b\\c\nd\te\u0001f\u001f""#
        );
        assert_eq!(json_string("Grafik ™ 日本"), "\"Grafik ™ 日本\"");

        let tricky = [
            "quote \" backslash \\ slash /",
            "lines\r\nand\ttabs",
            "\u{0}\u{8}\u{c}\u{7f}",
            "Intel(R) UHD Graphics\nNVIDIA T1200 Laptop GPU",
            "ünïcödé ™ 日本 😀",
            "",
        ];
        let facts: FactMap = tricky
            .iter()
            .enumerate()
            .map(|(i, v)| (format!("key \"{i}\"\\"), Some(v.to_string())))
            .chain([("absent".to_string(), None)])
            .collect();
        let text = facts_json("boot \"id\"", &facts);
        assert_eq!(parse_facts(&text), Some(("boot \"id\"".to_string(), facts)));

        // Escapes the writer never produces are still understood.
        let text = r#" { "facts" : { "k" : "\/\b\féé" } , "boot_id" : "b" } "#;
        let (boot_id, facts) = parse_facts(text).unwrap();
        assert_eq!(boot_id, "b");
        assert_eq!(facts["k"].as_deref(), Some("/\u{8}\u{c}éé"));
        assert_eq!(
            parse_facts(r#"{"boot_id": "b", "facts": {}}"#),
            Some(("b".into(), FactMap::new()))
        );
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
