"""SQLite storage (devices, outbox, ntfy cursors). sqlite3 calls run in a worker thread."""
from __future__ import annotations

import asyncio
import json
import os
import sqlite3
import threading
import uuid
from datetime import datetime, timedelta, timezone
from typing import Any, Dict, List, Optional

SCHEMA = """
CREATE TABLE IF NOT EXISTS devices (
    token TEXT PRIMARY KEY,
    environment TEXT NOT NULL,
    agent_ids TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS outbox (
    id TEXT PRIMARY KEY,
    agent TEXT NOT NULL,
    ntfy_id TEXT,
    title TEXT,
    text TEXT NOT NULL,
    created_at TEXT NOT NULL,
    sent_at TEXT,
    session_id TEXT,
    audio_path TEXT,
    UNIQUE (agent, ntfy_id)
);
CREATE INDEX IF NOT EXISTS outbox_created_at ON outbox (created_at);
CREATE TABLE IF NOT EXISTS cron_jobs (
    agent TEXT NOT NULL,
    job TEXT NOT NULL,
    name TEXT,
    notify INTEGER NOT NULL DEFAULT 1,
    last_seen TEXT,
    PRIMARY KEY (agent, job)
);
CREATE TABLE IF NOT EXISTS ntfy_cursor (
    agent TEXT PRIMARY KEY,
    last_id TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
"""


def utcnow() -> datetime:
    return datetime.now(timezone.utc)


