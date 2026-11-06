//! RE:ICA Manga Lab portable font packs (.pcraftfp).
//! A .pcraftfp file is a ZIP containing manifest.json and the actual OpenType
//! / TrueType binaries for each font family, including additional faces.
//! Font distribution is subject to the original fonts' licences.
use std::collections::HashSet;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use photocraft_engine::prefs::{Preferences, TextLibraryFontGroup};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;

pub const EXTENSION: &str = "pcraftfp";
const MAGIC: &str = "reica.mangalab.font-pack";
const VERSION: u32 = 2;
const MAX_ARCHIVE: usize = 160 * 1024 * 1024;
const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 128 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 128 * 1024;
const MAX_FONTS: usize = 512;
const MAX_BINARY_FILES: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: String,
    version: u32,
    group: String,
    fonts: Vec<FontRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FontRecord {
    family: String,
    files: Vec<String>,
}

/// Decoded font family with all its actual font source binaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontFamily {
    pub family: String,
    pub files: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontPack {
    pub group: String,
    pub fonts: Vec<FontFamily>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportSummary {
    pub group: String,
    pub added: usize,
    pub skipped_duplicates: usize,
    pub created: bool,
    pub installed_files: usize,
}

fn validate_name(value: &str, maximum: usize, what: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.chars().count() > maximum || value.chars().any(char::is_control) {
        return Err(format!("{what} is empty or invalid (maximum {maximum} characters)"));
    }
    Ok(())
}

fn extension(bytes: &[u8]) -> Result<&'static str, String> {
    if bytes.starts_with(b"OTTO") { Ok("otf") }
    else if bytes.starts_with(b"ttcf") { Ok("ttc") }
    else if bytes.starts_with(b"true") || bytes.starts_with(&[0, 1, 0, 0]) { Ok("ttf") }
    else { Err("A font in the pack is not a supported TTF, OTF or font collection".into()) }
}

/// Export a complete ZIP pack from the binaries already resolved by the text engine.
/// Missing families must fail explicitly: silently dropping files would create
/// a misleading "complete" backup.
pub fn encode(group: &str, families: &[FontFamily]) -> Result<Vec<u8>, String> {
    validate_name(group, 64, "Group name")?;
    if families.len() > MAX_FONTS {
        return Err("Too many font families in group".into());
    }
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut manifest = Manifest { format: MAGIC.into(), version: VERSION, group: group.trim().into(), fonts: Vec::new() };
    let mut total = 0usize;
    let mut file_count = 0usize;
    let mut names = HashSet::new();
    for (family_index, family) in families.iter().enumerate() {
        validate_name(&family.family, 256, "Font family")?;
        if !names.insert(family.family.to_lowercase()) {
            continue;
        }
        if family.files.is_empty() {
            return Err(format!("Could not find source font files for {}", family.family));
        }
        let mut files = Vec::new();
        for (face_index, data) in family.files.iter().enumerate() {
            if data.len() > MAX_FONT_BYTES || data.is_empty() {
                return Err(format!("Font file for {} exceeds the 32 MiB limit", family.family));
            }
            total = total.saturating_add(data.len());
            file_count += 1;
            if total > MAX_TOTAL_BYTES || file_count > MAX_BINARY_FILES {
                return Err("Font group exceeds the portable pack size limit".into());
            }
            let ext = extension(data)?;
            let filename = format!("fonts/{family_index:04}-{face_index:03}.{ext}");
            writer.start_file(&filename, options).map_err(|e| e.to_string())?;
            writer.write_all(data).map_err(|e| e.to_string())?;
            files.push(filename);
        }
        manifest.fonts.push(FontRecord { family: family.family.clone(), files });
    }
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err("Font pack manifest is too large".into());
    }
    writer.start_file("manifest.json", options).map_err(|e| e.to_string())?;
    writer.write_all(&manifest_bytes).map_err(|e| e.to_string())?;
    let result = writer.finish().map_err(|e| e.to_string())?.into_inner();
    if result.len() > MAX_ARCHIVE {
        return Err("Font pack archive exceeds the 160 MiB limit".into());
    }
    Ok(result)
}

