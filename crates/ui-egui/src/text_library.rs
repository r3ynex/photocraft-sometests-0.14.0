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
const GAP: f32 = 5.0;
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
    pub creating_group: bool,
    pub new_group_name: String,
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

const DEFAULT_FONT_GROUP: &str = "Default";

fn active_group(app: &PhotocraftApp) -> String {
    let prefs = app.session.prefs();
    let group = prefs.text_library_active_group.as_str();
    if group.is_empty() || group == DEFAULT_FONT_GROUP {
        return DEFAULT_FONT_GROUP.to_string();
    }
    if prefs.text_library_groups.iter().any(|g| g.name == group) {
        group.to_string()
    } else {
        DEFAULT_FONT_GROUP.to_string()
    }
}

fn font_groups(app: &PhotocraftApp) -> Vec<String> {
    std::iter::once(DEFAULT_FONT_GROUP.to_string())
        .chain(app.session.prefs().text_library_groups.iter().map(|g| g.name.clone()))
        .collect()
}

fn fonts_in_group(app: &PhotocraftApp, name: &str) -> Vec<String> {
    let p = app.session.prefs();
    if name == DEFAULT_FONT_GROUP {
        return p.text_library_fonts.clone();
    }
    p.text_library_groups.iter().find(|g| g.name == name)
        .map_or_else(Vec::new, |g| g.fonts.clone())
}

pub(crate) fn add_font_to_group(app: &mut PhotocraftApp, family: &str, name: &str) -> Result<(), String> {
    if !crate::type_tool::families().iter().any(|f| f == family) {
        return Err(format!("Font is not installed: {family}"));
    }
    if name != DEFAULT_FONT_GROUP && !app.session.prefs().text_library_groups.iter().any(|g| g.name == name) {
        return Err(format!("Font group does not exist: {name}"));
    }
    app.session.prefs.edit(|p| {
        let fonts = if name == DEFAULT_FONT_GROUP {
            &mut p.text_library_fonts
        } else {
            &mut p.text_library_groups.iter_mut().find(|g| g.name == name).expect("validated group").fonts
        };
        if !fonts.iter().any(|f| f == family) {
            fonts.push(family.to_string());
        }
    });
    Ok(())
}

fn add_font(app: &mut PhotocraftApp, family: &str) -> Result<(), String> {
    let group = active_group(app);
    add_font_to_group(app, family, &group)
}

fn delete_font(app: &mut PhotocraftApp, family: &str) -> Result<(), String> {
    let group = active_group(app);
    app.session.prefs.edit(|p| {
        if group == DEFAULT_FONT_GROUP {
            p.text_library_fonts.retain(|f| f != family);
        } else if let Some(g) = p.text_library_groups.iter_mut().find(|g| g.name == group) {
            g.fonts.retain(|f| f != family);
        }
    });
    Ok(())
}

fn create_group(app: &mut PhotocraftApp, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a group name".to_string());
    }
    if name.chars().count() > 64 {
        return Err("Group names may contain up to 64 characters".to_string());
    }
    if font_groups(app).iter().any(|g| g.eq_ignore_ascii_case(name)) {
        return Err(format!("A group named {name} already exists"));
    }
    app.session.prefs.edit(|p| {
        p.text_library_groups.push(photocraft_engine::prefs::TextLibraryFontGroup {
            name: name.to_string(), fonts: Vec::new(),
        });
        p.text_library_active_group = name.to_string();
    });
    Ok(())
}

