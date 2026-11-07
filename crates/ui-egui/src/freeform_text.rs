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

/// Screen-space controller dimensions are derived from the *visible* em height:
/// font size × document DPI × character scale × layer transform × canvas zoom.
/// Clamp them to keep tiny text selectable and huge text unobstructed.
#[derive(Clone, Copy, Debug)]
struct HandleMetrics {
    ring: f32,
    offset: f32,
    dot: f32,
    square: f32,
    ring_hit: f32,
}

fn handle_metrics(visible_em: f32) -> HandleMetrics {
    let em = if visible_em.is_finite() { visible_em.max(0.0) } else { 0.0 };
    let ring = (em * 0.43).clamp(10.0, 40.0);
    HandleMetrics {
        ring,
        offset: ring * 1.46,
        dot: (ring * 0.28).clamp(3.0, 7.0),
        square: (ring * 0.55).clamp(7.0, 18.0),
        ring_hit: (ring * 0.19).clamp(4.0, 7.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HandleKind {
    Move,
    Rotate,
    Scale,
}

/// Glyph indices selected with Shift-click, never persisted outside the editor.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub document: u64,
    pub layer: u64,
    pub indices: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq)]
struct DragTarget {
    index: usize,
    end_index: usize,
    /// Initial document-space anchor, including any preexisting freeform offset.
    center: [f64; 2],
    before: CharStyle,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Drag {
    document: u64,
    layer: u64,
    kind: HandleKind,
    start: [f64; 2],
    /// Shared rotation / scale pivot, average of the selected anchors.
    pivot: [f64; 2],
    transform: Affine,
    targets: Vec<DragTarget>,
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
    /// Document-space em-axis vector, after the layer transform.
    /// Screen scaling (including camera zoom, rotation and aspect) is applied
    /// at draw/hit-test time, so neither can disagree on handle positions.
    em_axis: [f64; 2],
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
        let em = (style.size_pt.abs() * layout.px_per_pt * style.freeform_scale.abs()).max(0.0);
        let (ex, ey) = layout.to_text(0.0, em);
        let origin = transform.apply(Point::new(0.0, 0.0));
        let extent = transform.apply(Point::new(f64::from(ex), f64::from(ey)));
        out.push(Handle {
            layer: id,
            index: photocraft_text::char_index(&text, cluster.range.start),
            end_index: photocraft_text::char_index(&text, cluster.range.end.min(text.len())),
            center: [p.x, p.y],
            transform,
            style,
            em_axis: [extent.x - origin.x, extent.y - origin.y],
        });
    }
    out
}

fn screen(xf: &ViewXform, p: [f64; 2]) -> Pos2 {
    xf.to_screen(p[0] as f32, p[1] as f32)
}

fn metrics_for(xf: &ViewXform, handle: &Handle) -> HandleMetrics {
    let axis = vec2(handle.em_axis[0] as f32, handle.em_axis[1] as f32);
    handle_metrics(xf.map_vec(axis).length())
}

fn size_pos(p: Pos2, metrics: HandleMetrics) -> Pos2 {
    p + vec2(metrics.offset, -metrics.offset)
}

pub fn draw_overlay(app: &mut PhotocraftApp, painter: &Painter, xf: &ViewXform) {
    let doc = app.session.active().map(|st| st.doc.id.0);
    let selected = app.ui.freeform_selection.clone().filter(|s| Some(s.document) == doc);
    let active = app.ui.freeform_drag.as_ref().map(|d| (d.layer, d.targets.iter().map(|s| s.index).collect::<Vec<_>>()));
    let current_handles = handles(app);
    for h in &current_handles {
        let p = screen(xf, h.center);
        let metrics = metrics_for(xf, h);
        let selected_here = selected.as_ref().is_some_and(|s| s.layer == h.layer.0 && s.indices.contains(&h.index))
            || active.as_ref().is_some_and(|(layer, ids)| *layer == h.layer.0 && ids.contains(&h.index));
        let color = if selected_here { Color32::from_rgb(255, 185, 71) } else { Color32::from_rgb(71, 175, 255) };
        painter.circle_stroke(p, metrics.ring, Stroke::new(if selected_here { 2.0 } else { 1.2 }, color));
        painter.circle_filled(p, metrics.dot, Color32::WHITE);
        painter.circle_stroke(p, metrics.dot, Stroke::new(1.0, color));
        let corner = size_pos(p, metrics);
        painter.line_segment([p + vec2(metrics.ring * 0.7, -metrics.ring * 0.7), corner], Stroke::new(1.0, color));
        let r = egui::Rect::from_center_size(corner, vec2(metrics.square, metrics.square));
        painter.rect_filled(r, 1.0, Color32::WHITE);
        painter.rect_stroke(r, 1.0, Stroke::new(1.0, color), egui::StrokeKind::Inside);
    }
    // A common centre makes group rotation and scaling predictable.
    if let Some(sel) = selected.filter(|s| s.indices.len() > 1) {
        let group: Vec<_> = current_handles.iter().filter(|h| h.layer.0 == sel.layer && sel.indices.contains(&h.index)).collect();
        if group.len() > 1 {
            let n = group.len() as f64;
            let centre = [group.iter().map(|h| h.center[0]).sum::<f64>() / n, group.iter().map(|h| h.center[1]).sum::<f64>() / n];
            let p = screen(xf, centre);
            let color = Color32::from_rgb(255, 185, 71);
            painter.circle_stroke(p, 5.0, Stroke::new(1.0, color));
            painter.line_segment([p + vec2(-7.0, 0.0), p + vec2(7.0, 0.0)], Stroke::new(1.0, color));
            painter.line_segment([p + vec2(0.0, -7.0), p + vec2(0.0, 7.0)], Stroke::new(1.0, color));
        }
    }
}

fn hit(app: &mut PhotocraftApp, xf: &ViewXform, pointer: Pos2) -> Option<(Handle, HandleKind)> {
    let mut best: Option<(f32, Handle, HandleKind)> = None;
    for h in handles(app) {
        let p = screen(xf, h.center);
        let metrics = metrics_for(xf, &h);
        let d = p.distance(pointer);
        let s = size_pos(p, metrics).distance(pointer);
        let candidate = if d <= metrics.dot + 2.5 {
            Some((d, HandleKind::Move))
        } else if s <= metrics.square * 0.5 + 3.5 {
            Some((s, HandleKind::Scale))
        } else if (d - metrics.ring).abs() <= metrics.ring_hit {
            Some(((d - metrics.ring).abs() + 1.0, HandleKind::Rotate))
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

fn apply(app: &mut PhotocraftApp, layer: u64, key: &str, changes: &[(usize, usize, CharStyle)]) {
    // One type.edit command, and one undo step for the entire group drag.
    let runs: Vec<_> = changes.iter().map(|(start, end, style)| json!({
        "start": start, "end": end.max(&(start + 1)),
        "freeformX": style.freeform_x, "freeformY": style.freeform_y,
        "freeformAngle": style.freeform_angle,
        "freeformScale": style.freeform_scale,
    })).collect();
    if let Err(e) = app.run("type.edit", json!({
        "layer": layer, "runs": runs, "coalesce": key,
    })) {
        app.ui.status = e;
        app.ui.status_error = true;
    }
}

fn angle(p: Pos2, pivot: Pos2) -> f64 {
    f64::from((p.y - pivot.y).atan2(p.x - pivot.x)).to_degrees()
}

fn local(aff: Affine, p: [f64; 2]) -> [f64; 2] {
    let local = aff.inverse().unwrap_or(Affine::IDENTITY).apply(Point::new(p[0], p[1]));
    [local.x, local.y]
}

fn delta_angle(a: [f64; 2], b: [f64; 2], pivot: [f64; 2]) -> f64 {
    let angle_of = |p: [f64; 2]| (p[1] - pivot[1]).atan2(p[0] - pivot[0]).to_degrees();
    let delta = angle_of(b) - angle_of(a);
    (delta + 180.0).rem_euclid(360.0) - 180.0
}

/// A pure group-transform function: move, rotate or scale about the selected
/// group's common pivot, leaving the original editable font size intact.
fn transformed(d: &Drag, at: [f64; 2], shift: bool) -> Vec<(usize, usize, CharStyle)> {
    let pivot = local(d.transform, d.pivot);
    let start = local(d.transform, d.start);
    let now = local(d.transform, at);
    let translate = [now[0] - start[0], now[1] - start[1]];
    let mut turn = delta_angle(start, now, pivot);
    if shift && matches!(d.kind, HandleKind::Rotate) {
        turn = (turn / 15.0).round() * 15.0;
    }
    let (sin, cos) = turn.to_radians().sin_cos();
    let old_radius = (start[0] - pivot[0]).hypot(start[1] - pivot[1]).max(0.001);
    let scale = ((now[0] - pivot[0]).hypot(now[1] - pivot[1]) / old_radius).clamp(0.1, 20.0);
    d.targets.iter().map(|target| {
        let mut style = target.before.clone();
        let current = local(d.transform, target.center);
        match d.kind {
            HandleKind::Move => {
                style.freeform_x += translate[0] as f32;
                style.freeform_y += translate[1] as f32;
            }
            HandleKind::Rotate => {
                let dx = current[0] - pivot[0];
                let dy = current[1] - pivot[1];
                let after = [pivot[0] + cos * dx - sin * dy, pivot[1] + sin * dx + cos * dy];
                style.freeform_x += (after[0] - current[0]) as f32;
                style.freeform_y += (after[1] - current[1]) as f32;
                style.freeform_angle += turn as f32;
            }
            HandleKind::Scale => {
                style.freeform_x += ((current[0] - pivot[0]) * (scale - 1.0)) as f32;
                style.freeform_y += ((current[1] - pivot[1]) * (scale - 1.0)) as f32;
                style.freeform_scale = (style.freeform_scale * scale as f32).clamp(0.1, 20.0);
            }
        }
        (target.index, target.end_index, style)
    }).collect()
}

pub fn pointer(app: &mut PhotocraftApp, event: ToolEvent, mods: Modifiers) -> bool {
    if !app.ui.freeform_text || !app.ui.tool.is_type() || app.ui.tool.is_type_mask() {
        app.ui.freeform_drag = None;
        return false;
    }
    if let Some(d) = app.ui.freeform_drag.clone() {
        let valid = app.session.active().is_some_and(|st| st.doc.id.0 == d.document && st.doc.layer(LayerId(d.layer)).is_some());
        if !valid {
            app.ui.freeform_drag = None;
            app.ui.freeform_selection = None;
            return false;
        }
        match event {
            ToolEvent::Up { .. } => {
                app.ui.freeform_drag = None;
                return true;
            }
            ToolEvent::Move { x, y, .. } => {
                let changes = transformed(&d, [x, y], mods.shift);
                apply(app, d.layer, &d.undo_key, &changes);
                return true;
            }
            ToolEvent::Down { .. } => {
                app.ui.freeform_drag = None;
            }
        }
    }

    let ToolEvent::Down { x, y, .. } = event else { return false };
    let Some(xf) = ViewXform::active(app) else { return false };
    let Some((h, kind)) = hit(app, &xf, screen(&xf, [x, y])) else {
        // Clicking a normal Type region still operates its caret as before.
        if !mods.shift { app.ui.freeform_selection = None; }
        return false;
    };
    let doc = match app.session.active() {
        Some(st) => st.doc.id.0,
        None => return false,
    };
    let mut selected = app.ui.freeform_selection.clone()
        .filter(|s| s.document == doc && s.layer == h.layer.0)
        .unwrap_or(Selection { document: doc, layer: h.layer.0, indices: Vec::new() });

    if mods.shift {
        // Shift-click adds/removes one arbitrary character; handles never turn
        // this press into a new Type layer.
        if let Some(i) = selected.indices.iter().position(|i| *i == h.index) {
            selected.indices.remove(i);
        } else {
            selected.indices.push(h.index);
        }
        app.ui.freeform_selection = Some(selected);
        return true;
    }
    if !selected.indices.contains(&h.index) {
        selected.indices.clear();
        selected.indices.push(h.index);
    }
    app.ui.freeform_selection = Some(selected.clone());
    let key = format!("freeform-{}", app.ui.alloc_id());
    if mods.alt {
        let mut style = h.style;
        style.freeform_x = 0.0;
        style.freeform_y = 0.0;
        style.freeform_angle = 0.0;
        style.freeform_scale = 1.0;
        apply(app, h.layer.0, &key, &[(h.index, h.end_index, style)]);
        return true;
    }
    let targets: Vec<DragTarget> = handles(app).into_iter().filter(|handle| {
        handle.layer == h.layer && selected.indices.contains(&handle.index)
    }).map(|handle| DragTarget {
        index: handle.index,
        end_index: handle.end_index,
        center: handle.center,
        before: handle.style,
    }).collect();
    if targets.is_empty() { return true; }
    let n = targets.len() as f64;
    let pivot = [
        targets.iter().map(|t| t.center[0]).sum::<f64>() / n,
        targets.iter().map(|t| t.center[1]).sum::<f64>() / n,
    ];
    app.ui.freeform_drag = Some(Drag {
        document: doc,
        layer: h.layer.0,
        kind,
        start: [x, y],
        pivot,
        transform: h.transform,
        targets,
        undo_key: key,
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(index: usize, x: f64) -> DragTarget {
        DragTarget {
            index, end_index: index + 1, center: [x, 0.0],
            before: CharStyle::default(),
        }
    }

    #[test]
    fn glyph_controller_tracks_visible_font_size_but_stays_usable() {
        let small = handle_metrics(12.0);
        let medium = handle_metrics(60.0);
        let large = handle_metrics(150.0);
        assert!(small.ring < medium.ring && medium.ring < large.ring);
        assert!(small.square < medium.square && medium.square < large.square);
        assert_eq!(handle_metrics(0.0).ring, 10.0);
        assert_eq!(handle_metrics(500.0).ring, 40.0);
        assert!(handle_metrics(f32::NAN).ring.is_finite());
    }

    #[test]
    fn ring_and_scale_hit_regions_follow_the_same_calculated_metrics() {
        for em in [8.0, 12.0, 30.0, 100.0, 300.0] {
            let m = handle_metrics(em);
            let centre = egui::pos2(100.0, 100.0);
            let size = size_pos(centre, m);
            assert!((size - centre).length() > m.ring);
            assert!(m.ring_hit >= 4.0);
            assert!(m.square >= 7.0);
        }
    }

    #[test]
    fn selected_glyphs_move_together_without_reflow() {
        let d = Drag {
            document: 1, layer: 2, kind: HandleKind::Move,
            start: [0.0, 0.0], pivot: [20.0, 0.0],
            transform: Affine::IDENTITY,
            targets: vec![target(0, 10.0), target(2, 30.0)],
            undo_key: String::new(),
        };
        let edited = transformed(&d, [6.0, -4.0], false);
        assert_eq!(edited.len(), 2);
        assert_eq!((edited[0].0, edited[1].0), (0, 2));
        assert_eq!((edited[0].2.freeform_x, edited[1].2.freeform_x), (6.0, 6.0));
        assert_eq!(edited[0].2.freeform_y, -4.0);
        assert_eq!(edited[1].2.size_pt, CharStyle::default().size_pt);
    }

    #[test]
    fn group_scaling_moves_centres_about_pivot() {
        let d = Drag {
            document: 1, layer: 2, kind: HandleKind::Scale,
            start: [30.0, 0.0], pivot: [20.0, 0.0],
            transform: Affine::IDENTITY,
            targets: vec![target(0, 10.0), target(2, 30.0)],
            undo_key: String::new(),
        };
        let edited = transformed(&d, [40.0, 0.0], false);
        assert_eq!(edited[0].2.freeform_x, -10.0);
        assert_eq!(edited[1].2.freeform_x, 10.0);
        assert_eq!(edited[0].2.freeform_scale, 2.0);
    }
}
