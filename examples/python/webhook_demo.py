"""
Matrix Bridge Webhook Integration Demo (Python / Flask)

This demo shows how an external platform integrates with the Matrix Bridge
via HTTP webhooks. It covers two directions:

  Outbound (Matrix -> External):
    The bridge POSTs events to our /webhook endpoint whenever something
    happens in a bridged Matrix room.

  Inbound (External -> Matrix):
    We call the bridge's REST API to send messages, upload media, and
    manage room mappings.

Requirements: Python 3.10+, Flask, requests
    pip install -r requirements.txt

Environment variables:
    BRIDGE_URL   - Base URL of the bridge API (default: http://localhost:29320)
    PLATFORM     - Platform identifier registered with the bridge (default: myapp)
    ROOM_ID      - External room ID to bridge (default: general)
    WEBHOOK_PORT - Port this demo listens on (default: 5050)
    WEBHOOK_HOST - Host for the webhook callback URL (default: http://localhost:5050)
"""

from __future__ import annotations

import logging
import os
import sys
from pathlib import Path

import requests
from flask import Flask, Request, jsonify, request

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

BRIDGE_URL: str = os.environ.get("BRIDGE_URL", "http://localhost:29320")
PLATFORM: str = os.environ.get("PLATFORM", "myapp")
ROOM_ID: str = os.environ.get("ROOM_ID", "general")
WEBHOOK_PORT: int = int(os.environ.get("WEBHOOK_PORT", "5050"))
WEBHOOK_HOST: str = os.environ.get("WEBHOOK_HOST", f"http://localhost:{WEBHOOK_PORT}")

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
)
log = logging.getLogger(__name__)

app = Flask(__name__)

# ---------------------------------------------------------------------------
# Outbound: receive webhook callbacks from the bridge
# ---------------------------------------------------------------------------


@app.route("/webhook", methods=["POST"])
def handle_webhook():
    """Receive an event pushed by the bridge (Matrix -> External).

    The bridge sends a JSON payload for every message in the bridged room.
    Cross-platform forwarded messages include a ``source_platform`` field at
    the top level, indicating which platform the message originally came from.

    Payload example (see README for full schema):
        {
            "event": "message",
            "platform": "myapp",
            "source_platform": "telegram",       // optional
            "message": { ... }
        }
    """
    payload: dict = request.get_json(silent=True) or {}

    event_type: str = payload.get("event", "unknown")
    source_platform: str | None = payload.get("source_platform")
    message: dict = payload.get("message", {})

    sender: dict = message.get("sender", {})
    content: dict = message.get("content", {})

    display_name: str = sender.get("display_name", "Unknown")
    body: str = content.get("body", "")
    content_type: str = content.get("type", "text")

    # Distinguish cross-platform forwarded messages from native Matrix ones.
    if source_platform:
        log.info(
            "[cross-platform] %s (via %s): [%s] %s",
            display_name,
            source_platform,
            content_type,
            body,
        )
    else:
        log.info(
            "[%s] %s: [%s] %s",
            event_type,
            display_name,
            content_type,
            body,
        )

    # Respond with 200 so the bridge knows we processed the event.
    return jsonify({"status": "ok"}), 200


# ---------------------------------------------------------------------------
# Inbound helpers: send data TO the bridge (External -> Matrix)
# ---------------------------------------------------------------------------


def send_text_message(text: str, *, sender_id: str = "bot", sender_name: str = "My Bot") -> dict:
    """Send a plain-text message to Matrix through the bridge.

    POST /api/v1/message
    """
    payload = {
        "platform": PLATFORM,
        "room_id": ROOM_ID,
        "sender": {
            "id": sender_id,
            "display_name": sender_name,
        },
        "content": {
            "type": "text",
            "body": text,
        },
    }

    resp = requests.post(f"{BRIDGE_URL}/api/v1/message", json=payload, timeout=10)
    resp.raise_for_status()
    result: dict = resp.json()
    log.info("Sent text message: %s", result)
    return result


