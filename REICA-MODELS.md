# RE:ICA — portable AI models

All AI **model weights and their local caches** are now stored next to the
PhotoCraft executable, in a single portable `models` directory. The location
is derived from the **running photocraft-reica.exe**, NOT the shell's working
directory, the Python interpreter location, or the current document path.

```text
REICA-MangaLab/
├── photocraft-reica.exe
├── README.txt
└── models/
    ├── skytnt/
    │   └── isnetis.onnx               # ~176 MB, download on demand
    ├── manga-ocr/
    │   ├── huggingface/
    │   │   └── hub/                   # Manga OCR base model (~400 MB)
    │   ├── torch/                     # Additional PyTorch caches if needed
    │   └── cache/                     # Other library caches (XDG)
    └── README.txt
```

**SkyTNT:** Object Selection → `Install SkyTNT` downloads the pinned
model only when clicked. It is SHA256-checked before it can be used.
Older, verified SkyTNT downloads in `~/.cache/reica/anime-seg/` are copied
into the portable folder automatically on installation if found.

**Manga OCR:** MangaLab → Translate → `Recognize Selection` launches
the Python OCR process with `HF_HOME`, `HF_HUB_CACHE`, `TORCH_HOME` and
`XDG_CACHE_HOME` pointing into `models/manga-ocr`. Hugging Face
Transformers downloads Manga OCR weights on first use, then runs offline.
Existing Hugging Face caches in the user profile are **not** automatically
migrated; the first use with this portable build may download them again.

**Python packages are NOT copied to models.** The installed Python
interpreter and packages (`manga-ocr`, `torch`, `onnxruntime`, `Pillow`,
`numpy`) are separate prerequisites for these optional tools.

No model weights are checked into Git or bundled in the release ZIP.

Move the **entire extracted folder** (EXE + models) together to keep both
model sets. To reclaim disk space, delete the relevant subdirectory under
`models`; PhotoCraft will recreate it when needed.

**Permissions:** avoid installing the portable build under
`C:\Program Files` or another protected/read-only directory. If PhotoCraft
cannot create the `models` subfolder, it reports an error instead of silently
writing to AppData. Use a writable location such as your Downloads folder or
another user-owned directory.

**Security/privacy:** AI recognition and segmentation run on your machine.
The model download is the only network transfer performed by these helpers;
no open editor image is uploaded. The model installer validates the SkyTNT
checksum before loading it.
