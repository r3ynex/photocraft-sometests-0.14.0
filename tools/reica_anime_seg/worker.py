"""RE:ICA / SkyTNT anime segmentation. An opt-in, local ONNX CPU worker.

Protocol (native parent only): request u32le(width), u32le(height), then RGBA8;
response one UTF-8 JSON line with width/height and zlib+base64-encoded GRAY8 alpha.
`--install` explicitly downloads the pinned ONNX weights (never an uploaded image).
No network requests occur in inference mode.
"""
import base64
import hashlib
import json
import os
from pathlib import Path
import struct
import sys
import urllib.request
import zlib

MODEL_REVISION = "a0a563c41338cbe0d23dfb4bfc3e243c518e5768"
MODEL_URL = "https://huggingface.co/skytnt/anime-seg/resolve/" + MODEL_REVISION + "/isnetis.onnx"
MODEL_SHA256 = "f15622d853e8260172812b657053460e20806f04b9e05147d49af7bed31a6e99"
MAX_MODEL_BYTES = 220 * 1024 * 1024
MAX_PIXELS = 12_000_000
MODEL_SIZE = 1024


def model_path():
    # PhotoCraft sets the absolute path relative to photocraft-reica.exe.
    explicit = os.environ.get("REICA_ANIME_SEG_MODEL")
    if explicit:
        return Path(explicit)
    models = os.environ.get("REICA_MODELS_DIR")
    if models:
        return Path(models) / "skytnt" / "isnetis.onnx"
    # Fallback only for launching this worker independently from PhotoCraft.
    return Path.home() / ".cache" / "reica" / "anime-seg" / "isnetis.onnx"


def emit(data):
    sys.stdout.write(json.dumps(data, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def read_exact(count):
    data = bytearray()
    while len(data) < count:
        chunk = sys.stdin.buffer.read(count - len(data))
        if not chunk:
            return None
        data.extend(chunk)
    return data


def checked_sha(path):
    h = hashlib.sha256()
    with path.open("rb") as src:
        for chunk in iter(lambda: src.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest() == MODEL_SHA256


def install():
    target = model_path()
    target.parent.mkdir(parents=True, exist_ok=True)
    if target.is_file() and checked_sha(target):
        return {"installed": True, "path": str(target), "already": True}

    temp = target.with_suffix(".partial")
    try:
        # Preserve existing downloads from the older home-cache location:
        # verify before copying, never trust an unverified local file.
        legacy = Path.home() / ".cache" / "reica" / "anime-seg" / "isnetis.onnx"
        if legacy != target and legacy.is_file() and checked_sha(legacy):
            import shutil
            shutil.copyfile(legacy, temp)
            temp.replace(target)
            return {"installed": True, "path": str(target), "already": False, "migrated": True}

        h = hashlib.sha256()
        transferred = 0
        with urllib.request.urlopen(MODEL_URL, timeout=60) as response, temp.open("wb") as out:
            while True:
                chunk = response.read(1 << 20)
                if not chunk:
                    break
                transferred += len(chunk)
                if transferred > MAX_MODEL_BYTES:
                    raise RuntimeError("Model exceeds expected 220 MiB limit")
                h.update(chunk)
                out.write(chunk)
        if h.hexdigest() != MODEL_SHA256:
            raise RuntimeError("Downloaded model checksum mismatch; refusing to load")
        temp.replace(target)
        return {"installed": True, "path": str(target), "already": False}
    finally:
        if temp.exists():
            temp.unlink()


def infer(session, rgba, width, height, threshold, np, Image):
    rgba_image = Image.frombytes("RGBA", (width, height), rgba)
    white = Image.new("RGBA", (width, height), (255, 255, 255, 255))
    image = Image.alpha_composite(white, rgba_image).convert("RGB")
    s = MODEL_SIZE
    if height > width:
        resized_h, resized_w = s, max(1, int(s * width / height))
    else:
        resized_w, resized_h = s, max(1, int(s * height / width))
    ph, pw = s - resized_h, s - resized_w
    top, left = ph // 2, pw // 2
    scaled = image.resize((resized_w, resized_h), Image.Resampling.BILINEAR)
    tensor = np.zeros((1, 3, s, s), dtype=np.float32)
    tensor[0, :, top:top + resized_h, left:left + resized_w] = (
        np.asarray(scaled, dtype=np.float32).transpose(2, 0, 1) / 255.0
    )
    prediction = np.asarray(session.run(["mask"], {"img": tensor})[0])
    if prediction.size != s * s:
        raise RuntimeError(f"Unexpected ONNX segmentation shape: {prediction.shape}")
    pred = prediction.reshape((s, s))[top:top + resized_h, left:left + resized_w]
    # The ONNX export already includes sigmoid. Restore soft edges instead of hard clipping hairs.
    pred = np.clip((pred.astype(np.float32) - float(threshold) + 0.14) / 0.28, 0.0, 1.0)
    small = Image.fromarray((pred * 255.0).astype(np.uint8), "L")
    full = small.resize((width, height), Image.Resampling.BILINEAR)
    alpha = full.tobytes()
    return {"width": width, "height": height,
            "mask": base64.b64encode(zlib.compress(alpha, level=6)).decode("ascii"),
            "nonzero": bool(any(alpha))}


def run():
    path = model_path()
    if not path.is_file():
        emit({"error": "SkyTNT model is not installed. Click Install Anime Model first."})
        return
    if not checked_sha(path):
        emit({"error": "ONNX model failed SHA256 verification; reinstall the pinned model."})
        return
    try:
        import numpy as np
        from PIL import Image
        import onnxruntime as ort
    except ImportError as exc:
        emit({"error": "Install Python packages: python -m pip install numpy pillow onnxruntime. " + str(exc)})
        return
    session = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    while True:
        header = read_exact(12)
        if header is None:
            return
        width, height, threshold_byte = struct.unpack("<III", header)
        count = width * height
        if (width <= 0 or height <= 0 or count > MAX_PIXELS or threshold_byte > 100):
            emit({"error": "Invalid canvas dimensions or threshold"})
            return
        rgba = read_exact(count * 4)
        if rgba is None:
            return
        try:
            emit(infer(session, rgba, width, height, threshold_byte / 100.0, np, Image))
        except Exception as exc:
            emit({"error": "Anime segmentation failed: " + str(exc)[:350]})


if __name__ == "__main__":
    try:
        if "--install" in sys.argv:
            emit(install())
        else:
            run()
    except Exception as exc:
        emit({"error": str(exc)[:500]})
        sys.exit(1)
