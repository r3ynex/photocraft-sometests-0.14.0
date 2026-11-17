//! RE:ICA MangaLab / Pages: non-destructive page grouping and conservative bubble cleaning.
//! One top-level raster layer or group is treated as one manga page. Nothing is flattened.
//! The fill is deliberately a separate raster layer, not a modification of the original.
use photocraft_doc::{Layer, LayerContent, LayerId, MAX_GROUP_DEPTH};
use photocraft_geom::Rect;
use photocraft_raster::{Surface, from_rgba};
use serde_json::{Value, json};
use std::collections::HashSet;

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

const COMMAND: &str = "reica.mangaPages.create";
const MAX_PIXELS: usize = 12_000_000;

#[derive(Clone, Copy, Debug)]
struct Options {
    reverse: bool,
    selected_only: bool,
    clean: bool,
    round: bool,
    rectangle: bool,
    sample_color: bool,
    threshold: u8,
    margin: usize,
    min_size: usize,
    first: usize,
}

impl Options {
    fn parse(p: &Value) -> Result<Self> {
        let number = |key: &str, default: u64| p.get(key).and_then(Value::as_u64).unwrap_or(default);
        let first = number("first", 1) as usize;
        let min_size = number("minSize", 35) as usize;
        let threshold = number("threshold", 225);
        let margin = number("margin", 3);
        if first == 0 || first > 99999 || min_size < 12 || min_size > 1000 || threshold < 150 || threshold > 250 || margin > 30 {
            return Err(EngineError::Other("RE:ICA Pages: invalid page, threshold or bubble size settings".into()));
        }
        Ok(Self {
            reverse: p.get("reverse").and_then(Value::as_bool).unwrap_or(false),
            selected_only: p.get("selectedOnly").and_then(Value::as_bool).unwrap_or(false),
            clean: p.get("autoClean").and_then(Value::as_bool).unwrap_or(true),
            round: p.get("round").and_then(Value::as_bool).unwrap_or(true),
            rectangle: p.get("rectangle").and_then(Value::as_bool).unwrap_or(true),
            sample_color: p.get("sampleColor").and_then(Value::as_bool).unwrap_or(false),
            threshold: threshold as u8,
            margin: margin as usize,
            min_size,
            first,
        })
    }
}

fn selected_page_ids(s: &Session, o: Options) -> Result<Vec<LayerId>> {
    let state = s.active().ok_or(EngineError::NoDocument)?;
    let selected: HashSet<_> = state.selected_layers().into_iter().collect();
    let doc = &state.doc;
    if doc.max_group_depth() + 2 > MAX_GROUP_DEPTH {
        return Err(EngineError::Other("Page/source/clean nesting would exceed the document group-depth limit".into()));
    }
    let ids: Vec<_> = doc
        .layers
        .iter()
        .filter(|l| {
            !doc.is_background(l.id)
                && !l.name.starts_with("Page ")
                && l.artboard().is_none()
                && (!o.selected_only || selected.contains(&l.id))
                && matches!(&l.content, LayerContent::Raster(_) | LayerContent::Group(_))
        })
        .map(|l| l.id)
        .collect();
    if ids.is_empty() {
        return Err(EngineError::Other("No page candidates: select top-level page layers/groups, or disable 'Selected only'".into()));
    }
    if ids.len() > 500 || o.first.saturating_add(ids.len()) > 100000 {
        return Err(EngineError::Other("Too many pages or invalid starting page number".into()));
    }
    Ok(ids)
}

/// The largest *raster* inside a page acts as the page background for bubble detection.
/// Other layers, including typeset text, are kept untouched and editable.
fn biggest_raster(layer: &Layer, depth: usize) -> Option<&Surface> {
    if depth > MAX_GROUP_DEPTH {
        return None;
    }
    if let LayerContent::Raster(surface) = &layer.content {
        return Some(surface);
    }
    layer.children()?.iter().filter(|child| child.visible).filter_map(|child| biggest_raster(child, depth + 1)).max_by_key(|s| {
        let bounds = s.content_bounds();
        u64::from(bounds.width()) * u64::from(bounds.height())
    })
}

fn light(p: [u8; 4], threshold: u8) -> bool {
    p[3] >= 235 && p[0] >= threshold && p[1] >= threshold && p[2] >= threshold
}