/// Validate a ZIP pack without extracting arbitrary paths to disk.
/// Only names listed in a small manifest are read, with per-file/total
/// decompressed-size limits (including ZIP-bomb resistance).
pub fn decode(bytes: &[u8]) -> Result<FontPack, String> {
    if bytes.len() > MAX_ARCHIVE {
        return Err("Font pack archive exceeds the 160 MiB limit".into());
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| format!("Invalid .pcraftfp ZIP archive: {e}"))?;
    let manifest = {
        let mut entry = zip.by_name("manifest.json")
            .map_err(|_| "Font pack has no manifest.json".to_string())?;
        if entry.size() as usize > MAX_MANIFEST_BYTES {
            return Err("Font pack manifest is too large".into());
        }
        let mut contents = Vec::new();
        entry.take(MAX_MANIFEST_BYTES as u64 + 1).read_to_end(&mut contents).map_err(|e| e.to_string())?;
        if contents.len() > MAX_MANIFEST_BYTES {
            return Err("Font pack manifest exceeded its size limit".into());
        }
        serde_json::from_slice::<Manifest>(&contents).map_err(|e| format!("Invalid font pack manifest: {e}"))?
    };
    if manifest.format != MAGIC {
        return Err("Not a RE:ICA Manga Lab font pack".into());
    }
    if manifest.version != VERSION {
        return Err(format!("Unsupported font pack version {} (expected {VERSION})", manifest.version));
    }
    validate_name(&manifest.group, 64, "Group name")?;
    if manifest.fonts.len() > MAX_FONTS {
        return Err("Font pack contains too many families".into());
    }
    let mut families = Vec::new();
    let mut total = 0usize;
    let mut seen_fonts = HashSet::new();
    let mut seen_paths = HashSet::new();
    let mut count = 0usize;
    for item in manifest.fonts {
        validate_name(&item.family, 256, "Font family")?;
        if !seen_fonts.insert(item.family.to_lowercase()) || item.files.is_empty() {
            return Err("Font pack has duplicate families or a family without files".into());
        }
        let mut files = Vec::new();
        for path in item.files {
            count += 1;
            if count > MAX_BINARY_FILES || !seen_paths.insert(path.clone())
                || !path.starts_with("fonts/")
                || path[6..].contains('/')
                || path[6..].contains('\\')
                || !path[6..].bytes().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'.' | b'-' | b'_'))
                || path.len() > 96
            {
                return Err("Unsafe or duplicate font entry path in .pcraftfp".into());
            }
            let mut entry = zip.by_name(&path).map_err(|e| format!("Missing font file {path}: {e}"))?;
            if entry.size() as usize > MAX_FONT_BYTES {
                return Err("Font binary exceeds the 32 MiB limit".into());
            }
            let mut data = Vec::new();
            entry.take(MAX_FONT_BYTES as u64 + 1).read_to_end(&mut data).map_err(|e| e.to_string())?;
            if data.len() > MAX_FONT_BYTES {
                return Err("Font binary expanded beyond the size limit".into());
            }
            let _ = extension(&data)?;
            total = total.saturating_add(data.len());
            if total > MAX_TOTAL_BYTES {
                return Err("Font pack expanded beyond its 128 MiB limit".into());
            }
            files.push(data);
        }
        families.push(FontFamily { family: item.family, files });
    }
    Ok(FontPack { group: manifest.group, fonts: families })
}

/// Merge into the same-name group, or create it. Preserve existing entries
/// and retain names even if a font isn't installed system-wide.
pub fn merge(prefs: &mut Preferences, group: &str, names: &[String]) -> ImportSummary {
    let mut created = false;
    let group = if group.eq_ignore_ascii_case("Default") {
        "Default".to_string()
    } else if let Some(existing) = prefs.text_library_groups.iter().find(|g| g.name.eq_ignore_ascii_case(group)) {
        existing.name.clone()
    } else {
        created = true;
        prefs.text_library_groups.push(TextLibraryFontGroup { name: group.into(), fonts: Vec::new() });
        group.to_string()
    };
    let current = if group == "Default" {
        &mut prefs.text_library_fonts
    } else {
        match prefs.text_library_groups.iter_mut().find(|g| g.name == group) {
            Some(found) => &mut found.fonts,
            None => return ImportSummary { group, added: 0, skipped_duplicates: 0, created, installed_files: 0 },
        }
    };
    let mut existing: HashSet<String> = current.iter().map(|s| s.to_lowercase()).collect();
    let mut added = 0;
    let mut skipped_duplicates = 0;
    for name in names {
        if existing.insert(name.to_lowercase()) {
            current.push(name.clone());
            added += 1;
        } else {
            skipped_duplicates += 1;
        }
    }
    prefs.text_library_active_group = group.clone();
    ImportSummary { group, added, skipped_duplicates, created, installed_files: 0 }
}

pub fn suggested_filename(name: &str) -> String {
    let safe = name.chars().map(|ch| {
        if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { ch }
    }).collect::<String>();
    let stem = safe.trim().trim_matches('.').trim();
    format!("{}.{}", if stem.is_empty() { "FontGroup" } else { stem }, EXTENSION)
}

