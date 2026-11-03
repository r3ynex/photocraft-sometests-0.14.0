//! Editable, per-character text effects for the Text Library.
//!
//! Effects rewrite character *style runs*, not the text or layer type. A single
//! engine command makes the operation one undo/redo step and refreshes its TySh.
use photocraft_doc::TextLayer;
use photocraft_doc::text::TextRun;
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::type_cmds::{TYPE_SIZE_MAX_PT, TYPE_SIZE_MIN_PT, with_text_targets};
use crate::{EngineError, Result, Session};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Random,
    Taper,
    Alternate,
}

#[derive(Clone, Copy, Debug)]
struct Options {
    mode: Mode,
    min_percent: f32,
    max_percent: f32,
    min_slant: f32,
    max_slant: f32,
    end_size: f32,
    reverse: bool,
    seed: u64,
}

fn invalid(why: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: "type.textEffect".into(), msg: why.into() }
}

fn number(p: &Value, field: &str, default: f32, low: f32, high: f32) -> Result<f32> {
    let v = match p.get(field) {
        Some(value) => value.as_f64().ok_or_else(|| invalid(format!("{field} must be a number")))? as f32,
        None => default,
    };
    if !v.is_finite() || !(low..=high).contains(&v) {
        return Err(invalid(format!("{field} must be between {low} and {high}")));
    }
    Ok(v)
}

fn options(p: &Value) -> Result<Options> {
    let mode = match p.get("mode").and_then(Value::as_str).unwrap_or("random") {
        "random" => Mode::Random,
        "taper" => Mode::Taper,
        "alternate" => Mode::Alternate,
        _ => return Err(invalid("mode must be random, taper, or alternate")),
    };
    let min_percent = number(p, "minPercent", 70.0, 10.0, 400.0)?;
    let max_percent = number(p, "maxPercent", 130.0, 10.0, 400.0)?;
    let min_slant = number(p, "minSlant", -12.0, -30.0, 30.0)?;
    let max_slant = number(p, "maxSlant", 12.0, -30.0, 30.0)?;
    let end_size = number(p, "endSize", 8.0, TYPE_SIZE_MIN_PT as f32, TYPE_SIZE_MAX_PT as f32)?;
    if min_percent > max_percent || min_slant > max_slant {
        return Err(invalid("minimum must not exceed maximum"));
    }
    let reverse = match p.get("reverse") {
        Some(v) => v.as_bool().ok_or_else(|| invalid("reverse must be a boolean"))?,
        None => false,
    };
    let seed = match p.get("seed") {
        Some(v) => v.as_u64().ok_or_else(|| invalid("seed must be a nonnegative integer"))?,
        None => 1,
    };
    Ok(Options { mode, min_percent, max_percent, min_slant, max_slant, end_size, reverse, seed })
}

