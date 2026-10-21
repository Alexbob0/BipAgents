"""Golden fixtures for the files the agents group writes in Hermes' profiles and in the bridge's config: the Python
functions run on sample files, their output text recorded byte for byte (tests/golden/agents_files.json).

Run from bridge/: PYTHONPATH=. "<venv>/bin/python" ../bridge-rs/tests/golden/gen_agents_files.py"""
import json
import os
import tempfile
from pathlib import Path

from bipbridge import agents_admin, own_skills
from bipbridge.config import AgentConfig, parse_config

HERE = os.path.dirname(os.path.abspath(__file__))

PROFILES = [
    None,
    "",
    "description: Recettes et menus\n",
    "name: cuisine\nui_meta:\n  color: '#ffaa00'\n",
    "ui_meta:\n  hermes-bots:\n    since: 2026-10-01\n",
    "description: \"Une description très longue qui dépasse la largeur de quatre-vingts colonnes pour voir le pliage\"\nyes: no\n",
    "- not a dict\n",
]

CONFIGS = [
    "model:\n  provider: custom\n  default: glm\n  base_url: http://gpu:8888/v1\nagent:\n  max_turns: 60\n",
    "model:\n  provider: custom\n  default: glm-5.3-flash\n  base_url: https://api.eurouter.ai/api/v1\n  supports_vision: true\n"
    "agent:\n  max_turns: 60\n  reasoning_effort: 'off'\nproviders:\n  other:\n    api: http://x\n",
    "# a comment Hermes wrote\nmodel:\n  provider: custom\n  base_url: \"http://gpu:8888/v1\"\n  default: 'qwen3:32b'\n"
    "toolsets:\n- web\n- terminal\nterminal:\n  backend: docker\n  docker_volumes: [\"/a:/b:ro\"]\nproviders:\n"
    "  bipagents-thinking:\n    api: http://gpu:8888/v1\n    extra_body:\n      chat_template_kwargs:\n        enable_thinking: true\n"
    "display:\n  personality: \"Café — été\"\n  empty: ''\n  n: 0755\n  on: on\n",
]

SKILLS = "model:\n  provider: custom\nskills:\n  disabled: [plant-care, arxiv]\n  external_dirs: []\n"


def main():
    cases = []
    with tempfile.TemporaryDirectory() as tmp:
        folder = Path(tmp) / "p"
        folder.mkdir()
        for text in PROFILES:
            path = folder / "profile.yaml"
            if path.exists():
                path.unlink()
            if text is not None:
                path.write_text(text, encoding="utf-8")
            try:
                agents_admin.mark_as_bot(folder)
                out = path.read_text(encoding="utf-8")
            except Exception as exc:   # noqa: BLE001
                out = None
            cases.append({"fn": "mark_as_bot", "input": text, "output": out})

        home = Path(tmp) / "home"
        profile = home / "profiles" / "vie"
        profile.mkdir(parents=True)
        config = parse_config({"bridge_key": "b" * 40, "pocket": {"url": "http://p"},
                               "agents": {"vie": {"hermes_url": "http://h", "hermes_key": "k" * 32,
                                                  "host_home": str(home)}}})
        agent = config.agent("vie")
        for text in CONFIGS:
            for level in ("none", "low", "xhigh"):
                (profile / "config.yaml").write_text(text, encoding="utf-8")
                agents_admin.set_thinking(config, agent, level)
                cases.append({"fn": "set_thinking", "input": text, "arg": level,
                              "output": (profile / "config.yaml").read_text(encoding="utf-8")})
        plain = AgentConfig(name="vie", display_name="Vie", hermes_url="http://h", hermes_key="k" * 32)
        cfg2 = parse_config({"bridge_key": "b" * 40, "pocket": {"url": "http://p"},
                             "hermes": {"home": str(home)}})
        for text in CONFIGS[:2]:
            for name, vision in (("deepseek-v4", False), ("z-ai/glm-5.3:free", None), ("é", True)):
                (profile / "config.yaml").write_text(text, encoding="utf-8")
                agents_admin.set_model(cfg2, plain, name, vision)
                cases.append({"fn": "set_model", "input": text, "arg": [name, vision],
                              "output": (profile / "config.yaml").read_text(encoding="utf-8")})

        skills = Path(tmp) / "s"
        for name in ("plant-care", "monthly-budget"):
            (skills / "skills" / name).mkdir(parents=True)
            (skills / "skills" / name / "SKILL.md").write_text(f"---\nname: {name}\ndescription: x\n---\n")
        for changes in ({"plant-care": True, "monthly-budget": False}, {"monthly-budget": True},
                        {"plant-care": False, "monthly-budget": False}):
            (skills / "config.yaml").write_text(SKILLS, encoding="utf-8")
            own_skills.set_enabled(skills, changes)
            cases.append({"fn": "set_enabled", "input": SKILLS, "arg": changes,
                          "output": (skills / "config.yaml").read_text(encoding="utf-8")})

        toml_path = Path(tmp) / "bridge.toml"
        base = ('bridge_key = "x"\n\n[agents.vie]\ndisplay_name = "Vie"\nhermes_url = "http://old"\n'
                'hermes_key_file = "/k"\n# her voice\nvoice = "pocket:colibri"\nspace = "/old"\n\n[agents.other]\n'
                'display_name = "O"\n\n\n')
        agents = [
            AgentConfig(name="vie", display_name='Vie "la" \\ grande', hermes_url="http://127.0.0.1:8660/p/vie",
                        hermes_key="k" * 32, public_url="https://x/hermes/vie", upload_dir_host="/s/u",
                        upload_dir_container="/fichiers", isolated=True, sandboxed=True, python=["/opt/py", "-I"],
                        hermes_home="/s/vie", space="/s", desktop_vnc="ws://v"),
            AgentConfig(name="vie", display_name="Vie", hermes_url="http://h", hermes_key="k" * 32),
        ]
        for agent in agents:
            toml_path.write_text(base, encoding="utf-8")
            config = parse_config({"bridge_key": "b" * 40, "pocket": {"url": "http://p"}}, source_path=str(toml_path))
            agents_admin.save_agent(config, agent)
            cases.append({"fn": "save_agent", "input": base, "arg": {k: v for k, v in agent.__dict__.items()},
                          "output": toml_path.read_text(encoding="utf-8")})
    with open(os.path.join(HERE, "agents_files.json"), "w", encoding="utf-8") as fh:
        json.dump(cases, fh, ensure_ascii=False, indent=1)
    print(len(cases), "cases")


main()
