"""Offline RE:ICA Manga Lab OCR worker. Protocol: width, height (LE u32) + RGBA pixels.
Runs as a persistent *child process* (not a web server); returns one JSON line per crop.
No remote uploads, network listener, or third-party API credentials.
The manga-ocr package downloads a model on first use unless already cached locally.
"""
import contextlib
import json
import struct
import sys

MAX_PIXELS = 9_000_000


def read_exact(length):
    buf = bytearray()
    while len(buf) < length:
        chunk = sys.stdin.buffer.read(length - len(buf))
        if not chunk:
            return None
        buf.extend(chunk)
    return bytes(buf)


def report(payload):
    sys.stdout.write(json.dumps(payload, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def main():
    # Keep Manga OCR startup status and library messages away from the JSON protocol.
    with contextlib.redirect_stdout(sys.stderr):
        from PIL import Image
        from manga_ocr import MangaOcr
        engine = MangaOcr()

    while True:
        head = read_exact(8)
        if head is None:
            return
        width, height = struct.unpack("<II", head)
        if not width or not height or width * height > MAX_PIXELS:
            report({"error": "Invalid or oversized OCR crop"})
            return
        pixels = read_exact(width * height * 4)
        if pixels is None:
            return
        try:
            image = Image.frombytes("RGBA", (width, height), pixels).convert("RGB")
            with contextlib.redirect_stdout(sys.stderr):
                text = engine(image)
            if not isinstance(text, str):
                raise ValueError("OCR returned non-text data")
            report({"text": text[:20000]})
        except Exception as exc:
            report({"error": f"OCR failed: {str(exc)[:300]}"})


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:
        report({"error": f"Could not start Manga OCR: {str(exc)[:400]}"})
        sys.exit(1)