def iso(dt: datetime) -> str:
    """Canonical, lexicographically sortable UTC timestamp: 2026-10-04T08:15:00.123Z."""
    return dt.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.") + f"{dt.microsecond // 1000:03d}Z"


def parse_since(value: str) -> datetime:
    """ISO 8601 (with ``Z`` or an offset; naive = UTC) or Unix seconds."""
    value = value.strip()
    try:
        return datetime.fromtimestamp(float(value), tz=timezone.utc)
    except ValueError:
        pass
    if value.endswith(("Z", "z")):
        value = value[:-1] + "+00:00"
    dt = datetime.fromisoformat(value)
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt


class Store:
    def __init__(self, path: str):
        self.path = path
        if path != ":memory:":
            os.makedirs(os.path.dirname(path) or ".", mode=0o700, exist_ok=True)
        self._conn = sqlite3.connect(path, check_same_thread=False, isolation_level=None)
        self._conn.row_factory = sqlite3.Row
        self._lock = threading.Lock()
        with self._lock:
            self._conn.execute("PRAGMA journal_mode=WAL")
            self._conn.executescript(SCHEMA)
        if path != ":memory:":
            try:
                os.chmod(path, 0o600)
            except OSError:
                pass

    def close(self) -> None:
        with self._lock:
            self._conn.close()

    def _exec(self, sql: str, params: tuple = ()) -> List[sqlite3.Row]:
        with self._lock:
            return self._conn.execute(sql, params).fetchall()

    async def _run(self, sql: str, params: tuple = ()) -> List[sqlite3.Row]:
        return await asyncio.to_thread(self._exec, sql, params)

    async def _run_count(self, sql: str, params: tuple = ()) -> int:
        def go() -> int:
            with self._lock:
                return self._conn.execute(sql, params).rowcount
        return await asyncio.to_thread(go)

    # -- scheduled tasks -----------------------------------------------------------------------

    async def note_cron_job(self, agent: str, job: str, name: Optional[str]) -> bool:
        """Records a job seen by the cron watcher; returns whether its replies should be pushed."""
        await self._run(
            "INSERT INTO cron_jobs (agent, job, name, notify, last_seen) VALUES (?,?,?,1,?) "
            "ON CONFLICT(agent, job) DO UPDATE SET name=COALESCE(excluded.name, cron_jobs.name), "
            "last_seen=excluded.last_seen", (agent, job, name, iso(utcnow())))
        rows = await self._run("SELECT notify FROM cron_jobs WHERE agent=? AND job=?", (agent, job))
        return bool(rows[0]["notify"]) if rows else True

    async def cron_jobs(self, agent: Optional[str] = None) -> List[Dict[str, Any]]:
        sql, params = "SELECT * FROM cron_jobs", ()
        if agent:
            sql, params = sql + " WHERE agent=?", (agent,)
        rows = await self._run(sql + " ORDER BY last_seen DESC", params)
        return [{**dict(r), "notify": bool(r["notify"])} for r in rows]

    async def set_cron_notify(self, agent: str, job: str, notify: bool) -> bool:
        return await self._run_count("UPDATE cron_jobs SET notify=? WHERE agent=? AND job=?",
                                     (1 if notify else 0, agent, job)) > 0

    # -- devices -----------------------------------------------------------------------------

    async def upsert_device(self, token: str, environment: str, agent_ids: List[str]) -> Dict[str, Any]:
        now = iso(utcnow())
        await self._run(
            "INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?) "
            "ON CONFLICT(token) DO UPDATE SET environment=excluded.environment, "
            "agent_ids=excluded.agent_ids, updated_at=excluded.updated_at",
            (token, environment, json.dumps(sorted(set(agent_ids))), now, now))
        device = await self.get_device(token)
        assert device is not None
        return device

    async def get_device(self, token: str) -> Optional[Dict[str, Any]]:
        rows = await self._run("SELECT * FROM devices WHERE token=?", (token,))
        return self._device(rows[0]) if rows else None

    async def delete_device(self, token: str) -> bool:
        return await self._run_count("DELETE FROM devices WHERE token=?", (token,)) > 0

    async def devices_for_agent(self, agent: str) -> List[Dict[str, Any]]:
        rows = await self._run("SELECT * FROM devices")
        out = []
        for row in rows:
            dev = self._device(row)
            if not dev["agent_ids"] or agent in dev["agent_ids"]:
                out.append(dev)
        return out

    @staticmethod
    def _device(row: sqlite3.Row) -> Dict[str, Any]:
        return {"token": row["token"], "environment": row["environment"],
                "agent_ids": json.loads(row["agent_ids"]), "created_at": row["created_at"],
                "updated_at": row["updated_at"]}

    # -- outbox ------------------------------------------------------------------------------

    async def add_outbox(self, agent: str, text: str, title: Optional[str] = None,
                         ntfy_id: Optional[str] = None, sent_at: Optional[str] = None,
                         session_id: Optional[str] = None) -> Optional[Dict[str, Any]]:
        """Insert a message; returns ``None`` when this ntfy message was already stored."""
        item_id = uuid.uuid4().hex
        count = await self._run_count(
            "INSERT OR IGNORE INTO outbox (id, agent, ntfy_id, title, text, created_at, sent_at, session_id) "
            "VALUES (?,?,?,?,?,?,?,?)",
            (item_id, agent, ntfy_id, title, text, iso(utcnow()), sent_at, session_id))
        if count == 0:
            return None
        return await self.get_outbox(item_id)

    async def set_outbox_audio(self, item_id: str, path: str) -> None:
        await self._run("UPDATE outbox SET audio_path=? WHERE id=?", (path, item_id))

    async def get_outbox(self, item_id: str) -> Optional[Dict[str, Any]]:
        rows = await self._run("SELECT * FROM outbox WHERE id=?", (item_id,))
        return dict(rows[0]) if rows else None

    async def list_outbox(self, since: Optional[datetime] = None, agent: Optional[str] = None,
                          limit: int = 100) -> List[Dict[str, Any]]:
        sql, params = "SELECT * FROM outbox WHERE 1=1", []
        if since is not None:
            sql += " AND created_at > ?"
            params.append(iso(since))
        if agent:
            sql += " AND agent = ?"
            params.append(agent)
        sql += " ORDER BY created_at ASC LIMIT ?"
        params.append(int(limit))
        return [dict(r) for r in await self._run(sql, tuple(params))]

    async def purge_outbox(self, older_than_days: int) -> List[Dict[str, Any]]:
        cutoff = iso(utcnow() - timedelta(days=older_than_days))
        rows = await self._run("SELECT id, audio_path FROM outbox WHERE created_at < ?", (cutoff,))
        await self._run("DELETE FROM outbox WHERE created_at < ?", (cutoff,))
        return [dict(r) for r in rows]

    # -- ntfy cursors ------------------------------------------------------------------------

    async def get_cursor(self, agent: str) -> Optional[str]:
        rows = await self._run("SELECT last_id FROM ntfy_cursor WHERE agent=?", (agent,))
        return rows[0]["last_id"] if rows else None

    async def set_cursor(self, agent: str, last_id: str) -> None:
        await self._run(
            "INSERT INTO ntfy_cursor (agent, last_id, updated_at) VALUES (?,?,?) "
            "ON CONFLICT(agent) DO UPDATE SET last_id=excluded.last_id, updated_at=excluded.updated_at",
            (agent, last_id, iso(utcnow())))
