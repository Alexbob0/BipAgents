"""The local-network door: when Tailscale is down (no Internet at home, the VPN off), the app reaches the bridge
on the LAN, over HTTPS with a self-signed certificate it pins (the SHA-256 of the certificate travels in the pairing
QR code), and Hermes through the bridge under ``/hermes/<agent>/…``. Off unless ``[lan] enabled = true``.

Also builds the pairing payload (what the QR code carries) and the LAN details the app refreshes on its own
(``GET /v1/pairing``) so a new IP address needs no new QR code while the tailnet works."""
from __future__ import annotations

import datetime
import hashlib
import ipaddress
import logging
import os
import socket
from typing import Any, AsyncIterator, Dict, Optional, Tuple

import httpx
from fastapi import APIRouter, HTTPException, Request
from fastapi.responses import StreamingResponse

from .config import AgentConfig, Config
from .logs import fields

log = logging.getLogger("bipbridge.lan")

CERT_NAME = "bridge-lan.pem"
KEY_NAME = "bridge-lan.key"
# Headers that belong to one connection, not to the request being relayed.
_HOP = {"connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "te", "trailers",
        "transfer-encoding", "upgrade", "host", "content-length"}


def certificate_paths(config: Config) -> Tuple[str, str]:
    return os.path.join(config.lan.cert_dir, CERT_NAME), os.path.join(config.lan.cert_dir, KEY_NAME)


def ensure_certificate(config: Config) -> Tuple[str, str]:
    """The LAN certificate and key, created once (ECDSA P-256, 20 years: the app pins it, no CA involved)."""
    cert_path, key_path = certificate_paths(config)
    if os.path.exists(cert_path) and os.path.exists(key_path):
        return cert_path, key_path
    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import ec
    from cryptography.x509.oid import NameOID

    os.makedirs(config.lan.cert_dir, mode=0o700, exist_ok=True)
    key = ec.generate_private_key(ec.SECP256R1())
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "BipAgents bridge (LAN)")])
    now = datetime.datetime.now(datetime.timezone.utc)
    cert = (x509.CertificateBuilder().subject_name(name).issuer_name(name).public_key(key.public_key())
            .serial_number(x509.random_serial_number()).not_valid_before(now - datetime.timedelta(days=1))
            .not_valid_after(now + datetime.timedelta(days=365 * 20))
            .sign(key, hashes.SHA256()))
    key_bytes = key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                                  serialization.NoEncryption())
    fd = os.open(key_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "wb") as fh:
        fh.write(key_bytes)
    with open(cert_path, "wb") as fh:
        fh.write(cert.public_bytes(serialization.Encoding.PEM))
    log.info("LAN certificate created", extra=fields(path=cert_path))
    return cert_path, key_path


def fingerprint(config: Config) -> Optional[str]:
    """SHA-256 of the certificate (DER), lowercase hex: what the app pins."""
    cert_path, _ = certificate_paths(config)
    try:
        from cryptography import x509
        from cryptography.hazmat.primitives import serialization
        with open(cert_path, "rb") as fh:
            der = x509.load_pem_x509_certificate(fh.read()).public_bytes(serialization.Encoding.DER)
    except (OSError, ValueError):
        return None
    return hashlib.sha256(der).hexdigest()


def detect_address() -> Optional[str]:
    """This machine's address on its main network (no packet is sent: a UDP « connect » only picks the route)."""
    for target in ("192.168.0.1", "10.0.0.1", "172.16.0.1"):
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
                probe.connect((target, 9))
                address = probe.getsockname()[0]
        except OSError:
            continue
        ip = ipaddress.ip_address(address)
        if ip.is_private and not ip.is_loopback and not address.startswith("100."):  # 100.x: the tailnet itself
            return address
    return None


def lan_info(config: Config) -> Optional[Dict[str, str]]:
    """{"url", "fingerprint"} for the app, or None when the door is off or not ready."""
    if not config.lan.enabled:
        return None
    address = config.lan.address or detect_address()
    print_fp = fingerprint(config)
    if not address or not print_fp:
        return None
    return {"url": f"https://{address}:{config.lan.port}", "fingerprint": print_fp}


def pairing_payload(config: Config, agent: AgentConfig) -> Dict[str, Any]:
    """What the pairing QR code carries (the app's « Scanner le QR code »): tailnet addresses, keys, voice, and the
    LAN door. Raises ValueError when a tailnet address is missing from the config."""
    if not agent.public_url:
        raise ValueError(f"agents.{agent.name}.public_url is missing (its Hermes address on the tailnet)")
    if not config.public_url:
        raise ValueError("public_url is missing (the bridge's address on the tailnet)")
    payload: Dict[str, Any] = {"name": agent.display_name, "baseURL": agent.public_url, "apiKey": agent.hermes_key,
                               "bridgeURL": config.public_url, "bridgeKey": config.bridge_key}
    if agent.voice:
        payload["voice"] = agent.voice
    lan = lan_info(config)
    if lan:
        payload["lan"] = lan
    return payload


def install_payload(config: Config) -> Dict[str, Any]:
    """The install's QR code: the bridge's tailnet address and key (and the LAN door). The app then asks the bridge
    for every agent (``GET /v1/agents``), so one scan adds them all, and agents created later show up too."""
    if not config.public_url:
        raise ValueError("public_url is missing (the bridge's address on the tailnet)")
    payload: Dict[str, Any] = {"v": 2, "bridgeURL": config.public_url, "bridgeKey": config.bridge_key}
    lan = lan_info(config)
    if lan:
        payload["lan"] = lan
    return payload


def hermes_proxy() -> APIRouter:
    """``/hermes/<agent>/<path>`` → the agent's Hermes, streamed both ways (SSE included). Hermes checks its own key:
    the app sends it as on the tailnet."""
    router = APIRouter()

    @router.api_route("/hermes/{agent}/{path:path}", methods=["GET", "POST", "PUT", "PATCH", "DELETE"])
    async def relay(agent: str, path: str, request: Request) -> StreamingResponse:
        services = request.app.state.services
        target = services.config.agent(agent)
        if target is None or not (path.startswith(("v1/", "api/")) or path in ("health", "v1/capabilities")):
            raise HTTPException(status_code=404, detail="not found")
        headers = {k: v for k, v in request.headers.items() if k.lower() not in _HOP}
        upstream_request = services.http.build_request(
            request.method, f"{target.hermes_url}/{path}", params=request.query_params, headers=headers,
            content=await request.body(), timeout=httpx.Timeout(30.0, read=None))  # SSE: no read timeout
        try:
            upstream = await services.http.send(upstream_request, stream=True)
        except httpx.HTTPError as exc:
            log.info("LAN relay failed", extra=fields(agent=target.name, error=type(exc).__name__))
            raise HTTPException(status_code=502, detail="Hermes unreachable") from exc

        async def body() -> AsyncIterator[bytes]:
            try:
                async for chunk in upstream.aiter_bytes():  # decoded: content-encoding is not passed on
                    yield chunk
            finally:
                await upstream.aclose()

        response_headers = {k: v for k, v in upstream.headers.items() if k.lower() not in _HOP | {"content-encoding"}}
        return StreamingResponse(body(), status_code=upstream.status_code, headers=response_headers)

    return router
