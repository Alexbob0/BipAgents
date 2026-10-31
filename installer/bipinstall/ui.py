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


CORAL = _rgb(255, 122, 92, 209)      # the steps' and questions' accent
GREEN = _rgb(59, 196, 120, 78)
GREY = _rgb(140, 145, 160, 245)
RED = _rgb(229, 72, 77, 167)
BOLD = "\033[1m" if _COLOR else ""
RESET = "\033[0m" if _COLOR else ""

# A Bip, as the apps draw it: the body's grid in scripts/pixel/pixel.json (BODIES.standard, 6 × 6), one cell = two
# terminal columns, the eyes on row 2, columns 1 and 4, in the « Quotidien » colour (COLORS.peche.body, #F4B5A3) and
# INK (#16181D). Without colour the eyes are holes.
_BODY = ('.####.', '######', '######', '######', '######', '.####.')
_EYES = (2, (1, 4))
_PECHE = _rgb(244, 181, 163, 217)
_INK = _rgb(22, 24, 29, 234)


def bip(mood: str = "hello", says: str = "") -> str:
    """The Bip, saying `says` (the mood is kept for the callers: the apps' still Bip has one face)."""
    del mood
    bubble = f"  {BOLD}{says}{RESET}" if says else ""
    lines = []
    for row, cells in enumerate(_BODY):
        line = ""
        for col, cell in enumerate(cells):
            if cell != "#":
                line += "  "
            elif row == _EYES[0] and col in _EYES[1]:
                line += f"{_INK}██{RESET}" if _COLOR else "  "
            else:
                line += f"{_PECHE}██{RESET}"
        lines.append(" " + line + (bubble if row == _EYES[0] else ""))
    return "\n".join(lines)


def banner(user: str, machine: str) -> str:
    hello = f"Hello {user}! Let's install your agents."
    return (f"\n{bip('hello', hello)}\n"
            f"\n{GREY}{'Machine:'} {machine}{RESET}\n")


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
