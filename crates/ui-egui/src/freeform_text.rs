//! Freeform Text: drag per-character controls without rasterising the Type layer.
//!
//! The character index shown here is a Unicode scalar index; text styles are
//! stored as UTF-8 byte runs. The existing type.edit command converts between
//! the two and moves run boundaries when the text changes.
use egui::{Color32, Modifiers, Painter, Pos2, Stroke, vec2};
use photocraft_doc::{LayerContent, LayerId};
use photocraft_doc::text::CharStyle;
use photocraft_geom::{Affine, Point};
use serde_json::json;

use crate::PhotocraftApp;
use crate::canvas::{ToolEvent, ViewXform};

const RING: f32 = 13.0;
const SIZE_HANDLE: f32 = 19.0;

#[derive(Clone, Copy, Debug)]
pub enum HandleKind {
    Move,
    Rotate,
    Scale,
}

#[derive(Clone, Debug)]
pub struct Drag {
    document: u64,
    layer: u64,
    index: usize,
    end_index: usize,
    kind: HandleKind,
    start: [f64; 2],
    pivot: [f64; 2],
    transform: Affine,
    before: CharStyle,
    undo_key: String,
}

#[derive(Clone)]
struct Handle {
    layer: LayerId,
    index: usize,
    end_index: usize,
    center: [f64; 2],
    transform: Affine,
    style: CharStyle,
}

fn selected_type(app: &PhotocraftApp) -> Option<LayerId> {
    if !app.ui.freeform_text || !app.ui.tool.is_type() || app.ui.tool.is_type_mask() {
        return None;
    }
    let st = app.session.active()?;
    let id = app.ui.text_edit.as_ref().map(|ed| LayerId(ed.layer)).or(st.active_layer)?;
    let layer = st.doc.layer(id)?;
    let locks = st.doc.effective_locks(id);
    if !layer.visible || locks.all || locks.position || !matches!(layer.content, LayerContent::Text(_)) {
        return None;
    }
    Some(id)
}

fn handles(app: &mut PhotocraftApp) -> Vec<Handle> {
    let Some(id) = selected_type(app) else { return Vec::new() };
    let Some((layout, transform, text)) = crate::type_tool::layout(app, id) else { return Vec::new() };
    let run_ends = {
        let Some(st) = app.session.active() else { return Vec::new() };
        let Some(layer) = st.doc.layer(id) else { return Vec::new() };
        let LayerContent::Text(t) = &layer.content else { return Vec::new() };
        let mut end = 0;
        t.char_runs().into_iter().map(|r| {
            end += r.len;
            end
        }).collect::<Vec<_>>()
    };
    let mut out = Vec::new();
    for cluster in &layout.clusters {
        if cluster.range.start >= text.len() || !text.is_char_boundary(cluster.range.start) {
            continue;
        }
        if matches!(text[cluster.range.start..].chars().next(), Some('\n' | '\r') | None) {
            continue;
        }
        let Some(line) = layout.lines.get(cluster.line) else { continue };
        let run = run_ends.partition_point(|&end| end <= cluster.range.start);
        let Some(style) = layout.styles.get(run).cloned() else { continue };
        // Cluster midpoints are stable caret-space anchors, including for UTF-8
        // and combined glyphs. The offset from Freeform Text is applied after.
        let (tx, ty) = layout.to_text(
            cluster.x + cluster.advance * 0.5,
            line.baseline - style.size_pt * layout.px_per_pt * 0.4,
        );
        let p = transform.apply(Point::new(
            f64::from(tx + style.freeform_x),
            f64::from(ty + style.freeform_y),
        ));
        out.push(Handle {
            layer: id,
            index: photocraft_text::char_index(&text, cluster.range.start),
            end_index: photocraft_text::char_index(&text, cluster.range.end.min(text.len())),
            center: [p.x, p.y],
            transform,
            style,
        });
    }
    out
}

fn screen(xf: &ViewXform, p: [f64; 2]) -> Pos2 {
    xf.to_screen(p[0] as f32, p[1] as f32)
}

fn size_pos(p: Pos2) -> Pos2 {
    p + vec2(SIZE_HANDLE, -SIZE_HANDLE)
}

pub fn draw_overlay(app: &mut PhotocraftApp, painter: &Painter, xf: &ViewXform) {
    let active = app.ui.freeform_drag.as_ref().map(|d| (d.layer, d.index));
    for h in handles(app) {
        let p = screen(xf, h.center);
        let selected = active == Some((h.layer.0, h.index));
        let color = if selected { Color32::from_rgb(255, 185, 71) } else { Color32::from_rgb(71, 175, 255) };
        // Circle for rotation, centre dot for translation and a square for size.
        painter.circle_stroke(p, RING, Stroke::new(if selected { 2.0 } else { 1.2 }, color));
        painter.circle_filled(p, 4.0, Color32::WHITE);
        painter.circle_stroke(p, 4.0, Stroke::new(1.0, color));
        let corner = size_pos(p);
        painter.line_segment([p + vec2(9.0, -9.0), corner], Stroke::new(1.0, color));
        let r = egui::Rect::from_center_size(corner, vec2(7.0, 7.0));
        painter.rect_filled(r, 1.0, Color32::WHITE);
        painter.rect_stroke(r, 1.0, Stroke::new(1.0, color), egui::StrokeKind::Inside);
    }
}

