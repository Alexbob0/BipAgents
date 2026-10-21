"""tools_version() of agent_tools.py for tests/tools-knowledge_agent_tools.rs (run from bridge/ with the venv)."""
from pathlib import Path

from bipbridge import agent_tools

out = Path(__file__).parent / "agent_tools_version.txt"
out.write_text(agent_tools.tools_version() + "\n")