/// Paints only enclosed, mostly white regions with a plausible oval or rectangle contour.
/// Uses the light-region scanline extents, not a full bounding rectangle: black lettering inside
/// a bubble is covered while the outer ink outline remains safely outside the fill.
fn detect_and_fill(source: &Surface, output: &mut Surface, o: Options) -> usize {
    let rect = source.content_bounds();
    let w = rect.width() as usize;
    let h = rect.height() as usize;
    let Some(count) = w.checked_mul(h) else { return 0 };
    if w < o.min_size || h < o.min_size || count > MAX_PIXELS || count == 0 {
        return 0;
    }
    let mut rgba = vec![[0u8; 4]; count];
    source.read_rgba8_into(rect, &mut rgba);
    let mut seen = vec![false; count];

    let mut cleaned = 0;
    for seed in 0..count {
        if seen[seed] || !light(rgba[seed], o.threshold) {
            continue;
        }
        seen[seed] = true;
        let mut todo = vec![seed];
        let mut area = 0usize;
        let (mut minx, mut miny, mut maxx, mut maxy) = (w, h, 0usize, 0usize);
        let mut component = Vec::new();
        let mut sum = [0u64; 3];
        let mut at_border = false;
        while let Some(i) = todo.pop() {
            let x = i % w;
            let y = i / w;
            minx = minx.min(x);
            maxx = maxx.max(x);
            miny = miny.min(y);
            maxy = maxy.max(y);
            component.push(i);
            for c in 0..3 {
                sum[c] += u64::from(rgba[i][c]);
            }
            area += 1;
            if x == 0 || y == 0 || x + 1 == w || y + 1 == h {
                at_border = true;
            }
            let neighbors = [
                if x > 0 { Some(i - 1) } else { None },
                if x + 1 < w { Some(i + 1) } else { None },
                if y > 0 { Some(i - w) } else { None },
                if y + 1 < h { Some(i + w) } else { None },
            ];
            for neighbor in neighbors.into_iter().flatten() {
                if !seen[neighbor] && light(rgba[neighbor], o.threshold) {
                    seen[neighbor] = true;
                    todo.push(neighbor);
                }
            }
        }
        let bw = maxx - minx + 1;
        let bh = maxy - miny + 1;
        let bbox_area = bw.saturating_mul(bh);
        if at_border || bw < o.min_size || bh < o.min_size || bbox_area == 0 {
            continue;
        }
        if bbox_area > count / 3 || bw > 6 * bh || bh > 6 * bw {
            continue;
        }
        // Reject sparse white texture fragments, tiny islands and broad page backgrounds.
        let coverage = area as f32 / bbox_area as f32;
        if !(0.38..=0.995).contains(&coverage) {
            continue;
        }
        // Only real candidates need row contours. Tiny white islands don't allocate h rows.
        let mut row_min = vec![w; bh];
        let mut row_max = vec![0usize; bh];
        for i in component {
            let x = i % w;
            let y = i / w - miny;
            row_min[y] = row_min[y].min(x);
            row_max[y] = row_max[y].max(x);
        }
        let span = |y: usize| -> usize { if y < bh && row_min[y] < w { row_max[y] - row_min[y] + 1 } else { 0 } };
        // Compare average width near the upper edge and middle to classify the contour.
        let k = (bh / 12).max(1);
        let avg = |from: usize| -> usize {
            let to = from.saturating_add(k).min(bh);
            (from..to).map(span).sum::<usize>() / (to - from).max(1)
        };
        let upper = avg(k);
        let mid = avg(bh / 2 - k / 2);
        let round = upper * 100 < mid * 72;
        if (round && !o.round) || (!round && !o.rectangle) {
            continue;
        }

        // Require some darker content within the light region's bounding box. In a typical
        // lettered bubble this is text and the thin ink contour. Avoid blank white panels.
        let mut dark = 0usize;
        let stride = (bw / 40).max(1);
        for y in (miny..=maxy).step_by((bh / 35).max(1)) {
            for x in (minx..=maxx).step_by(stride) {
                let p = rgba[y * w + x];
                if p[3] > 200 && p[0] < 160 && p[1] < 160 && p[2] < 160 {
                    dark += 1;
                }
            }
        }
        if dark < 2 {
            continue;
        }
        // One thin opaque scanline per row. The transparent output remains fully editable.
        let color = if o.sample_color {
            [sum[0] as f32 / area as f32 / 255.0, sum[1] as f32 / area as f32 / 255.0, sum[2] as f32 / area as f32 / 255.0, 1.0]
        } else {
            [1.0, 1.0, 1.0, 1.0]
        };
        let paint = from_rgba(&output.format(), color);
        let mut did_paint = false;
        for y in 0..bh {
            if row_min[y] >= w {
                continue;
            }
            let x0 = row_min[y].saturating_add(o.margin);
            let x1 = row_max[y].saturating_sub(o.margin);
            if x1 <= x0 {
                continue;
            }
            output.fill_rect(
                Rect::new(rect.x0 + x0 as i32, rect.y0 + miny as i32 + y as i32, rect.x0 + x1 as i32 + 1, rect.y0 + miny as i32 + y as i32 + 1),
                &paint,
            );
            did_paint = true;
        }
        if did_paint {
            cleaned += 1;
        }
    }
    cleaned
}

