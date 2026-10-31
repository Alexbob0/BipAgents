"""The gateway and the agents' sandboxes (installer/bipinstall/migrations.py)."""
def test_the_gateway_removes_the_agents_dead_sandboxes_before_it_starts(tmp_path, monkeypatch):
    """migrations.sandboxes_at_gateway_start (2026-10-31): a gateway restart kills its sandboxes' fuse-overlayfs mounts
    and Hermes kept the dead containers; a drop-in for each name the gateway's unit may have removes them first."""
    import sys
    from bipinstall import hermes, migrations
    monkeypatch.setattr(sys, "platform", "linux")
    runner = hermes.Runner(dry=False)
    ran = []
    monkeypatch.setattr(runner, "run", lambda args, **kw: ran.append(list(args)))
    said = migrations.sandboxes_at_gateway_start(runner, tmp_path)
    assert "dead sandboxes" in said
    for unit in ("hermes-gateway.service", "bipagents-hermes.service"):
        text = (tmp_path / ".config/systemd/user" / (unit + ".d") / "60-bipagents-sandboxes.conf").read_text()
        assert text.splitlines()[-1].startswith("ExecStartPre=-/bin/sh -c 'podman ps -a")
        assert 'grep "^hermes-"' in text and "%" not in text   # never another container (browsers, egress)
    assert ["systemctl", "--user", "daemon-reload"] in ran
    assert "2026-10-31-sandboxes-at-gateway-start" in [mid for mid, _ in migrations.MIGRATIONS]
