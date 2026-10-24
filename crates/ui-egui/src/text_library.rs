//! Text Library for PhotoCraft v0.15.1.
//!
//! Fonts are independent global favourites (Preferences::text_library_fonts).
//! Styles use the engine's persistent StylePreset library, in the "Text Library" group.
//! A style is *effects only*, never font, color, point size, alignment, or text content.
//!
//! Integrated into PhotoCraft's native docking system, with bounded 3-column cards
//! and a scrollable, vertically resizable panel.

use std::collections::VecDeque;

use egui::{Color32, ColorImage, CornerRadius, Rect, Sense, TextureHandle, TextureOptions, Vec2, pos2, vec2};
use photocraft_color::{Color, ColorMode, SampleType};
use photocraft_doc::{Document, Layer, LayerContent, Size, TextLayer};
use photocraft_engine::presets::styles::StylePreset;
use photocraft_geom::Affine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::PhotocraftApp;

const GROUP: &str = "Text Library";
const PREVIEW_ID: &str = "photocraft-text-library-style-previews";
const CARD_H: f32 = 79.0;
const GAP: f32 = 6.0;
const PREVIEW_W: u32 = 144;
const PREVIEW_H: u32 = 68;
const PREVIEW_CACHE_SIZE: usize = 64;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tab {
    #[default]
    Fonts,
    Styles,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub open: bool,
    pub tab: Tab,
    pub adding_font: bool,
    pub font_to_add: String,
    pub dragged: Option<(Tab, String)>,
    pub selected: Option<(Tab, String)>,
}

#[derive(Clone, Default)]
struct StyleTextures(VecDeque<(String, TextureHandle)>);

pub fn handles(id: &str) -> bool {
    id == "window.panel.textLibrary"
}

pub fn checked(app: &PhotocraftApp, id: &str) -> Option<bool> {
    handles(id).then_some(app.ui.text_library.open)
}

pub fn menu(app: &mut PhotocraftApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    if !handles(id) {
        return None;
    }
    let shown = p.get("show").and_then(Value::as_bool).unwrap_or(!app.ui.text_library.open);
    app.ui.text_library.open = shown;
    Some(Ok(json!({"open": shown})))
}

fn message(app: &mut PhotocraftApp, result: Result<(), String>) {
    if let Err(e) = result {
        app.ui.status = e;
        app.ui.status_error = true;
    } else {
        app.ui.status_error = false;
    }
}

fn add_font(app: &mut PhotocraftApp, family: &str) -> Result<(), String> {
    if !crate::type_tool::families().iter().any(|f| f == family) {
        return Err(format!("Font is not installed: {family}"));
    }
    app.session.prefs.edit(|p| {
        if !p.text_library_fonts.iter().any(|f| f == family) {
            p.text_library_fonts.push(family.to_string());
        }
    });
    Ok(())
}

fn delete_font(app: &mut PhotocraftApp, family: &str) -> Result<(), String> {
    app.session.prefs.edit(|p| p.text_library_fonts.retain(|f| f != family));
    Ok(())
}

fn type_target(app: &PhotocraftApp) -> Result<u64, String> {
    // The type tool can retain an editing selection distinct from the active Layers row.
    if let Some(ed) = &app.ui.text_edit {
        return Ok(ed.layer);
    }
    let state = app.session.active().ok_or("Open a document first")?;
    let id = state.active_layer.ok_or("Select a text layer first")?;
    let layer = state.doc.layer(id).ok_or("Selected layer is missing")?;
    if !matches!(&layer.content, LayerContent::Text(_)) {
        return Err("Select a text layer first".into());
    }
    Ok(id.0)
}

fn apply_font(app: &mut PhotocraftApp, family: &str) -> Result<(), String> {
    let p = crate::type_tool::with_target(app, json!({"font": family})).ok_or("Select a text layer first")?;
    app.run("type.setStyle", p)?;
    app.note_recent_font(family);
    Ok(())
}

fn library_styles(app: &PhotocraftApp) -> Vec<StylePreset> {
    app.session.presets.styles.iter().find(|g| g.name == GROUP).map_or_else(Vec::new, |g| g.items.clone())
}

