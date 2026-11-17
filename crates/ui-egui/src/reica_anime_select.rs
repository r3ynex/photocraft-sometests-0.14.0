//! Object Selection / Anime Characters (SkyTNT): local, opt-in ONNX segmentation.
//! The model runs in a private Python child; all pixels stay on the user's machine.
//! No image is sent to an external endpoint. Existing PhotoCraft selections remain native.
use crate::PhotocraftApp;
use serde_json::{json, Value};

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
    use std::sync::{Arc, Mutex, OnceLock};

    use photocraft_doc::Document;

    const SCRIPT: &str = include_str!("../../../tools/reica_anime_seg/worker.py");
    const MAX_PIXELS: u64 = 12_000_000;

    struct Process {
        python: String,
        child: Child,
        input: ChildStdin,
        output: BufReader<ChildStdout>,
    }

    impl Drop for Process {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    static PROCESS: OnceLock<Mutex<Option<Process>>> = OnceLock::new();

    fn infer(python: &str, width: u32, height: u32, threshold: u32, pixels: &[u8]) -> Result<String, String> {
        let lock = PROCESS.get_or_init(|| Mutex::new(None));
        let mut process = lock.lock().map_err(|_| "Anime ONNX process lock poisoned".to_owned())?;
        if process.as_ref().is_none_or(|p| p.python != python) {
            let mut child = Command::new(python)
                .arg("-u").arg("-c").arg(SCRIPT)
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit())
                .env("PYTHONIOENCODING", "utf-8")
                .spawn().map_err(|e| format!("Cannot launch Python ({python}): {e}"))?;
            let input = child.stdin.take().ok_or("No child stdin")?;
            let output = child.stdout.take().ok_or("No child stdout")?;
            *process = Some(Process { python: python.into(), child, input, output: BufReader::new(output) });
        }
        let result = (|| {
            let proc = process.as_mut().ok_or("Anime worker unavailable")?;
            proc.input.write_all(&width.to_le_bytes()).map_err(|e| e.to_string())?;
            proc.input.write_all(&height.to_le_bytes()).map_err(|e| e.to_string())?;
            proc.input.write_all(&threshold.to_le_bytes()).map_err(|e| e.to_string())?;
            proc.input.write_all(pixels).map_err(|e| e.to_string())?;
            proc.input.flush().map_err(|e| e.to_string())?;
            let mut line = String::new();
            if proc.output.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                return Err("Anime worker exited. Install Python packages: python -m pip install numpy pillow onnxruntime".into());
            }
            let msg: Value = serde_json::from_str(&line).map_err(|e| format!("Bad SkyTNT response: {e}"))?;
            if let Some(error) = msg.get("error").and_then(Value::as_str) { return Err(error.to_owned()); }
            if msg.get("width").and_then(Value::as_u64) != Some(width as u64)
                || msg.get("height").and_then(Value::as_u64) != Some(height as u64) {
                return Err("Anime worker returned a mask with incorrect dimensions".into());
            }
            msg.get("mask").and_then(Value::as_str).map(str::to_owned)
                .ok_or_else(|| "Anime worker returned no mask".into())
        })();
        if result.is_err() { *process = None; }
        result
    }

    enum Outcome {
        Install(Result<String, String>),
        Mask {
            source: Arc<Document>,
            result: Result<String, String>,
        },
    }

    #[derive(Default)]
    struct JobState {
        running: bool,
        finished: Option<Outcome>,
    }
    static JOB: OnceLock<Mutex<JobState>> = OnceLock::new();
    fn state() -> &'static Mutex<JobState> { JOB.get_or_init(|| Mutex::new(JobState::default())) }

    pub fn busy() -> bool {
        state().lock().is_ok_and(|s| s.running)
    }

    fn start_job(task: impl FnOnce() -> Outcome + Send + 'static, ctx: &egui::Context) -> Result<(), String> {
        let mut guard = state().lock().map_err(|_| "Anime selection state locked".to_owned())?;
        if guard.running { return Err("Anime segmentation or model download is already running".into()); }
        guard.running = true;
        guard.finished = None;
        drop(guard);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task))
                .unwrap_or_else(|_| Outcome::Install(Err("Anime segmentation process panicked".into())));
            if let Ok(mut state) = state().lock() {
                state.running = false;
                state.finished = Some(outcome);
            }
            ctx.request_repaint();
        });
        Ok(())
    }

    pub fn install(app: &mut PhotocraftApp, ui: &egui::Ui) {
        let python = app.ui.tool_options.anime_python.trim().to_owned();
        let r = start_job(move || {
            let result = Command::new(&python).arg("-u").arg("-c").arg(SCRIPT).arg("--install")
                .output().map_err(|e| format!("Cannot launch Python ({python}): {e}"))
                .and_then(|output| {
                    let data: Value = serde_json::from_slice(&output.stdout)
                        .map_err(|e| format!("Invalid installer response: {e}"))?;
                    if let Some(e) = data.get("error").and_then(Value::as_str) { return Err(e.to_owned()); }
                    if !output.status.success() { return Err("Model download failed".into()); }
                    data.get("path").and_then(Value::as_str).map(str::to_owned)
                        .ok_or_else(|| "Model installation returned no path".into())
                });
            Outcome::Install(result)
        }, ui.ctx());
        app.ui.status = match r {
            Ok(()) => "Downloading pinned SkyTNT model (~176 MB) and verifying SHA256…".into(),
            Err(e) => e,
        };
    }

    pub fn select(app: &mut PhotocraftApp, ui: &egui::Ui) {
        let Some(d) = app.session.active() else {
            app.ui.status = "Open an image to select anime characters".into();
            return;
        };
        let bounds = d.doc.bounds();
        let count = u64::from(bounds.width()) * u64::from(bounds.height());
        if count == 0 || count > MAX_PIXELS {
            app.ui.status = "Anime selection supports up to 12 megapixels per canvas. Use a cropped copy.".into();
            return;
        }
        let source = Arc::clone(&d.doc);
        let python = app.ui.tool_options.anime_python.trim().to_owned();
        let threshold = (app.ui.tool_options.anime_threshold.clamp(0.0, 1.0) * 100.0).round() as u32;
        let r = start_job(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let image = photocraft_compose::render(&source, bounds)
                    .over_background([1.0, 1.0, 1.0]).to_rgba8();
                infer(&python, bounds.width(), bounds.height(), threshold, &image.pixels)
            })).unwrap_or_else(|_| Err("Could not render the canvas for Anime Selection".into()));
            Outcome::Mask { source, result }
        }, ui.ctx());
        app.ui.status = match r {
            Ok(()) => "Finding anime characters locally with SkyTNT…".into(),
            Err(e) => e,
        };
    }

    pub fn poll(app: &mut PhotocraftApp) {
        let outcome = match state().lock() {
            Ok(mut job) => job.finished.take(),
            Err(_) => None,
        };
        let Some(outcome) = outcome else { return; };
        match outcome {
            Outcome::Install(Ok(path)) => {
                app.ui.status_error = false;
                app.ui.status = format!("SkyTNT model installed and SHA256-verified at {path}");
            }
            Outcome::Install(Err(e)) => {
                app.ui.status_error = true;
                app.ui.status = e;
            }
            Outcome::Mask { source, result } => {
                let valid = app.session.active().is_some_and(|d| {
                    d.doc.id == source.id &&
                    photocraft_engine::layer_multi_cmds::same_pixels(&source, &d.doc)
                });
                if !valid {
                    app.ui.status_error = true;
                    app.ui.status = "Canvas changed during Anime Selection; result discarded".into();
                    return;
                }
                match result {
                    Ok(mask) => {
                        let bounds = source.bounds();
                        match app.run("reica.animeSelection.apply", json!({
                            "docId": source.id.0,
                            "width": bounds.width(),
                            "height": bounds.height(),
                            "mask": mask,
                            "mode": "replace",
                        })) {
                            Ok(result) => {
                                app.ui.status_error = false;
                                app.ui.status = if result["changed"] == true {
                                    "Anime characters selected! Use Select and Mask to refine hair edges.".into()
                                } else {
                                    "SkyTNT did not find a character. The previous selection was preserved.".into()
                                };
                            }
                            Err(e) => { app.ui.status_error = true; app.ui.status = e; }
                        }
                    }
                    Err(e) => { app.ui.status_error = true; app.ui.status = e; }
                }
            }
        }
    }
}

pub fn controls(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        native::poll(app);
        let busy = native::busy();
        ui.add(egui::TextEdit::singleline(&mut app.ui.tool_options.anime_python)
            .desired_width(95.0).hint_text("Python")).on_hover_text("Python with numpy, pillow, onnxruntime installed");
        if ui.add_enabled(!busy, egui::Button::new("Install SkyTNT")).on_hover_text("Download pinned 176 MB ONNX model only on request").clicked() {
            native::install(app, ui);
        }
        ui.add(egui::Slider::new(&mut app.ui.tool_options.anime_threshold, 0.1..=0.9)
            .text("Confidence")).on_hover_text("Softer/harder character boundary");
        if ui.add_enabled(!busy && app.session.active().is_some(),
            egui::Button::new(if busy { "Finding…" } else { "Select Anime Characters" })).clicked() {
            native::select(app, ui);
        }
    }
    #[cfg(target_arch = "wasm32")]
    ui.label("Local SkyTNT ONNX is only available in the desktop app");
}
