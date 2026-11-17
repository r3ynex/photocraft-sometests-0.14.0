//! RE:ICA MangaLab local Japanese OCR. No localhost server, no cloud upload:
//! a private long-lived Python child process receives cropped RGBA through stdin.
use crate::PhotocraftApp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub python: String,
    pub original: String,
    pub translated: String,
    pub font_size: f32,
    pub box_xywh: [i32; 4],
    pub doc_id: u64,
    pub auto_bake: bool,
    pub status: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            python: "python".into(),
            original: String::new(),
            translated: String::new(),
            font_size: 18.0,
            box_xywh: [0; 4],
            doc_id: 0,
            auto_bake: false,
            status: "Select a speech bubble with Rectangular Marquee to recognize it.".into(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
    use std::sync::{Mutex, OnceLock};

    const MAX_PIXELS: u64 = 9_000_000;
    const PYTHON_HELPER: &str = include_str!("../../../tools/reica_manga_ocr/worker.py");

    struct Worker {
        command: String,
        child: Child,
        stdin: ChildStdin,
        stdout: BufReader<ChildStdout>,
    }
    impl Drop for Worker {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
    static WORKER: OnceLock<Mutex<Option<Worker>>> = OnceLock::new();

    fn new_worker(python: &str) -> Result<Worker, String> {
        let mut child = Command::new(python)
            .arg("-u")
            .arg("-c")
            .arg(PYTHON_HELPER)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .env("PYTHONIOENCODING", "utf-8")
            .spawn()
            .map_err(|e| format!("Cannot launch Python ({python}): {e}"))?;
        let stdin = child.stdin.take().ok_or("Python stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("Python stdout unavailable")?;
        Ok(Worker { command: python.into(), child, stdin, stdout: BufReader::new(stdout) })
    }

    fn recognize(python: &str, width: u32, height: u32, pixels: &[u8]) -> Result<String, String> {
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
            return Err("Crop exceeds 9 million pixels".into());
        }
        let lock = WORKER.get_or_init(|| Mutex::new(None));
        let mut worker = lock.lock().map_err(|_| "OCR worker lock poisoned".to_string())?;
        if worker.as_ref().is_none_or(|w| w.command != python) {
            *worker = Some(new_worker(python)?);
        }
        let result = (|| {
            let session = worker.as_mut().ok_or("OCR worker missing")?;
            session.stdin.write_all(&width.to_le_bytes()).map_err(|e| e.to_string())?;
            session.stdin.write_all(&height.to_le_bytes()).map_err(|e| e.to_string())?;
            session.stdin.write_all(pixels).map_err(|e| e.to_string())?;
            session.stdin.flush().map_err(|e| e.to_string())?;
            let mut line = String::new();
            if session.stdout.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                return Err("OCR process ended. Install: python -m pip install manga-ocr pillow".into());
            }
            let response: Value = serde_json::from_str(&line).map_err(|_| "Invalid response from local OCR process".to_string())?;
            if let Some(error) = response.get("error").and_then(Value::as_str) {
                return Err(error.to_owned());
            }
            response.get("text").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| "OCR did not return text".to_string())
        })();
        if result.is_err() {
            *worker = None;
        }
        result
    }

    struct OcrJob {
        running: bool,
        result: Option<(u64, [i32; 4], Result<String, String>)>,
    }
    impl Default for OcrJob {
        fn default() -> Self {
            Self { running: false, result: None }
        }
    }
    static JOB: OnceLock<Mutex<OcrJob>> = OnceLock::new();
    fn job() -> &'static Mutex<OcrJob> {
        JOB.get_or_init(|| Mutex::new(OcrJob::default()))
    }

    fn hide_translation(layers: &mut [photocraft_doc::Layer]) {
        for layer in layers {
            if layer.name == "translation" && layer.is_group() {
                layer.visible = false;
            } else if let Some(children) = layer.children_mut() {
                hide_translation(children);
            }
        }
    }
    pub(super) fn start(app: &PhotocraftApp, settings: &mut Settings, ui: &egui::Ui) {
        let state = match app.session.active() {
            Some(state) => state,
            None => {
                settings.status = "Open a PSD/document first".into();
                return;
            }
        };
        let rect = match &state.doc.selection {
            Some(mask) => mask.content_bounds().intersect(&state.doc.bounds()),
            None => {
                settings.status = "Select a bubble with Rectangular Marquee first".into();
                return;
            }
        };
        let size = u64::from(rect.width()) * u64::from(rect.height());
        if rect.is_empty() || size > MAX_PIXELS {
            settings.status = "Select a nonempty text region smaller than 9 MP".into();
            return;
        }
        let Ok(mut job_state) = job().lock() else { return };
        if job_state.running {
            settings.status = "Local OCR is already recognizing a region".into();
            return;
        }
        job_state.running = true;
        job_state.result = None;
        drop(job_state);
        let mut source = (*state.doc).clone();
        hide_translation(&mut source.layers);
        let python = settings.python.trim().to_string();
        let id = state.doc.id.0;
        let xywh = [rect.x0, rect.y0, rect.width() as i32, rect.height() as i32];
        let ctx = ui.ctx().clone();
        settings.status = "Recognizing offline… Initial model download can take time (~400 MB).".into();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let image = photocraft_compose::render(&source, rect).over_background([1.0, 1.0, 1.0]).to_rgba8();
                recognize(&python, rect.width(), rect.height(), &image.pixels)
            }))
            .unwrap_or_else(|_| Err("Local OCR crashed while processing the selection".into()));
            if let Ok(mut status) = job().lock() {
                status.running = false;
                status.result = Some((id, xywh, result));
            }
            ctx.request_repaint();
        });
    }

    pub(super) fn poll(settings: &mut Settings) -> bool {
        let Ok(mut status) = job().lock() else { return false };
        if status.running {
            return true;
        }
        if let Some((id, box_xywh, result)) = status.result.take() {
            match result {
                Ok(text) => {
                    settings.original = text;
                    settings.translated.clear(); // OCR is not a translation!
                    settings.box_xywh = box_xywh;
                    settings.doc_id = id;
                    settings.status = "OCR finished. Review Japanese, then enter/edit the translation.".into();
                }
                Err(error) => {
                    settings.status = error;
                }
            }
        }
        false
    }
}