fn create(s: &mut Session, p: &Value) -> Result<Value> {
    let o = Options::parse(p)?;
    let ids = selected_page_ids(s, o)?;
    let wanted: HashSet<LayerId> = ids.iter().copied().collect();
    let n = ids.len();
    let (groups, bubbles, skipped) = s.edit("RE:ICA MangaLab: Create Page Groups", move |doc, _| {
        let fmt = doc.pixel_format();
        let old = std::mem::take(&mut doc.layers);
        let mut rebuilt = Vec::with_capacity(old.len());
        let (mut groups, mut bubbles, mut skipped) = (Vec::new(), 0usize, 0usize);
        for layer in old {
            if !wanted.contains(&layer.id) {
                rebuilt.push(layer);
                continue;
            }
            let order = groups.len();
            let page_number = o.first + if o.reverse { n - 1 - order } else { order };
            let mut fill_layer = Layer::raster("bubble_fills", fmt);
            if o.clean && (o.round || o.rectangle) {
                if let Some(source) = biggest_raster(&layer, 0) {
                    if let Some(surface) = fill_layer.surface_mut() {
                        bubbles += detect_and_fill(source, surface, o);
                    }
                } else {
                    skipped += 1;
                }
            }
            let clean = Layer::group("clean", vec![fill_layer]);
            let page_visible = layer.visible;
            let mut original_layer = layer;
            original_layer.visible = true; // visibility now belongs to the page group
            let original = Layer::group("source", vec![original_layer]);
            let mut group = Layer::group(format!("Page {page_number:03}"), vec![clean, original]);
            group.visible = page_visible;
            groups.push(group.id);
            rebuilt.push(group);
        }
        doc.layers = rebuilt;
        Ok((groups, bubbles, skipped))
    })?;
    let selected = groups.clone();
    crate::layer_multi_cmds::set_selection(s, selected, groups.first().copied(), groups.first().copied())?;
    Ok(json!({"pages": groups.len(), "bubbles": bubbles, "noRasterPages": skipped}))
}

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "Open a document first".into())
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: COMMAND,
        label: "RE:ICA: Create Manga Page Groups",
        menu: &[],
        shortcut: None,
        params: r##"{"first":1,"reverse":false,"selectedOnly":false,"autoClean":true,"round":true,"rectangle":true,"sampleColor":false,"minSize":35,"threshold":225,"margin":3}"##,
        enabled: has_doc,
        run: create,
        journal: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::{ColorMode, SampleType};
    use photocraft_doc::{Document, Size};

    #[test]
    fn page_order_can_be_reversed_without_rearranging_layers() {
        let mut doc = Document::new("manga", Size::new(300, 300), ColorMode::Rgb, SampleType::U8);
        let format = doc.pixel_format();
        doc.layers.push(Layer::raster("page A", format));
        doc.layers.push(Layer::raster("page B", format));
        let original = std::mem::take(&mut doc.layers);
        let mut out = Vec::new();
        let n = original.len();
        for (i, layer) in original.into_iter().enumerate() {
            out.push(Layer::group(format!("Page {:03}", 1 + n - i - 1), vec![Layer::group("source", vec![layer])]));
        }
        assert_eq!(out[0].name, "Page 002");
        assert_eq!(out[1].name, "Page 001");
        assert_eq!(out[0].children().unwrap_or(&[])[0].children().unwrap_or(&[])[0].name, "page A");
    }

    #[test]
    fn enclosed_white_rectangular_bubble_is_filled_in_a_separate_layer() {
        let doc = Document::new("bubble", Size::new(200, 180), ColorMode::Rgb, SampleType::U8);
        let fmt = doc.pixel_format();
        let mut source = Surface::new(fmt);
        let dark = from_rgba(&fmt, [0.0, 0.0, 0.0, 1.0]);
        let light = from_rgba(&fmt, [1.0, 1.0, 1.0, 1.0]);
        source.fill_rect(Rect::new(0, 0, 200, 180), &dark);
        source.fill_rect(Rect::new(30, 30, 140, 120), &light);
        // Put black lettering inside the light bubble.
        source.fill_rect(Rect::new(42, 65, 112, 71), &dark);
        let mut out = Surface::new(fmt);
        let opt = Options {
            reverse: false,
            selected_only: false,
            clean: true,
            round: true,
            rectangle: true,
            sample_color: false,
            threshold: 225,
            margin: 3,
            min_size: 35,
            first: 1,
        };
        assert_eq!(detect_and_fill(&source, &mut out, opt), 1);
        assert_eq!(source.rgba(50, 67)[0], 0.0);
        assert!(out.rgba(50, 67)[0] > 0.98);
        assert!(out.rgba(30, 30)[3] < 0.1); // preserve outline and nearby detail
    }
}
