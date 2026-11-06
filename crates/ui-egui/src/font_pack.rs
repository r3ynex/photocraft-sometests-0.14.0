//! Portable RE:ICA Manga Lab font-group manifests (.pcraftfp).
//!
//! A pack contains family names, not font binaries. Imported names are retained
//! even when fonts are unavailable on this machine, so a missing installation
//! does not silently destroy a shared group's contents.
use std::collections::HashSet;

use photocraft_engine::prefs::{Preferences, TextLibraryFontGroup};
use serde::{Deserialize, Serialize};

pub const EXTENSION: &str = "pcraftfp";
const MAGIC: &str = "reica.mangalab.font-pack";
const VERSION: u32 = 1;
const MAX_FILE_BYTES: usize = 128 * 1024;
const MAX_FONTS: usize = 512;
const MAX_GROUP_CHARS: usize = 64;
const MAX_FAMILY_CHARS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FontPack {
    format: String,
    version: u32,
    pub group: String,
    pub fonts: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportSummary {
    pub group: String,
    pub added: usize,
    pub skipped_duplicates: usize,
    pub created: bool,
}

fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

fn validate_group(s: &str) -> Result<(), String> {
    if s.trim().is_empty() || s.chars().count() > MAX_GROUP_CHARS
        || s.chars().any(char::is_control)
    {
        return Err("Font pack group must be a nonempty name of at most 64 characters".into());
    }
    Ok(())
}

fn validate_font(s: &str) -> Result<(), String> {
    if s.trim().is_empty() || s.chars().count() > MAX_FAMILY_CHARS
        || s.chars().any(char::is_control)
    {
        return Err("Font pack has an empty, invalid, or too-long font family".into());
    }
    Ok(())
}

fn normalized_fonts(fonts: &[String]) -> Result<Vec<String>, String> {
    if fonts.len() > MAX_FONTS {
        return Err(format!("Font pack exceeds the limit of {MAX_FONTS} font families"));
    }
    let mut out = Vec::with_capacity(fonts.len());
    let mut unique = HashSet::new();
    for family in fonts {
        validate_font(family)?;
        let name = family.trim();
        if unique.insert(name.to_lowercase()) {
            out.push(name.to_string());
        }
    }
    Ok(out)
}

/// Produce a versioned, readable UTF-8 manifest. Does not copy or redistribute font files.
pub fn encode(group: &str, fonts: &[String]) -> Result<String, String> {
    validate_group(group)?;
    let pack = FontPack {
        format: MAGIC.to_string(),
        version: VERSION,
        group: group.trim().to_string(),
        fonts: normalized_fonts(fonts)?,
    };
    let text = serde_json::to_string_pretty(&pack).map_err(|e| e.to_string())?;
    if text.len() > MAX_FILE_BYTES {
        return Err("Font pack is too large to export".into());
    }
    Ok(text)
}

/// Import only a supported manifest version. No implicit filesystem access.
pub fn decode(bytes: &[u8]) -> Result<FontPack, String> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err("Font pack file is too large (128 KiB maximum)".into());
    }
    let mut pack: FontPack = serde_json::from_slice(bytes)
        .map_err(|e| format!("Invalid .pcraftfp file: {e}"))?;
    if pack.format != MAGIC {
        return Err("Not a RE:ICA Manga Lab font pack (.pcraftfp)".into());
    }
    if pack.version != VERSION {
        return Err(format!("Unsupported font pack version {} (supported: {VERSION})", pack.version));
    }
    validate_group(&pack.group)?;
    pack.group = pack.group.trim().to_string();
    pack.fonts = normalized_fonts(&pack.fonts)?;
    Ok(pack)
}

