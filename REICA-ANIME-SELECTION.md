# RE:ICA PhotoCraft — SkyTNT Anime Characters Selection (beta)

This is **one combined soft selection covering all detected anime characters**, not one mask per character. It is integrated into PhotoCraft's existing **Object Selection** toolbar.

## Quick start

1. Install Python 3.10+ (or use the Python virtual environment you already configured for Manga OCR).
2. Install local inference dependencies in that Python environment:

   ```powershell
   python -m pip install numpy pillow onnxruntime
   ```

   The optional GPU runtime package is not required. The beta uses ONNX Runtime's **CPUExecutionProvider** for widest compatibility.
3. Open a canvas in RE:ICA PhotoCraft and choose **Object Selection**. The options bar now includes a Python path field, **Install SkyTNT**, a confidence slider, and **Select Anime Characters**.
4. Click **Install SkyTNT** once. This explicitly downloads the 176 MB pinned `isnetis.onnx` model, verifies the SHA256, and stores it under `models/skytnt/isnetis.onnx` next to `photocraft-reica.exe`.
5. Click **Select Anime Characters**. The visible image composite is analyzed locally and its soft foreground mask becomes an ordinary PhotoCraft selection.
6. Use the regular selection actions, **Select and Mask**, refine brushes, layer masks or copy-to-new-layer. Nothing is automatically cut out or deleted.

If Python isn't on PATH, set its full executable path in the Object Selection options bar. To use a predownloaded model, copy the checksum-verified `isnetis.onnx` into `models/skytnt` next to the executable.

## Model provenance and security

- Author/project: [SkyTNT/anime-segmentation](https://github.com/SkyTNT/anime-segmentation).
- Weights: [skytnt/anime-seg isnetis.onnx](https://huggingface.co/skytnt/anime-seg/blob/a0a563c41338cbe0d23dfb4bfc3e243c518e5768/isnetis.onnx).
- Published model-card license: Apache-2.0.
- Fixed revision: `a0a563c41338cbe0d23dfb4bfc3e243c518e5768`.
- Required SHA256: `f15622d853e8260172812b657053460e20806f04b9e05147d49af7bed31a6e99`.

The RE:ICA repository does **not** include the binary model. Both SkyTNT and Manga OCR keep their models beside the executable; see [REICA-MODELS.md](REICA-MODELS.md). The only network transfer is an explicit user-initiated download of the model itself; **no images or PSD contents are uploaded**. Inference happens in a local Python child process without an HTTP listener.

## Implementation notes

- The local worker `tools/reica_anime_seg/worker.py` reads the rendered visible canvas RGBA, resizes with aspect-preserving, centered square letterboxing to 1024×1024 and normalizes RGB values to [0,1], matching the upstream `get_mask` function in `inference.py`.
- It runs the pinned ONNX `img` -> `mask` graph, crops the padded mask, restores it to the original canvas size, applies the adjustable confidence to a soft alpha edge, and returns a compressed GRAY8 mask over private stdin/stdout.
- The Rust UI does this work on a background thread, caches the Python ONNX session for subsequent selections and refuses to apply a mask if the image pixels changed while inference was running.
- The engine command `reica.animeSelection.apply` validates the document and output dimensions, limits images to 12 megapixels, decompresses the mask with an output-size bound, then writes a **native undoable selection**. It supports replace/add/subtract/intersect modes (the first UI version uses replace).
- The original artwork is never modified by the selection command.

## Known limitations

- The model produces **one foreground mask**. Distinct characters cannot be clicked separately; that is intentional.
- The beta analyzes the **whole visible canvas**, not only the contents of a manually dragged selection rectangle. If the document exceeds **12 megapixels**, the tool explains that a cropped working copy is needed. This avoids heavy, potentially unsafe allocations on multi-page PSDs.
- It is trained mainly on colored anime illustrations. Black-and-white manga, textured backgrounds and overlapping shapes may need manual refinement.
- ONNX Runtime in the first beta defaults to CPU, so processing may take several seconds on slower laptops.
- The tool is desktop-only; web/wasm users see an explicit unavailable label.
- It does not automatically create a layer mask: use PhotoCraft's existing Select and Mask actions for that.

## Files

- `tools/reica_anime_seg/worker.py`: offline Python inference, SHA-checked opt-in model installer.
- `crates/ui-egui/src/reica_anime_select.rs`: asynchronous invocation and UI controls.
- `crates/ui-egui/src/tools/retouch_ui.rs`: Object Selection integration.
- `crates/ui-egui/src/state/tool_options.rs`: saved Python path and confidence options.
- `crates/engine/src/cmd/reica_anime_select_cmds.rs`: native selection command with bounds validation and tests.

Branch: `feature/reica-anime-seg-skytnt-20261117`. Keep existing stable MangaLab build until Windows UI check, tests and hands-on PSD verification succeed.