fn save_style(app: &mut PhotocraftApp) -> Result<(), String> {
    let layer = type_target(app)?;
    let selected = app.session.active().and_then(|s| s.doc.layer(photocraft_doc::LayerId(layer))).ok_or("Select a text layer")?;
    if selected.effects.items.is_empty() {
        return Err("Selected text layer has no effects to save".into());
    }
    // Native preset persistence: this is saved to Preferences/presets and
    // survives app restarts, while the document's text formatting is never read.
    app.run("style.presets.new", json!({
        "name": "Text Style", "group": GROUP, "layer": layer,
        "includeEffects": true, "includeBlending": false
    }))?;
    Ok(())
}

fn apply_style(app: &mut PhotocraftApp, preset: &StylePreset) -> Result<(), String> {
    let id = type_target(app)?;
    // The regular `style.presets.apply` command resets Fill Opacity even when
    // the preset has no blending options. `replace` changes *only* layer effects.
    let effects: Vec<Value> = preset.effects.iter().map(|fx| {
        json!({
            "kind": photocraft_engine::layer_style::kind_of(fx),
            "fx": fx,
            "params": {}
        })
    }).collect();
    app.run("layer.layerStyle.replace", json!({"layer": id, "effects": effects}))?;
    Ok(())
}

fn delete_style(app: &mut PhotocraftApp, name: &str) -> Result<(), String> {
    app.run("style.presets.edit", json!({
        "action": "delete", "group": GROUP, "preset": name
    }))?;
    Ok(())
}

fn apply_card(app: &mut PhotocraftApp, tab: Tab, name: &str, styles: &[StylePreset]) -> Result<(), String> {
    match tab {
        Tab::Fonts => apply_font(app, name),
        Tab::Styles => {
            let style = styles.iter().find(|s| s.name == name).ok_or("Style is no longer available")?;
            apply_style(app, style)
        }
    }
}

fn remove_card(app: &mut PhotocraftApp, tab: Tab, name: &str) -> Result<(), String> {
    match tab {
        Tab::Fonts => delete_font(app, name),
        Tab::Styles => delete_style(app, name),
    }
}

// Actual text shaped and rendered by PhotoCraft's text engine, with the saved
// layer effects applied by PhotoCraft's compositor. No imitation CSS styles.
fn render_style_preview(preset: &StylePreset) -> ColorImage {
    let mut doc = Document::new(
        "Text Library Preview",
        Size::new(PREVIEW_W, PREVIEW_H),
        ColorMode::Rgb,
        SampleType::U8,
    );
    let mut text = TextLayer {
        text: "Aa".into(),
        font_family: photocraft_text::fonts::DEFAULT_FAMILY.to_string(),
        size_pt: 37.0,
        color: Color::rgb(0.94, 0.94, 0.94),
        transform: Affine::translate(37.0, 49.0),
        ..Default::default()
    };
    photocraft_engine::type_cmds::refresh(&doc, &mut text);
    let mut layer = Layer::new("Preview", LayerContent::Text(text));
    layer.effects.items = preset.effects.clone();
    layer.effects.enabled = true;
    doc.layers.push(layer);
    let pixels = photocraft_compose::render(&doc, doc.bounds()).to_rgba8().pixels;
    ColorImage::from_rgba_unmultiplied([PREVIEW_W as usize, PREVIEW_H as usize], &pixels)
}

fn style_texture(ctx: &egui::Context, style: &StylePreset) -> TextureHandle {
    let key = format!("{}:{}", style.name, serde_json::to_string(&style.effects).unwrap_or_default());
    let id = egui::Id::new(PREVIEW_ID);
    if let Some(texture) = ctx.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<StyleTextures>(id);
        let index = cache.0.iter().position(|(k, _)| k == &key)?;
        let item = cache.0.remove(index)?;
        let texture = item.1.clone();
        cache.0.push_back(item);
        Some(texture)
    }) {
        return texture;
    }
    let image = render_style_preview(style);
    let texture = ctx.load_texture(format!("text-lib-{}", style.name), image, TextureOptions::LINEAR);
    ctx.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<StyleTextures>(id);
        while cache.0.len() >= PREVIEW_CACHE_SIZE {
            cache.0.pop_front();
        }
        cache.0.push_back((key, texture.clone()));
    });
    texture
}

