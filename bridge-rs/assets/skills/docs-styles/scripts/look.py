#!/usr/bin/env python3
"""Render a .pptx or .docx to one PNG per page AND one contact sheet (all slides on one image) to look at with
vision_analyze. Needs LibreOffice (soffice) + pdftoppm.   look.py deck.pptx --outdir render"""
import argparse
import glob
import json
import os
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import _deps  # noqa: E402

_deps.need("PIL")
from PIL import Image, ImageDraw  # noqa: E402


def sheet(files, out, cols=3, width=480):
    ims = [Image.open(f).convert("RGB") for f in files]
    h = int(width * ims[0].height / ims[0].width)
    rows = (len(ims) + cols - 1) // cols
    pad = 16
    canvas = Image.new("RGB", (cols * (width + pad) + pad, rows * (h + pad + 22) + pad), "white")
    d = ImageDraw.Draw(canvas)
    for i, im in enumerate(ims):
        x, y = pad + (i % cols) * (width + pad), pad + (i // cols) * (h + pad + 22)
        canvas.paste(im.resize((width, h)), (x, y + 22))
        d.text((x, y + 4), f"page {i + 1}", fill="black")
    canvas.save(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pptx")
    ap.add_argument("--outdir", default="render")
    a = ap.parse_args()
    soffice = shutil.which("soffice") or shutil.which("libreoffice")
    split = shutil.which("pdftoppm")
    if not soffice or not split:
        print(json.dumps({"rendered": False, "missing": [n for n, p in (("soffice", soffice), ("pdftoppm", split)) if not p]}))
        return
    os.makedirs(a.outdir, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        subprocess.run([soffice, "--headless", "--convert-to", "pdf", "--outdir", tmp, a.pptx], check=True,
                       capture_output=True, timeout=300)
        pdf = glob.glob(os.path.join(tmp, "*.pdf"))[0]
        shutil.copy(pdf, os.path.join(a.outdir, "deck.pdf"))
        subprocess.run([split, "-png", "-r", "60", pdf, os.path.join(a.outdir, "slide")], check=True, timeout=300)
    files = sorted(glob.glob(os.path.join(a.outdir, "slide*.png")))
    sheet(files, os.path.join(a.outdir, "sheet.png"))
    print(json.dumps({"rendered": True, "sheet": os.path.join(a.outdir, "sheet.png"), "slides": files,
                      "pdf": os.path.join(a.outdir, "deck.pdf")}))


if __name__ == "__main__":
    main()