/// Merge into an existing same-name group; create it otherwise. Keep ordering,
/// case-insensitive uniqueness and unknown (not installed) family names.
pub fn merge(prefs: &mut Preferences, pack: FontPack) -> ImportSummary {
    let mut created = false;
    let group = if same_name(&pack.group, "Default") {
        "Default".to_string()
    } else if let Some(existing) = prefs.text_library_groups.iter().find(|g| same_name(&g.name, &pack.group)) {
        existing.name.clone()
    } else {
        created = true;
        prefs.text_library_groups.push(TextLibraryFontGroup { name: pack.group.clone(), fonts: Vec::new() });
        pack.group.clone()
    };
    let current = if group == "Default" {
        &mut prefs.text_library_fonts
    } else {
        &mut prefs.text_library_groups.iter_mut().find(|g| g.name == group)
            .expect("font group was created or found").fonts
    };
    let mut unique: HashSet<String> = current.iter().map(|f| f.to_lowercase()).collect();
    let mut added = 0;
    let mut skipped_duplicates = 0;
    for name in pack.fonts {
        if unique.insert(name.to_lowercase()) {
            current.push(name);
            added += 1;
        } else {
            skipped_duplicates += 1;
        }
    }
    prefs.text_library_active_group = group.clone();
    ImportSummary { group, added, skipped_duplicates, created }
}

/// Safe, portable default filename from the group label.
pub fn suggested_filename(name: &str) -> String {
    let clean = name.chars().map(|ch| {
        if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            '_'
        } else {
            ch
        }
    }).collect::<String>();
    let stem = clean.trim().trim_matches('.').trim();
    let stem = if stem.is_empty() { "FontGroup" } else { stem };
    format!("{stem}.{EXTENSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_roundtrip_keeps_name_unicode_and_order() {
        let pack = decode(encode("Звуки / SFX", &[
            "Anime Ace".into(), "Моя гарнитура".into(), "anime ace".into()
        ]).unwrap().as_bytes()).unwrap();
        assert_eq!(pack.group, "Звуки / SFX");
        assert_eq!(pack.fonts, vec!["Anime Ace", "Моя гарнитура"]);
        assert_eq!(suggested_filename(&pack.group), "Звуки _ SFX.pcraftfp");
    }

    #[test]
    fn import_merges_without_destroying_existing_order_or_unknown_fonts() {
        let mut prefs = Preferences::default();
        prefs.text_library_groups.push(TextLibraryFontGroup {
            name: "SFX".into(), fonts: vec!["One".into(), "Two".into()],
        });
        let pack = decode(encode("sfx", &[
            "two".into(), "Missing font on this computer".into(), "Three".into(),
        ]).unwrap().as_bytes()).unwrap();
        let result = merge(&mut prefs, pack);
        assert!(!result.created);
        assert_eq!(result.group, "SFX");
        assert_eq!(result.added, 2);
        assert_eq!(result.skipped_duplicates, 1);
        assert_eq!(prefs.text_library_groups[0].fonts,
            vec!["One", "Two", "Missing font on this computer", "Three"]);
    }

    #[test]
    fn default_group_merges_into_existing_default_without_duplicate_group() {
        let mut prefs = Preferences::default();
        prefs.text_library_fonts = vec!["Fira Sans".into()];
        let pack = decode(encode("DEFAULT", &["fira sans".into(), "Rubik".into()]).unwrap().as_bytes()).unwrap();
        let result = merge(&mut prefs, pack);
        assert_eq!(result.group, "Default");
        assert_eq!(result.added, 1);
        assert_eq!(prefs.text_library_fonts, vec!["Fira Sans", "Rubik"]);
        assert!(prefs.text_library_groups.is_empty());
    }

    #[test]
    fn reject_wrong_type_version_and_oversized_packs() {
        let valid = encode("Fonts", &["My Font".into()]).unwrap();
        let wrong_type = valid.replace(MAGIC, "other.format");
        assert!(decode(wrong_type.as_bytes()).is_err());
        let wrong_version = valid.replace("\"version\": 1", "\"version\": 99");
        assert!(decode(wrong_version.as_bytes()).is_err());
        assert!(decode(&vec![b' '; MAX_FILE_BYTES + 1]).is_err());
        assert!(encode("bad\nname", &["Good".into()]).is_err());
        assert!(encode("Fonts", &["".into()]).is_err());
    }
}