fn paint_card(ui: &mut egui::Ui, tab: Tab, name: &str, style: Option<&StylePreset>, width: f32, picked: bool) -> egui::Response {
    let theme = crate::theme::Tokens::get(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(width, CARD_H), Sense::click_and_drag());
    let fill = if picked { theme.row_selected } else if response.hovered() { theme.hover } else { theme.field };
    ui.painter().rect_filled(rect, CornerRadius::same(9), fill);
    ui.painter().rect_stroke(rect, CornerRadius::same(9), egui::Stroke::new(1.0, if picked { theme.accent } else { theme.card_border }), egui::StrokeKind::Inside);

    let preview = Rect::from_min_max(rect.min + vec2(3.0, 3.0), pos2(rect.right() - 3.0, rect.bottom() - 25.0));
    if ui.is_rect_visible(rect) {
        match (tab, style) {
            (Tab::Fonts, _) => {
                // Font samples are real rasterized installed fonts. The existing engine
                // caches their textures and handles fonts without Latin glyphs.
                // The font renderer uses a 100px-wide thumbnail at 18pt. Scale
                // down in narrow docks, and clip it within the card.
                let size = (preview.width() * 18.0 / 104.0).clamp(8.0, 15.0);
                ui.scope_builder(egui::UiBuilder::new().max_rect(preview), |ui| {
                    ui.set_clip_rect(preview.intersect(ui.clip_rect()));
                    crate::font_preview::paint(ui, name, preview, size);
                });
            }
            (Tab::Styles, Some(st)) => {
                let tex = style_texture(ui.ctx(), st);
                ui.painter().image(tex.id(), preview, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            _ => {}
        }
    }
    let galley = ui.painter().layout_no_wrap(name.to_string(), egui::FontId::proportional(10.0), theme.text_dim);
    let label_pos = pos2(rect.center().x - galley.size().x.min(rect.width() - 8.0) / 2.0, rect.bottom() - 20.0);
    // Clip long font family names rather than allowing them to overlap neighbours.
    let label_rect = Rect::from_min_max(pos2(rect.left() + 4.0, rect.bottom() - 22.0), pos2(rect.right() - 4.0, rect.bottom() - 3.0));
    ui.painter().with_clip_rect(label_rect).galley(label_pos, galley, theme.text_dim);
    response.on_hover_text(format!("{name} • click to apply, drag to remove"))
}

/// Contents used by the shared DockArea. The old independent floating window is gone.
/// DockArea owns the title bar and the resize handles, so there is only one header.
pub fn panel(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let mut state = std::mem::take(&mut app.ui.text_library);
    body(app, &mut state, ui);
    app.ui.text_library = state;
}

fn body(app: &mut PhotocraftApp, state: &mut State, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.tab, Tab::Fonts, "Fonts");
        ui.selectable_value(&mut state.tab, Tab::Styles, "Styles");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("+").on_hover_text(match state.tab { Tab::Fonts => "Add an installed font", Tab::Styles => "Save effects from selected text layer" }).clicked() {
                match state.tab {
                    Tab::Fonts => {
                        state.adding_font = !state.adding_font;
                        if state.font_to_add.is_empty() {
                            state.font_to_add = crate::type_tool::families().first().cloned().unwrap_or_default();
                        }
                    }
                    Tab::Styles => {
                        let result = save_style(app);
                        message(app, result);
                    },
                }
            }
        });
    });
    ui.separator();

    if state.tab == Tab::Fonts && state.adding_font {
        let families = crate::type_tool::families();
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 77.0).max(20.0);
            crate::type_tool::family_picker_in(
                ui, "text-library-font-add", &mut state.font_to_add, width,
                &families, app.session.prefs().type_.font_preview, &[],
            );
            if ui.button("Add").clicked() {
                let name = state.font_to_add.clone();
                let result = add_font(app, &name);
                if result.is_ok() {
                    state.adding_font = false;
                }
                message(app, result);
            }
        });
        ui.separator();
    }

    let fonts = app.session.prefs().text_library_fonts.clone();
    let styles = library_styles(app);
    let names: Vec<String> = match state.tab {
        Tab::Fonts => fonts,
        Tab::Styles => styles.iter().map(|s| s.name.clone()).collect(),
    };

    // No min-width growth: a fixed three-column grid must never dictate the
    // size of its parent DockArea / floating window. A 4th font adds a row,
    // not another column, and the font-picker popup can't widen the pane.
    if names.is_empty() {
        ui.label(match state.tab {
            Tab::Fonts => "Click + to add your first font",
            Tab::Styles => "Select a text layer with effects and click +",
        });
    }
    let available = ui.available_width().max(1.0);
    // Leave a few points for scrollbar so it does not feed back into width.
    let w = ((available - 2.0 * GAP - 14.0) / 3.0).clamp(1.0, 132.0);
    // The docking container sets available height. Keep header and trash fixed,
    // and scroll only the grid; the splitter/float border resizes this area.
    let scroll_height = (ui.available_height() - 50.0).max(34.0);
    egui::ScrollArea::vertical()
        .id_salt(("text-library-cards", state.tab))
        .max_height(scroll_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_width(available);
            egui::Grid::new(("text-library-grid", state.tab))
                .num_columns(3)
                .spacing(vec2(GAP, GAP))
                .show(ui, |ui| {
                    for (i, name) in names.iter().enumerate() {
                        let selected = state.selected.as_ref().is_some_and(|(t, n)| *t == state.tab && n == name);
                        let style = if state.tab == Tab::Styles { styles.iter().find(|s| &s.name == name) } else { None };
                        let response = paint_card(ui, state.tab, name, style, w, selected);
                        if response.clicked() {
                            state.selected = Some((state.tab, name.clone()));
                            let result = apply_card(app, state.tab, name, &styles);
                            message(app, result);
                        }
                        if response.drag_started() {
                            state.dragged = Some((state.tab, name.clone()));
                        }
                        response.context_menu(|ui| {
                            if ui.button("Remove from library").clicked() {
                                let result = remove_card(app, state.tab, name);
                                message(app, result);
                                ui.close();
                            }
                        });
                        if i % 3 == 2 { ui.end_row(); }
                    }
                });
        });

    ui.add_space(5.0);
    let (trash, response) = ui.allocate_exact_size(Vec2::new(ui.available_width().max(1.0), 31.0), Sense::hover());
    let hovered = state.dragged.is_some() && response.hovered();
    let t = crate::theme::Tokens::get(ui.ctx());
    ui.painter().rect_filled(trash, CornerRadius::same(7), if hovered { t.hover } else { t.field });
    ui.painter().rect_stroke(trash, CornerRadius::same(7), egui::Stroke::new(1.0, if hovered { t.accent } else { t.card_border }), egui::StrokeKind::Inside);
    ui.painter().text(trash.center(), egui::Align2::CENTER_CENTER, "Drag here to remove", egui::FontId::proportional(11.0), t.text_dim);

    // On mouse release, remove only if the pointer is within the lower trash strip.
    // Dragging outside cancels, clicking a card never removes it.
    if ui.input(|i| i.pointer.any_released()) {
        if let Some((tab, name)) = state.dragged.take() {
            if ui.input(|i| i.pointer.latest_pos().is_some_and(|p| trash.contains(p))) {
                let result = remove_card(app, tab, &name);
                message(app, result);
                if state.selected.as_ref() == Some(&(tab, name)) {
                    state.selected = None;
                }
            }
        }
    } else if state.dragged.is_some() && !ui.input(|i| i.pointer.any_down()) {
        state.dragged = None;
    }
}

#[cfg(test)]
mod library_layout_tests {
    use super::*;
    #[test]
    fn grid_width_is_bounded_at_common_dock_sizes() {
        for available in [220.0, 260.0, 290.0, 350.0, 800.0] {
            let w = ((available - 2.0 * GAP - 14.0) / 3.0_f32).clamp(1.0, 132.0);
            assert!(3.0 * w + 2.0 * GAP <= available);
        }
    }
}
