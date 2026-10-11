"""What the installer shows: the Bip saying hello, numbered steps with a tick, questions in colour. Plain text when the
output is not a terminal, or with NO_COLOR set (https://no-color.org)."""
from __future__ import annotations

import os
import sys

_COLOR = sys.stdout.isatty() and not os.environ.get("NO_COLOR") and os.environ.get("TERM") != "dumb"
_TRUE = os.environ.get("COLORTERM") in ("truecolor", "24bit")


def _rgb(r: int, g: int, b: int, fallback: int) -> str:
    if not _COLOR:
        return ""
    return f"\033[38;2;{r};{g};{b}m" if _TRUE else f"\033[38;5;{fallback}m"


CORAL = _rgb(255, 122, 92, 209)      # the Bip's colour (« Quotidien »)
DEEP = _rgb(201, 74, 46, 166)
GREEN = _rgb(59, 196, 120, 78)
GREY = _rgb(140, 145, 160, 245)
RED = _rgb(229, 72, 77, 167)
BOLD = "\033[1m" if _COLOR else ""
RESET = "\033[0m" if _COLOR else ""

# A Bip, as on the app's cards: round body, two eyes, little feet. One face per mood.
_FACES = {"hello": "•ᴗ•", "busy": "•_•", "happy": "^ᴗ^", "sad": "•︵•"}


def bip(mood: str = "hello", says: str = "") -> str:
    face = _FACES.get(mood, _FACES["hello"])
    bubble = f"  {BOLD}{says}{RESET}" if says else ""
    lines = [
        f"   {CORAL}▄▄▄▄▄{RESET}",
        f" {CORAL}▄███████▄{RESET}",
        f" {CORAL}██{RESET} {face} {CORAL}██{RESET}{bubble}",
        f" {CORAL}▀███████▀{RESET}",
        f"   {DEEP}▀▀ ▀▀{RESET}",
    ]
    return "\n".join(lines)


def banner(user: str, machine: str) -> str:
    return (f"\n{bip('hello', f'Bonjour {user} ! On installe tes agents.')}\n"
            f"\n{GREY}Machine : {machine}{RESET}\n")


def step(number: int, total: int, title: str) -> str:
    return f"\n{CORAL}{BOLD}[{number}/{total}]{RESET} {BOLD}{title}{RESET}"


def done(text: str) -> str:
    return f"  {GREEN}✓{RESET} {text}"


def note(text: str) -> str:
    return f"  {GREY}·{RESET} {text}"


def fail(text: str) -> str:
    return f"  {RED}✗{RESET} {text}"


def question(text: str) -> str:
    return f"{CORAL}?{RESET} {BOLD}{text}{RESET}"


def finale(text: str) -> str:
    return f"\n{bip('happy', text)}\n"


def sad(text: str) -> str:
    return f"\n{bip('sad', text)}\n"
