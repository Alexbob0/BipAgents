"""What to install on this machine, and what it will cost: the profile follows the memory left once the system
and the people's agents are counted, and where the model runs (usually elsewhere).

Figures measured on our machines (to be confirmed on each system): a Hermes gateway serving one person's agents
0.7 to 1 GB, a bridge 0.1 GB, the Pocket voices 0.6 GB, a navigation 1 to 1.5 GB, the system about 5 GB on macOS."""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import List

from .detect import Machine

SYSTEM_GIB = {"macos": 5.0, "linux": 2.0}
PERSON_GIB = 1.1        # one multiplex gateway + its bridge
VOICES_GIB = 0.6        # Pocket TTS, shared by everyone on the machine
BROWSER_GIB = 1.5       # one navigation at a time on small machines
LOCAL_MODEL_GIB = 5.5   # a 7-8B model, 4-bit, with its context


@dataclass
class Plan:
    profile: str                    # "léger" | "local" | "costaud"
    model_here: bool
    browser: str                    # "on-demand" | "permanent" | "none"
    people: int                     # comfortable number of people with this setup
    notes: List[str] = field(default_factory=list)

    def memory_gib(self, people: int = 1) -> float:
        total = VOICES_GIB + PERSON_GIB * people
        total += BROWSER_GIB if self.browser == "on-demand" else (BROWSER_GIB * people if self.browser == "permanent" else 0)
        total += LOCAL_MODEL_GIB if self.model_here else 0
        return total


def make_plan(machine: Machine, model_here: bool) -> Plan:
    """The setup this machine carries comfortably. `model_here`: the model runs on this machine (the exception)."""
    free = machine.memory_gib - SYSTEM_GIB.get(machine.system, 3.0)
    notes: List[str] = []
    big = machine.memory_gib >= 64
    browser = "permanent" if big else "on-demand"
    profile = "costaud" if big else ("local" if model_here else "léger")
    plan = Plan(profile=profile, model_here=model_here, browser=browser, people=1, notes=notes)
    base = plan.memory_gib(0)
    per_person = PERSON_GIB + (BROWSER_GIB if browser == "permanent" else 0)
    plan.people = max(0, int((free - base) // per_person))
    if plan.people == 0:
        if browser != "none":
            plan.browser = "none"
            notes.append("Not enough memory for the agents' browser: it is left out.")
            plan.people = max(0, int((free - plan.memory_gib(0)) // PERSON_GIB))
    if model_here:
        notes.append(f"The local model alone takes about {LOCAL_MODEL_GIB:.1f} GB. "
                        "A model on another machine or through an API frees that space.")
    notes.append("Bip voices by Pocket (CPU); a voice model already running here or on your network can speak too.")
    return plan
