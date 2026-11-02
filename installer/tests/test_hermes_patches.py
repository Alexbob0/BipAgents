"""BipAgents' Hermes patches (hermes/patches/, made on the pinned Hermes v0.22.5): put on as is, and a patch whose
context moved (a Hermes updated past the pin) merged only when that merge is clean."""
import subprocess
from pathlib import Path

from bipinstall import hermes

ROOT = Path(__file__).resolve().parents[2]


def git(checkout: Path, *args: str) -> str:
    return subprocess.run(["git", "-C", str(checkout), "-c", "user.email=t@t", "-c", "user.name=t", *args],
                          check=True, capture_output=True, text=True).stdout.strip()


def test_the_patches_are_made_on_the_pinned_hermes():
    folder = ROOT / "hermes" / "patches"
    base = (folder / "BASE").read_text().split()
    assert base[0].startswith(hermes.HERMES_COMMIT) and base[1] == "v0.22.5"
    names = [p.name for p in hermes.set_patches(folder)]
    assert names[0].startswith("0005-") and not any(n.startswith(("0001-", "0004-", "0012-")) for n in names)


def moved_context(tmp_path):
    """A patch made on the base, then Hermes moved a line of its context: `git apply` refuses it, a merge doesn't."""
    checkout = tmp_path / "hermes-agent"
    checkout.mkdir()
    git(checkout, "init", "-q")
    (checkout / "app.py").write_text("".join(f"line {i}\n" for i in range(12)))
    git(checkout, "add", ".")
    git(checkout, "commit", "-qm", "base")
    (checkout / "app.py").write_text((checkout / "app.py").read_text().replace("line 6\n", "line 6 patched\n"))
    patch = tmp_path / "0001-x.patch"
    patch.write_text(git(checkout, "diff", "--full-index") + "\n")
    git(checkout, "checkout", "--", "app.py")
    (checkout / "app.py").write_text((checkout / "app.py").read_text().replace("line 4\n", "line 4 upstream\n"))
    git(checkout, "commit", "-qam", "upstream moved")
    return checkout, patch


def test_a_patch_whose_context_moved_is_merged_and_the_index_left_alone(tmp_path):
    checkout, patch = moved_context(tmp_path)
    runner = hermes.Runner()
    assert subprocess.run(["git", "-C", str(checkout), "apply", "--check", str(patch)]).returncode != 0
    assert hermes.apply_fitting(runner, checkout, [patch]) == str(patch)
    text = (checkout / "app.py").read_text()
    assert "line 4 upstream\n" in text and "line 6 patched\n" in text
    assert git(checkout, "diff", "--cached") == ""   # as a plain `git apply` leaves it
    # Put on again: nothing changes, and it says so (no second copy recorded).
    assert hermes.apply_3way(runner, checkout, patch) is False
    assert (checkout / "app.py").read_text() == text


def test_a_merge_with_conflicts_changes_nothing(tmp_path):
    checkout, patch = moved_context(tmp_path)
    (checkout / "app.py").write_text((checkout / "app.py").read_text().replace("line 6\n", "line 6 upstream\n"))
    git(checkout, "commit", "-qam", "upstream changed the same line")
    before = (checkout / "app.py").read_text()
    assert hermes.apply_fitting(hermes.Runner(), checkout, [patch]) is None
    assert (checkout / "app.py").read_text() == before and "<<<<<<<" not in before
    assert git(checkout, "status", "--porcelain", "--untracked-files=no") == ""