/// Star beside the font picker in the Type options bar and Character/Properties panels.
/// Shift-click adds immediately to the active group; a normal click opens a group popover.
pub(crate) fn font_star(app: &mut PhotocraftApp, ui: &mut egui::Ui, family: &str) {
    let active = active_group(app);
    let in_active = fonts_in_group(app, &active).iter().any(|f| f == family);
    let label = if in_active { "★" } else { "☆" };
    let shift = ui.input(|i| i.modifiers.shift);
    let button = ui.add_sized([22.0, 22.0], egui::Button::new(label))
        .on_hover_text("Add font to a group (Shift-click: active group)");
    if shift && button.clicked() {
        let result = add_font_to_group(app, family, &active);
        message(app, result);
    }
    // Do not open a popup for Shift-click: that gesture is the quick-add shortcut.
    if !shift {
        egui::Popup::menu(&button).show(|ui| {
            ui.set_min_width(175.0);
            ui.label("Add font to group");
            ui.separator();
            for name in font_groups(app) {
                let present = fonts_in_group(app, &name).iter().any(|f| f == family);
                let label = if present { format!("✓ {name}") } else { name.clone() };
                if ui.button(label).clicked() {
                    let result = add_font_to_group(app, family, &name);
                    message(app, result);
                    ui.close();
                }
            }
        });
    }
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

fn paint_card(ui: &mut egui::Ui, tab: Tab, name: &str, style: Option<&StylePreset>, rect: Rect, picked: bool) -> egui::Response {
    let theme = crate::theme::Tokens::get(ui.ctx());
    let response = ui.interact(rect, ui.id().with(("text-library-card", tab, name)), Sense::click_and_drag());
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

    if state.tab == Tab::Fonts {
        let mut selected_group = active_group(app);
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("text-library-font-groups")
                .selected_text(selected_group.clone())
                .width((ui.available_width() - 95.0).clamp(100.0, 250.0))
                .show_ui(ui, |ui| {
                    for group in font_groups(app) {
                        ui.selectable_value(&mut selected_group, group.clone(), group);
                    }
                });
            if ui.small_button("+ Group").on_hover_text("Create a new font group").clicked() {
                state.creating_group = !state.creating_group;
            }
        });
        if selected_group != active_group(app) {
            app.session.prefs.edit(|p| p.text_library_active_group = selected_group);
            state.selected = None;
        }
        if state.creating_group {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut state.new_group_name)
                    .hint_text("Group name").desired_width(170.0));
                if ui.small_button("Create").clicked() {
                    let result = create_group(app, &state.new_group_name);
                    if result.is_ok() {
                        state.creating_group = false;
                        state.new_group_name.clear();
                        state.selected = None;
                    }
                    message(app, result);
                }
            });
        }
        ui.separator();
    }

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

    let fonts = fonts_in_group(app, &active_group(app));
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
            Tab::Fonts => "Click + to add a font to this group",
            Tab::Styles => "Select a text layer with effects and click +",
        });
    }
    // Only the scroll content consumes rows. The fixed header and trash stay in place.
    // The scrollbar is managed by ScrollArea; compute widths from its actual inner UI.
    let scroll_height = (ui.available_height() - 50.0).max(34.0);
    egui::ScrollArea::vertical()
        .id_salt(("text-library-cards", state.tab))
        .max_height(scroll_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let content_width = ui.available_width().max(1.0);
            let card_width = ((content_width - 2.0 * GAP) / 3.0).max(0.0);
            let row_count = names.len().div_ceil(3);
            let grid_height = if row_count == 0 {
                1.0
            } else {
                row_count as f32 * CARD_H + (row_count - 1) as f32 * GAP
            };

            // Reserve the ENTIRE grid height at once. Individual cards use
            // ui.interact(rect), which does not advance the layout cursor.
            // The single reservation prevents overlapping rows and keeps
            // ScrollArea's content height accurate in narrow docked panels.
            let (grid_rect, _) = ui.allocate_exact_size(
                vec2(content_width, grid_height),
                Sense::hover(),
            );
            for (index, name) in names.iter().enumerate() {
                let col = (index % 3) as f32;
                let row = (index / 3) as f32;
                let x = grid_rect.left() + col * (card_width + GAP);
                let y = grid_rect.top() + row * (CARD_H + GAP);
                let card_rect = Rect::from_min_size(pos2(x, y), vec2(card_width, CARD_H));
                let selected = state.selected.as_ref().is_some_and(|(t, n)| *t == state.tab && n == name);
                let style = if state.tab == Tab::Styles { styles.iter().find(|s| &s.name == name) } else { None };
                let response = paint_card(ui, state.tab, name, style, card_rect, selected);
                if response.clicked() {
                    state.selected = Some((state.tab, name.clone()));
                    let result = apply_card(app, state.tab, name, &styles);
                    message(app, result);
                }
                if response.drag_started() {
                    state.dragged = Some((state.tab, name.clone()));
                }
                response.context_menu(|ui| {
                    if ui.button("Remove from group").clicked() {
                        let result = remove_card(app, state.tab, name);
                        message(app, result);
                        ui.close();
                    }
                });
            }
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
            let w = (available - 2.0 * GAP) / 3.0;
            assert!(((3.0 * w + 2.0 * GAP) - available).abs() < 0.001);
            assert!(w > 0.0);
        }
    }
}
