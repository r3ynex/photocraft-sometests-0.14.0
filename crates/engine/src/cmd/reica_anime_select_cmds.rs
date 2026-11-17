//! SkyTNT anime-character segmentation result -> native, undoable PhotoCraft selection.
//! Inference happens only in the optional local desktop helper; the engine accepts a mask and
//! never downloads models or changes existing artwork. The mask combines with normal selections.
use std::io::Read;

use base64::Engine as _;
use flate2::read::ZlibDecoder;
use photocraft_algo::selection::{self as selection, SelectionMode};
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

const COMMAND: &str = "reica.animeSelection.apply";
const MAX_PIXELS: u64 = 12_000_000;

fn invalid(message: &str) -> EngineError {
    EngineError::Other(format!("Anime Selection: {message}"))
}

fn decode_mask(encoded: &str, expected: usize) -> Result<Vec<u8>> {
    if expected == 0 || expected as u64 > MAX_PIXELS {
        return Err(invalid("mask dimensions exceed 12 megapixels"));
    }
    // Base64 is a compressed GRAY8 mask, not an executable or an image file.
    // Bound the compressed input and decompressed output to prevent memory bombs.
    if encoded.len() > 24_000_000 {
        return Err(invalid("mask payload exceeds 24 MiB"));
    }
    let compressed = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| invalid("invalid base64 mask"))?;
    let mut decoder = ZlibDecoder::new(compressed.as_slice()).take((expected + 1) as u64);
    let mut result = Vec::with_capacity(expected);
    decoder.read_to_end(&mut result).map_err(|_| invalid("invalid zlib mask"))?;
    if result.len() != expected {
        return Err(invalid("mask dimensions do not match the canvas"));
    }
    Ok(result)
}

fn apply(s: &mut Session, params: &Value) -> Result<Value> {
    let id = params.get("docId").and_then(Value::as_u64).ok_or_else(|| invalid("missing source document ID"))?;
    let w = params.get("width").and_then(Value::as_u64).ok_or_else(|| invalid("missing mask width"))?;
    let h = params.get("height").and_then(Value::as_u64).ok_or_else(|| invalid("missing mask height"))?;
    let expected = w.checked_mul(h).filter(|n| *n <= MAX_PIXELS && *n > 0).ok_or_else(|| invalid("invalid mask dimensions"))?;
    let encoded = params.get("mask").and_then(Value::as_str).ok_or_else(|| invalid("missing compressed mask"))?;
    let current = s.active().ok_or(EngineError::NoDocument)?;
    let bounds = current.doc.bounds();
    if current.doc.id.0 != id || u64::from(bounds.width()) != w || u64::from(bounds.height()) != h {
        return Err(invalid("document changed while segmentation was running"));
    }
    let raw = decode_mask(encoded, expected as usize)?;
    if raw.iter().all(|n| *n == 0) {
        return Ok(json!({"changed":false,"selected":current.doc.selection.is_some()}));
    }
    let coverage: Vec<f32> = raw.into_iter().map(|n| f32::from(n) / 255.0).collect();
    let mode = match params.get("mode").and_then(Value::as_str).unwrap_or("replace") {
        "replace" => SelectionMode::Replace,
        "add" => SelectionMode::Add,
        "subtract" => SelectionMode::Subtract,
        "intersect" => SelectionMode::Intersect,
        _ => return Err(invalid("unknown selection mode")),
    };
    let selected = s.edit("Anime Characters (SkyTNT)", |doc, _| {
        doc.selection = selection::combine(doc.selection.as_ref(), &coverage, bounds, mode);
        Ok(doc.selection.is_some())
    })?;
    Ok(json!({"changed":true,"selected":selected}))
}

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "open a document first".into())
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: COMMAND,
        label: "Anime Characters (SkyTNT)",
        menu: &[],
        shortcut: None,
        params: r##"{"docId":number,"width":number,"height":number,"mask":base64_zlib_GRAY8,"mode":"replace|add|subtract|intersect"}"##,
        enabled: has_doc,
        run: apply,
        journal: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write;

    fn packed(input: &[u8]) -> String {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(input).unwrap();
        base64::engine::general_purpose::STANDARD.encode(encoder.finish().unwrap())
    }

    #[test]
    fn masks_roundtrip_losslessly_with_soft_edges() {
        let alpha = [0, 1, 55, 125, 245, 255, 127, 0, 0];
        assert_eq!(decode_mask(&packed(&alpha), alpha.len()).unwrap(), alpha);
    }

    #[test]
    fn invalid_or_oversized_masks_are_rejected() {
        assert!(decode_mask(&packed(&[255; 4]), 5).is_err());
        assert!(decode_mask(&packed(&[255; 5]), 4).is_err());
        assert!(decode_mask(&packed(&[0; 4]), 0).is_err());
        assert!(decode_mask("not base64!", 4).is_err());
        assert!(decode_mask(&packed(&[0; 4]), (MAX_PIXELS + 1) as usize).is_err());
    }
}