/// Save the imported font files to the editor's own data folder. No paths
/// from ZIP are used on disk; names are derived solely from the content.
/// This installs fonts **for the editor**, not system-wide.
#[cfg(not(target_arch = "wasm32"))]
pub fn persist_fonts(directory: &Path, pack: &FontPack) -> Result<usize, String> {
    use std::hash::{Hash, Hasher};
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let mut stored = HashSet::new();
    let mut count = 0usize;
    for family in &pack.fonts {
        for data in &family.files {
            // Reject unparseable sources before saving or changing preferences.
            let checked = photocraft_text::FontDb::new().register_font_data(data.clone());
            if !checked.iter().any(|name| name.eq_ignore_ascii_case(&family.family)) {
                return Err(format!("Packed font data does not match family {}", family.family));
            }
            let mut h = std::collections::hash_map::DefaultHasher::new();
            data.hash(&mut h);
            let ext = extension(data)?;
            let filename = format!("{:016x}-{:08x}.{ext}", h.finish(), data.len());
            if !stored.insert(filename.clone()) { continue; }
            let target = directory.join(filename);
            if target.exists() {
                let existing = std::fs::read(&target).map_err(|e| e.to_string())?;
                if existing != *data {
                    return Err("A saved font file has a hash collision or is corrupted".into());
                }
            } else {
                photocraft_format::atomic_write(&target, data).map_err(|e| e.to_string())?;
                count += 1;
            }
        }
    }
    Ok(count)
}

#[cfg(target_arch = "wasm32")]
pub fn persist_fonts(_directory: &Path, _pack: &FontPack) -> Result<usize, String> {
    Err("Installing font files is not supported in the browser build".into())
}

/// Font packs must survive application restarts: load every validated binary
/// stored under the app's private FontPacks directory. Only generated file
/// extensions are considered; unlike the ZIP reader no nested paths are used.
#[cfg(not(target_arch = "wasm32"))]
pub fn load_persisted_fonts(directory: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(directory) else { return 0 };
    let mut files = Vec::new();
    for item in rd.flatten().take(MAX_BINARY_FILES) {
        let path = item.path();
        if !path.is_file() { continue; }
        if !matches!(path.extension().and_then(|e| e.to_str()), Some("ttf" | "otf" | "ttc")) { continue; }
        if let Ok(meta) = path.metadata() {
            if meta.len() as usize > MAX_FONT_BYTES { continue; }
        }
        if let Ok(data) = std::fs::read(path) {
            if extension(&data).is_ok() { files.push(data); }
        }
    }
    if files.is_empty() { return 0 }
    let mut imported = 0;
    if let Ok(mut engine) = photocraft_text::shared().lock() {
        for bytes in files {
            if !engine.fonts.register_font_data(bytes).is_empty() { imported += 1; }
        }
    }
    imported
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> Vec<u8> {
        photocraft_text::fonts::INTER_REGULAR.clone()
    }

    #[test]
    fn font_pack_roundtrip_includes_actual_binary_and_unicode_group() {
        let bytes = encode("Манга / SFX", &[FontFamily {
            family: "Inter".into(), files: vec![font()],
        }]).unwrap();
        assert!(bytes.starts_with(b"PK"));
        let archive = decode(&bytes).unwrap();
        assert_eq!(archive.group, "Манга / SFX");
        assert_eq!(archive.fonts[0].family, "Inter");
        assert_eq!(archive.fonts[0].files[0], font());
        assert_eq!(suggested_filename(&archive.group), "Манга _ SFX.pcraftfp");
    }

    #[test]
    fn group_import_merges_without_overwriting_prior_fonts() {
        let mut prefs = Preferences::default();
        prefs.text_library_groups.push(TextLibraryFontGroup { name: "SFX".into(), fonts: vec!["First".into()] });
        let r = merge(&mut prefs, "sfx", &["first".into(), "Inter".into()]);
        assert_eq!(r.group, "SFX");
        assert_eq!(r.added, 1);
        assert_eq!(r.skipped_duplicates, 1);
        assert_eq!(prefs.text_library_groups[0].fonts, ["First", "Inter"]);
    }

    #[test]
    fn rejects_invalid_or_oversized_archives() {
        assert!(decode(b"{\"group\":\"empty\"}").is_err());
        let bytes = encode("Group", &[FontFamily { family: "Inter".into(), files: vec![font()] }]).unwrap();
        let broken = bytes[..bytes.len() / 2].to_vec();
        assert!(decode(&broken).is_err());
        assert!(decode(&vec![0; MAX_ARCHIVE + 1]).is_err());
        assert!(encode("line\nbreak", &[]).is_err());
    }
}