def upload_media(file_path: str | Path) -> dict:
    """Upload a file to the bridge and return the media metadata.

    POST /api/v1/upload  (multipart/form-data)

    Returns a dict that typically includes an ``mxc_url`` (or similar)
    which you then reference in a subsequent message.
    """
    path = Path(file_path)
    if not path.is_file():
        raise FileNotFoundError(f"File not found: {path}")

    with path.open("rb") as fh:
        files = {"file": (path.name, fh)}
        resp = requests.post(f"{BRIDGE_URL}/api/v1/upload", files=files, timeout=30)

    resp.raise_for_status()
    result: dict = resp.json()
    log.info("Uploaded media: %s", result)
    return result


def send_image_message(
    file_path: str | Path,
    *,
    sender_id: str = "bot",
    sender_name: str = "My Bot",
) -> dict:
    """Upload an image then send it as an image message to Matrix.

    This is a two-step process:
      1. Upload the file via /api/v1/upload to obtain an mxc:// URI.
      2. Send a message with content type "image" referencing that URI.
    """
    # Step 1: upload
    media = upload_media(file_path)
    mxc_url: str = media.get("mxc_url", media.get("url", ""))

    # Step 2: send image message
    payload = {
        "platform": PLATFORM,
        "room_id": ROOM_ID,
        "sender": {
            "id": sender_id,
            "display_name": sender_name,
        },
        "content": {
            "type": "image",
            "url": mxc_url,
            "body": Path(file_path).name,
        },
    }

    resp = requests.post(f"{BRIDGE_URL}/api/v1/message", json=payload, timeout=10)
    resp.raise_for_status()
    result: dict = resp.json()
    log.info("Sent image message: %s", result)
    return result


# ---------------------------------------------------------------------------
# Setup: register webhook + room mapping on startup
# ---------------------------------------------------------------------------


def register_webhook() -> dict:
    """Register this server as a webhook receiver with the bridge.

    POST /api/v1/webhooks

    The bridge will POST events to the callback URL we provide.
    """
    payload = {
        "platform": PLATFORM,
        "url": f"{WEBHOOK_HOST}/webhook",
    }

    resp = requests.post(f"{BRIDGE_URL}/api/v1/webhooks", json=payload, timeout=10)
    resp.raise_for_status()
    result: dict = resp.json()
    log.info("Registered webhook: %s", result)
    return result


def create_room_mapping() -> dict:
    """Create a mapping between an external room and a Matrix room.

    POST /api/v1/rooms

    This tells the bridge which Matrix room corresponds to our external
    room identifier.
    """
    payload = {
        "platform": PLATFORM,
        "external_id": ROOM_ID,
    }

    resp = requests.post(f"{BRIDGE_URL}/api/v1/rooms", json=payload, timeout=10)
    resp.raise_for_status()
    result: dict = resp.json()
    log.info("Created room mapping: %s", result)
    return result


def setup() -> None:
    """Run one-time setup: register webhook and room mapping."""
    log.info("Setting up bridge integration...")
    log.info("  Bridge URL : %s", BRIDGE_URL)
    log.info("  Platform   : %s", PLATFORM)
    log.info("  Room ID    : %s", ROOM_ID)
    log.info("  Webhook URL: %s/webhook", WEBHOOK_HOST)

    try:
        register_webhook()
    except requests.RequestException as exc:
        log.warning("Failed to register webhook (bridge may not be running): %s", exc)

    try:
        create_room_mapping()
    except requests.RequestException as exc:
        log.warning("Failed to create room mapping: %s", exc)

    log.info("Setup complete. Listening for webhook callbacks...")


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    setup()

    # Optionally send a test message on startup.
    if "--send-test" in sys.argv:
        send_text_message("Hello from the Python webhook demo!")

    app.run(host="0.0.0.0", port=WEBHOOK_PORT, debug=False)