pub fn panel(app: &mut PhotocraftApp, s: &mut Settings, ui: &mut egui::Ui) {
    #[cfg(not(target_arch = "wasm32"))]
    let busy = native::poll(s);
    #[cfg(target_arch = "wasm32")]
    let busy = false;

    egui::ScrollArea::vertical().id_salt("reica-translate").auto_shrink([false, false]).show(ui, |ui| {
        ui.heading("Manga OCR / Translate");
        ui.weak("Local Japanese recognition. Select one text balloon with Rectangular Marquee.");
        ui.weak("Images stay on this computer. OCR does not translate automatically.");
        ui.add_space(5.0);
        #[cfg(not(target_arch = "wasm32"))]
        {
            ui.horizontal(|ui| {
                ui.label("Python executable");
                ui.add(egui::TextEdit::singleline(&mut s.python).desired_width(130.0));
            });
            ui.small("One-time setup: python -m pip install manga-ocr pillow");
            if ui.add_enabled(!busy, egui::Button::new(if busy { "Recognizing…" } else { "Recognize Selection" })).clicked() {
                native::start(app, s, ui);
            }
        }
        #[cfg(target_arch = "wasm32")]
        ui.label("Local OCR requires the Windows/Linux/macOS desktop build.");
        ui.label(&s.status);
        ui.add_space(9.0);
        ui.label("Japanese OCR (editable for corrections)");
        ui.add(egui::TextEdit::multiline(&mut s.original).desired_rows(3));
        ui.label("Translated text (enter a translation to make a layer)");
        ui.add(egui::TextEdit::multiline(&mut s.translated).desired_rows(4));
        ui.horizontal(|ui| {
            ui.label("Size (pt)");
            ui.add(egui::DragValue::new(&mut s.font_size).range(6.0..=200.0).speed(0.5));
        });
        ui.checkbox(&mut s.auto_bake, "Also create baked raster copy on Apply");
        ui.weak("Editable and baked versions are separate and never shown simultaneously.");
        if ui.add_enabled(!s.translated.trim().is_empty() && s.doc_id != 0, egui::Button::new("Apply Editable Overlay")).clicked() {
            let same_document = app.session.active().is_some_and(|doc| doc.doc.id.0 == s.doc_id);
            if !same_document {
                s.status = "The document changed since OCR. Scan this document again.".into();
            } else {
                let [x, y, w, h] = s.box_xywh;
                let params = json!({
                    "text": s.translated,
                    "box": [x,y,w,h],
                    "size": s.font_size,
                    "name": "RE:ICA translated text",
                });
                match app.run("reica.translate.apply", params) {
                    Ok(_) => {
                        s.status = "Editable translation created in translation/editable".into();
                        if s.auto_bake {
                            match app.run("reica.translate.bake", json!({})) {
                                Ok(_) => s.status = "Editable and baked layers created. Baked version visible.".into(),
                                Err(e) => s.status = format!("Editable layer created; bake failed: {e}"),
                            }
                        }
                    }
                    Err(e) => {
                        s.status = format!("Could not create translation: {e}");
                    }
                }
            }
        }
        if ui.button("Bake / Re-bake Current Translation").clicked() {
            match app.run("reica.translate.bake", json!({})) {
                Ok(_) => s.status = "Translation baked. Editable text preserved, hidden.".into(),
                Err(e) => s.status = format!("Bake failed: {e}"),
            }
        }
        ui.separator();
        ui.weak("Coming later: automatic text-region detection, optional Google translation and multi-page batch processing.");
    });
}
