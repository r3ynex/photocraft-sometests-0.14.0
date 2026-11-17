//! RE:ICA MangaLab translation overlays: one Undo step per editable insertion or raster bake.
//! OCR runs outside the engine. No source pages or existing translations are ever flattened.
use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};
use photocraft_doc::{Document, Layer, LayerContent, LayerId};
use photocraft_raster::{Surface, from_rgba};
use serde_json::{Value, json};

const APPLY: &str = "reica.translate.apply";
const BAKE: &str = "reica.translate.bake";
const MAX_BAKE_PIXELS: u64 = 9_000_000;

fn invalid(what: &str) -> EngineError {
    EngineError::Other(format!("RE:ICA Translate: {what}"))
}
fn doc_ready(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "Open a document first".into())
}
fn root_page(doc: &Document, active: Option<LayerId>) -> Option<LayerId> {
    let path = doc.path_of(active?)?;
    let root = doc.layers.get(*path.first()?)?;
    root.name.starts_with("Page ").then_some(root.id)
}
fn translation_group(doc: &Document, page: Option<LayerId>) -> Option<LayerId> {
    let children: &[Layer] = page.and_then(|id| doc.layer(id)).and_then(Layer::children).unwrap_or(&doc.layers);
    children.iter().find(|layer| layer.name == "translation" && layer.is_group()).map(|layer| layer.id)
}
fn editable_children_mut(doc: &mut Document, translation: LayerId) -> Result<&mut Vec<Layer>> {
    let group = doc.layer_mut(translation).ok_or(EngineError::NoLayer(translation))?;
    let editable = group
        .children_mut()
        .and_then(|children| children.iter_mut().find(|l| l.name == "editable" && l.is_group()))
        .ok_or_else(|| invalid("editable group missing"))?;
    editable.children_mut().ok_or_else(|| invalid("editable group invalid"))
}
fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    let text = p.get("text").and_then(Value::as_str).unwrap_or("").trim();
    if text.is_empty() || text.len() > 20_000 {
        return Err(invalid("text must contain 1–20000 characters"));
    }
    let bounds = p.get("box").and_then(Value::as_array).ok_or_else(|| invalid("missing text region"))?;
    if bounds.len() != 4 || bounds.iter().any(|v| v.as_f64().is_none_or(|n| !n.is_finite())) {
        return Err(invalid("expected finite box [x,y,width,height]"));
    }
    let size = p.get("size").and_then(Value::as_f64).unwrap_or(18.0);
    if !(6.0..=200.0).contains(&size) {
        return Err(invalid("text size out of range"));
    }
    let mut t = crate::type_cmds::new_text_layer(s, p, APPLY)?;
    let page = {
        let state = s.active().ok_or(EngineError::NoDocument)?;
        root_page(&state.doc, state.active_layer)
    };
    let layer_id = s.edit("RE:ICA: Add Editable Translation", move |doc, active| {
        crate::type_cmds::refresh(doc, &mut t);
        let text_layer = Layer::new("Translated text", LayerContent::Text(t));
        let new_id = text_layer.id;
        if let Some(translation) = translation_group(doc, page) {
            // Undo remains one transaction, even when inserting into an existing group.
            editable_children_mut(doc, translation)?.push(text_layer);
            // Previous bake must not obscure newly edited content.
            if let Some(grp) = doc.layer_mut(translation) {
                if let Some(baked) = grp.children_mut().and_then(|ch| ch.iter_mut().find(|l| l.name == "baked")) {
                    baked.visible = false;
                }
                if let Some(editable) = grp.children_mut().and_then(|ch| ch.iter_mut().find(|l| l.name == "editable")) {
                    editable.visible = true;
                }
            }
        } else {
            let mut baked = Layer::group("baked", vec![Layer::raster("Translation Raster", doc.pixel_format())]);
            baked.visible = false;
            let editable = Layer::group("editable", vec![text_layer]);
            let translation = Layer::group("translation", vec![baked, editable]);
            if let Some(page_id) = page {
                doc.layer_mut(page_id).and_then(Layer::children_mut).ok_or_else(|| invalid("target page cannot contain groups"))?.push(translation);
            } else {
                doc.layers.push(translation);
            }
        }
        *active = Some(new_id);
        Ok(new_id)
    })?;
    crate::layer_multi_cmds::set_selection(s, vec![layer_id], Some(layer_id), Some(layer_id))?;
    Ok(json!({"layer":layer_id.0, "editable":true}))
}
fn bake(s: &mut Session, _p: &Value) -> Result<Value> {
    let (page, trans, target, mut isolated) = {
        let state = s.active().ok_or(EngineError::NoDocument)?;
        let page = root_page(&state.doc, state.active_layer);
        let trans = translation_group(&state.doc, page).ok_or_else(|| invalid("no translation group on this page"))?;
        let group = state.doc.layer(trans).ok_or(EngineError::NoLayer(trans))?;
        let mut editable =
            group.children().and_then(|ch| ch.iter().find(|l| l.name == "editable")).cloned().ok_or_else(|| invalid("no editable translation"))?;
        editable.visible = true;
        let mut scene = (*state.doc).clone();
        scene.layers.clear();
        scene.layers.push(editable);
        (page, trans, state.doc.bounds(), scene)
    };
    let pixels_count = u64::from(target.width()) * u64::from(target.height());
    if pixels_count == 0 || pixels_count > MAX_BAKE_PIXELS {
        return Err(invalid("bake limited to 9 million canvas pixels; use a smaller page"));
    }
    isolated.selection = None;
    let rendered = photocraft_compose::render(&isolated, target);
    let format = isolated.pixel_format();
    let mut flat = Vec::<f32>::new();
    for rgba in &rendered.px {
        flat.extend(from_rgba(&format, *rgba));
    }
    let mut surface = Surface::new(format);
    surface.write_region(target, &flat);
    let raster_id = s.edit("RE:ICA: Bake Translation Layer", move |doc, _active| {
        // Do not bake into an unrelated page after switching documents/layers.
        let found = translation_group(doc, page).ok_or_else(|| invalid("translation group changed"))?;
        if found != trans {
            return Err(invalid("translation group changed"));
        }
        let group = doc.layer_mut(trans).ok_or(EngineError::NoLayer(trans))?;
        let layers = group.children_mut().ok_or_else(|| invalid("translation group invalid"))?;
        let baked = layers.iter_mut().find(|l| l.name == "baked").ok_or_else(|| invalid("baked group missing"))?;
        let raster = baked.children_mut().ok_or_else(|| invalid("baked group invalid"))?;
        let layer = Layer::new("Translation Raster", LayerContent::Raster(surface));
        let id = layer.id;
        raster.clear();
        raster.push(layer);
        baked.visible = true;
        if let Some(editable) = layers.iter_mut().find(|l| l.name == "editable") {
            editable.visible = false;
        }
        Ok(id)
    })?;
    Ok(json!({"layer":raster_id.0, "baked":true, "editablePreserved":true}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: APPLY,
            label: "RE:ICA: Add Editable Translation",
            menu: &[],
            shortcut: None,
            params: r##"{"text":str,"box":[x,y,w,h],"font":str?,"size":pt?}"##,
            enabled: doc_ready,
            journal: true,
            run: apply,
        },
        CommandSpec {
            id: BAKE,
            label: "RE:ICA: Bake Translation Overlay",
            menu: &[],
            shortcut: None,
            params: "{}",
            enabled: doc_ready,
            journal: true,
            run: bake,
        },
    ]
}