fn hit(app: &mut PhotocraftApp, xf: &ViewXform, pointer: Pos2) -> Option<(Handle, HandleKind)> {
    let mut best: Option<(f32, Handle, HandleKind)> = None;
    for h in handles(app) {
        let p = screen(xf, h.center);
        let d = p.distance(pointer);
        let s = size_pos(p).distance(pointer);
        let candidate = if d <= 5.5 {
            Some((d, HandleKind::Move))
        } else if s <= 6.0 {
            Some((s, HandleKind::Scale))
        } else if (d - RING).abs() <= 4.0 {
            Some(((d - RING).abs() + 1.0, HandleKind::Rotate))
        } else {
            None
        };
        if let Some((distance, kind)) = candidate
            && best.as_ref().is_none_or(|(prev, _, _)| distance < *prev)
        {
            best = Some((distance, h, kind));
        }
    }
    best.map(|(_, h, k)| (h, k))
}

fn apply(app: &mut PhotocraftApp, layer: u64, index: usize, end_index: usize, key: &str, style: &CharStyle) {
    let run = json!({
        "start": index, "end": end_index.max(index + 1),
        "freeformX": style.freeform_x, "freeformY": style.freeform_y,
        "freeformAngle": style.freeform_angle,
        "freeformScale": style.freeform_scale,
    });
    if let Err(e) = app.run("type.edit", json!({
        "layer": layer, "runs": [run], "coalesce": key,
    })) {
        app.ui.status = e;
        app.ui.status_error = true;
    }
}

fn angle(p: Pos2, pivot: Pos2) -> f64 {
    f64::from((p.y - pivot.y).atan2(p.x - pivot.x)).to_degrees()
}

pub fn pointer(app: &mut PhotocraftApp, event: ToolEvent, mods: Modifiers) -> bool {
    if !app.ui.freeform_text || !app.ui.tool.is_type() || app.ui.tool.is_type_mask() {
        app.ui.freeform_drag = None;
        return false;
    }
    if let Some(d) = app.ui.freeform_drag.clone() {
        if !app.session.active().is_some_and(|st| {
            st.doc.id.0 == d.document && st.doc.layer(LayerId(d.layer)).is_some()
        }) {
            app.ui.freeform_drag = None;
            return false;
        }
        match event {
            ToolEvent::Up { .. } => {
                app.ui.freeform_drag = None;
                return true;
            }
            ToolEvent::Move { x, y, .. } => {
                let Some(xf) = ViewXform::active(app) else { return true };
                let mut style = d.before.clone();
                match d.kind {
                    HandleKind::Move => {
                        let inverse = d.transform.inverse().unwrap_or(Affine::IDENTITY);
                        let from = inverse.apply(Point::new(d.start[0], d.start[1]));
                        let to = inverse.apply(Point::new(x, y));
                        style.freeform_x += (to.x - from.x) as f32;
                        style.freeform_y += (to.y - from.y) as f32;
                    }
                    HandleKind::Rotate => {
                        let pivot = screen(&xf, d.pivot);
                        let current = screen(&xf, [x, y]);
                        let original = screen(&xf, d.start);
                        let delta = angle(current, pivot) - angle(original, pivot);
                        // Shift snaps the changed angle to 15-degree steps.
                        let value = f64::from(d.before.freeform_angle) + delta;
                        style.freeform_angle = if mods.shift { (value / 15.0).round() as f32 * 15.0 } else { value as f32 };
                    }
                    HandleKind::Scale => {
                        let pivot = screen(&xf, d.pivot);
                        let current = screen(&xf, [x, y]).distance(pivot);
                        let original = screen(&xf, d.start).distance(pivot).max(1.0);
                        style.freeform_scale = (d.before.freeform_scale * current / original).clamp(0.1, 20.0);
                    }
                }
                apply(app, d.layer, d.index, d.end_index, &d.undo_key, &style);
                return true;
            }
            ToolEvent::Down { .. } => {
                app.ui.freeform_drag = None;
            }
        }
    }

    let ToolEvent::Down { x, y, .. } = event else { return false };
    let Some(xf) = ViewXform::active(app) else { return false };
    let Some((h, kind)) = hit(app, &xf, screen(&xf, [x, y])) else { return false };
    let doc = match app.session.active() {
        Some(st) => st.doc.id.0,
        None => return false,
    };
    let key = format!("freeform-{}", app.ui.alloc_id());
    if mods.alt {
        // Alt-click resets exactly one glyph to its original character layout.
        let mut style = h.style;
        style.freeform_x = 0.0;
        style.freeform_y = 0.0;
        style.freeform_angle = 0.0;
        style.freeform_scale = 1.0;
        apply(app, h.layer.0, h.index, h.end_index, &key, &style);
        return true;
    }
    app.ui.freeform_drag = Some(Drag {
        document: doc,
        layer: h.layer.0,
        index: h.index,
        end_index: h.end_index,
        kind,
        start: [x, y],
        pivot: h.center,
        transform: h.transform,
        before: h.style,
        undo_key: key,
    });
    true
}
