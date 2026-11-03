"""The libraries the scripts use, pinned. They come with the agents' sandbox image (sandbox/Containerfile); in a sandbox
without them (an older image, a self-hosted Hermes) they are installed once, at these versions, into a folder of the
sandbox's own (~/.cache/bipagents-pylib), through the registries' proxy. Nothing is installed when they are there."""
import importlib
import os
import subprocess
import sys

# python-pptx and python-docx: MIT; lxml: BSD-3-Clause; Pillow: MIT-CMU (HPND); XlsxWriter: BSD-2-Clause (python-pptx's
# charts); typing_extensions: PSF-2.0. Keep in step with sandbox/requirements.txt.
PINS = {
    "pptx": "python-pptx==1.0.2",
    "docx": "python-docx==1.2.0",
    "lxml": "lxml==6.1.3",
    "PIL": "Pillow==12.3.0",
    "xlsxwriter": "XlsxWriter==3.2.9",
    "typing_extensions": "typing_extensions==4.16.0",
}
NEEDS = {"pptx": ["lxml", "PIL", "xlsxwriter", "typing_extensions"], "docx": ["lxml", "typing_extensions"]}
TARGET = os.path.join(os.path.expanduser("~"), ".cache", "bipagents-pylib")


def _missing(names):
    out = []
    for name in names:
        try:
            importlib.import_module(name)
        except ImportError:
            out.append(name)
    return out


def need(*modules):
    """Makes `modules` (and what they need) importable; exits with a short message when it can't."""
    if os.path.isdir(TARGET) and TARGET not in sys.path:
        sys.path.insert(0, TARGET)
    wanted = []
    for m in modules:
        for name in [m] + NEEDS.get(m, []):
            if name not in wanted:
                wanted.append(name)
    missing = _missing(wanted)
    if not missing:
        return
    pins = [PINS[name] for name in missing if name in PINS]
    done = subprocess.run([sys.executable, "-m", "pip", "install", "--quiet", "--disable-pip-version-check",
                           "--no-warn-script-location", "--target", TARGET, *pins],
                          stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, timeout=600)
    if TARGET not in sys.path:
        sys.path.insert(0, TARGET)
    importlib.invalidate_caches()
    still = _missing(wanted)
    if done.returncode != 0 or still:
        sys.stderr.write("Missing Python libraries: " + ", ".join(PINS.get(n, n) for n in still or missing)
                         + "\n" + (done.stderr or "")[-600:])
        sys.exit(3)