/// SplitMix64: predictable across systems and repeatable given the same seed.
/// Separate lanes avoid accidentally coupling random size and random slant.
fn unit(seed: u64, index: usize, lane: u64) -> f32 {
    let mut z = seed.wrapping_add((index as u64).wrapping_mul(0x9e3779b97f4a7c15)).wrapping_add(lane.wrapping_mul(0x94d049bb133111eb));
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^= z >> 31;
    ((z >> 40) as f32) / 16_777_215.0
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn apply(t: &mut TextLayer, (start, end): (usize, usize), o: Options) {
    if start >= end || t.text.is_empty() {
        return;
    }
    let old = t.char_runs();
    let total = t.text.char_indices()
        .filter(|(at, ch)| *at >= start && *at < end && *ch != '\n' && *ch != '\r')
        .count();
    if total == 0 {
        return;
    }

    let mut next: Vec<TextRun> = Vec::new();
    let mut old_index = 0;
    let mut old_end = old[0].len;
    let mut first_size = None;
    let mut nth = 0usize;
    for (at, ch) in t.text.char_indices() {
        while at >= old_end && old_index + 1 < old.len() {
            old_index += 1;
            old_end += old[old_index].len;
        }
        let mut style = old[old_index].style.clone();
        if at >= start && at < end && ch != '\n' && ch != '\r' {
            let initial = *first_size.get_or_insert(style.size_pt);
            let fraction = if total <= 1 { 0.0 } else { nth as f32 / (total - 1) as f32 };
            match o.mode {
                Mode::Random => {
                    let pct = lerp(o.min_percent, o.max_percent, unit(o.seed, nth, 0));
                    style.size_pt = (style.size_pt * pct / 100.0).clamp(TYPE_SIZE_MIN_PT as f32, TYPE_SIZE_MAX_PT as f32);
                    style.slant_deg = lerp(o.min_slant, o.max_slant, unit(o.seed, nth, 1));
                }
                Mode::Taper => {
                    let destination = o.end_size.min(initial);
                    let progress = if o.reverse { 1.0 - fraction } else { fraction };
                    style.size_pt = lerp(initial, destination, progress);
                }
                Mode::Alternate => {
                    let large = nth % 2 == 0;
                    let pct = if large { o.max_percent } else { o.min_percent };
                    style.size_pt = (style.size_pt * pct / 100.0).clamp(TYPE_SIZE_MIN_PT as f32, TYPE_SIZE_MAX_PT as f32);
                    style.slant_deg = if large { o.max_slant } else { o.min_slant };
                }
            }
            nth += 1;
        }
        let len = ch.len_utf8();
        if let Some(last) = next.last_mut().filter(|last| last.style == style) {
            last.len += len;
        } else {
            next.push(TextRun { len, style });
        }
    }
    t.runs = next;
    t.sync_summary();
}

fn target_available(s: &Session) -> std::result::Result<(), String> {
    crate::type_cmds::has_format_target(s)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "type.textEffect",
        label: "Apply Editable Text Effect",
        menu: &[],
        shortcut: None,
        params: r##"{"layer":id?,"range":[startChar,endChar]?,"mode":"random|taper|alternate","minPercent":10..400,"maxPercent":10..400,"minSlant":-30..30 degrees,"maxSlant":-30..30 degrees,"endSize":0.1..1296 pt,"reverse":bool,"seed":u64}"##,
        enabled: target_available,
        journal: true,
        run: |s, p| {
            let opts = options(p)?;
            let ids = with_text_targets(s, p, "Editable Text Effect", |t, range| {
                apply(t, range, opts);
                Ok(())
            })?;
            Ok(json!({ "layers": ids.iter().map(|id| id.0).collect::<Vec<_>>(), "editable": true }))
        },
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_doc::text::CharStyle;

    fn text(input: &str, size: f32) -> TextLayer {
        TextLayer { text: input.into(), size_pt: size, ..Default::default() }
    }

    fn chars(t: &TextLayer) -> Vec<(char, CharStyle)> {
        let runs = t.char_runs();
        let mut result = Vec::new();
        let mut at = 0;
        for run in runs {
            for ch in t.text[at..at + run.len].chars() {
                result.push((ch, run.style.clone()));
            }
            at += run.len;
        }
        result
    }

    #[test]
    fn taper_keeps_text_editable_and_reverses_direction() {
        let opts = Options { mode: Mode::Taper, min_percent: 70.0, max_percent: 130.0, min_slant: -12.0, max_slant: 12.0, end_size: 10.0, reverse: false, seed: 1 };
        let mut normal = text("TEST", 40.0);
        apply(&mut normal, (0, 4), opts);
        assert_eq!(chars(&normal).iter().map(|(_, c)| c.size_pt.round() as i32).collect::<Vec<_>>(), vec![40, 30, 20, 10]);
        let mut reversed = text("TEST", 40.0);
        apply(&mut reversed, (0, 4), Options { reverse: true, ..opts });
        assert_eq!(chars(&reversed).iter().map(|(_, c)| c.size_pt.round() as i32).collect::<Vec<_>>(), vec![10, 20, 30, 40]);
        assert_eq!(reversed.text, "TEST");
    }

    #[test]
    fn random_is_deterministic_unicode_safe_and_local_to_range() {
        let opts = options(&json!({"mode":"random","minPercent":65,"maxPercent":95,"minSlant":-20,"maxSlant":20,"seed":99})).unwrap();
        let mut a = text("Aй😺B", 24.0);
        let mut b = a.clone();
        apply(&mut a, (1, 7), opts); // byte offsets covering й + 😺, not A or B
        apply(&mut b, (1, 7), opts);
        assert_eq!(a.runs, b.runs);
        assert_eq!(a.runs.iter().map(|r| r.len).sum::<usize>(), a.text.len());
        let cs = chars(&a);
        assert_eq!(cs.len(), 4);
        assert_eq!(cs[0].1.size_pt, 24.0);
        assert_eq!(cs[3].1.size_pt, 24.0);
        assert!(cs[1].1.size_pt < 24.0 && cs[2].1.size_pt < 24.0);
        assert!((-20.0..=20.0).contains(&cs[1].1.slant_deg));
        // PhotoCraft native save/load retains the precise custom angle.
        let json = serde_json::to_string(&a.runs).unwrap();
        let decoded: Vec<TextRun> = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, a.runs);
    }

    #[test]
    fn bad_parameter_ranges_rejected() {
        assert!(options(&json!({"minPercent":130,"maxPercent":80})).is_err());
        assert!(options(&json!({"minSlant":-60})).is_err());
        assert!(options(&json!({"mode":"unknown"})).is_err());
        assert!(options(&json!({"endSize":0})).is_err());
        assert!(options(&json!({"seed":-1})).is_err());
    }

    #[test]
    fn command_updates_one_type_layer_without_rasterizing() {
        use photocraft_doc::{LayerContent, LayerId};
        let mut s = Session::new();
        s.execute("file.new", json!({"width":250,"height":140,"background":"transparent"})).unwrap();
        let id = s.execute("type.create", json!({"text":"HELLO","size":36,"x":10,"y":70})).unwrap()["layer"].as_u64().unwrap();
        s.execute("type.textEffect", json!({"layer":id,"mode":"taper","endSize":12})).unwrap();
        let layer = s.active().unwrap().doc.layer(LayerId(id)).unwrap();
        match &layer.content {
            LayerContent::Text(t) => {
                assert_eq!(t.text, "HELLO");
                assert_eq!(chars(t).last().unwrap().1.size_pt, 12.0);
                assert!(t.psd_raw.is_some());
            }
            _ => panic!("effect rasterized the type layer"),
        }
    }
}
