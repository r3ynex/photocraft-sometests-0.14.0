# RE:ICA Manga Lab — Translate / Manga OCR (first integration)

This feature belongs to the **existing RE:ICA Manga Lab dock**. A new fifth tab, `Translate`, has been added alongside Fonts, Styles, Effects and Pages.

## Current capabilities

- **Offline Japanese OCR:** select a speech bubble using PhotoCraft's Rectangular Marquee, then press **Recognize Selection**.
- PhotoCraft renders the selected region *from the actual document composite*, excluding pre-existing `translation` groups so previously generated text is not recognized again. Pixels are sent **locally** over stdin to a persistent Python child process.
- The OCR text can be corrected in the panel. The separate **Translated text** field is intentionally blank: *OCR does not translate*. Enter or paste the corrected Russian (or another language) translation.
- **Apply Editable Overlay:** creates a real, editable PhotoCraft type layer inside `translation/editable`, using the selection's coordinates. Repeated use adds further type layers to the same group.
- **Bake / Re-bake:** renders only the editable translation layers to `translation/baked/Translation Raster`, hides the editable version and shows the baked version. Editable text is *not destroyed*. You can re-enable it for correction.
- **Also create baked raster copy on Apply:** optional convenience switch.

## Requirements and setup

The native application (Windows/macOS/Linux), Python and [Manga OCR](https://github.com/kha-white/manga-ocr) are required. It cannot run in a browser build.

Recommended isolated Python 3.11 environment. In a terminal with the chosen Python executable:

```sh
python -m pip install manga-ocr pillow
```

Put the correct Python interpreter path into **Python executable** in the panel (e.g. `python` or a full path to `python.exe` from a virtual environment). This setting does **not** execute a shell command: it is an executable path. **Model download is controlled by manga-ocr**, occurs on first use and is approximately 400 MB; subsequent recognition is offline once cached.

The bundled Python worker script is **our own integration code** (not a vendored manga-ocr model or source): `tools/reica_manga_ocr/worker.py`. The Rust GUI embeds its source at compile time with `include_str!`. No network listener, no API key and no local port are needed. The child is retained in memory for subsequent crops to avoid reloading the model every time.

## Resulting layer tree

```text
Page 001                    # if a Pages group was active
  translation
    baked
      Translation Raster    # raster from editable translation, hidden until baked
    editable
      Translated text       # real, editable text layer
  clean
    bubble_fills
  source
    original layer(s)
```

Outside of a Pages group, a top-level `translation` group is created instead. Groups are reusable: every additional recognized fragment adds one editable text layer. A new bake replaces the old baked raster but preserves all editable layers.

## Recognition protocol

PhotoCraft renders the selected crop at its original resolution (with a 9-million-pixel safety limit), composites it against white and sends to a **persistent Python child**:

- request: `u32 LE width`, `u32 LE height`, `width*height*4` bytes of RGBA
- response: one newline-terminated JSON object, e.g. `{"text":"こんにちは"}` or `{"error":"..."}`

Manga OCR's actual Python API is `from manga_ocr import MangaOcr; mocr = MangaOcr(); text = mocr(PIL_image)`. The worker does not pretend the OCR model detects bounding boxes; PhotoCraft already knows the selected crop rectangle. The process is started lazily, on user request, with a background Rust worker so it doesn't freeze the UI while the model loads.

## Model storage (portable)

The Manga OCR Hugging Face model/cache now lives under `models/manga-ocr/huggingface` next to `photocraft-reica.exe`, not in the user profile. Related Torch/XDG caches are redirected to the same `models/manga-ocr` folder. You can move the entire PhotoCraft directory to another computer without re-downloading the model (provided Python and its packages are installed there). See [REICA-MODELS.md](REICA-MODELS.md).

## Known limitations

- The first version requires a **manually selected region**. Automatic detection of every speech balloon/text region across the page is **not shipped yet**; an optional local text detector can be added later.
- No automatic machine translation (Google / other providers) is in the current build. The translated text is entered manually, independently of the OCR original.
- The output position and approximate font size can be adjusted, but auto-typesetting and bubble-aware line wrapping are future enhancements.
- A full raster bake is currently limited to documents of **9 megapixels**. Large manga PSDs need a page-level crop bake; this is planned for the next increment.
- The photo composite may contain noises, backgrounds or lettering that Manga OCR misreads, and the model can hallucinate plausible Japanese on textless images. Review each result.
- Don't overwrite the only copy of your PSD; test on a duplicate, especially after a source-code upgrade.

## Planned subsequent steps

1. Optional locally installed text-region detection (not directly vendor-copied from GPL code).
2. Google Cloud Translation (opt-in, user-managed API credentials) or another explicit provider. **Do not** upload images or text without user action.
3. Auto-typesetting, preview, text editing and page-scoped bake on very large PSDs.
4. Cache OCR, multi-region and multi-page batches; explicit retry/stop for long model loading.

## Source map

- `crates/ui-egui/src/text_library.rs`: Translate tab within MangaLab.
- `crates/ui-egui/src/reica_translate.rs`: UI, document crop, background worker and Python IPC.
- `tools/reica_manga_ocr/worker.py`: offline Manga OCR wrapper, no HTTP server.
- `crates/engine/src/cmd/reica_translate_cmds.rs`: editable overlay and non-destructive raster bake.
- `crates/engine/src/cmd.rs`: engine registration.

The feature branch is `feature/reica-mangalab-translate-20261116`. Keep previous known-good MangaLab branches intact until compilation, regression tests and hands-on PSD verification all pass.

## Windows build / troubleshooting

Download the **REICA-MangaLab-Windows-Portable** artifact from the successful *REICA MangaLab Windows Portable* run under GitHub Actions on this feature branch. The workflow's green status means the executable compiled; the repository's separate full CI may show a red cross for formatting or Clippy issues unrelated to packaging. Always select the run by **branch and SHA**, not the older unverified upstream-merge branch.

