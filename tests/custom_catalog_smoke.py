"""Exercise optional Custom fields against an isolated local validation server."""
import json
import urllib.error
import urllib.request
import uuid

BASE = "http://127.0.0.1:5180"
headers = {"Content-Type": "application/json", "Origin": BASE}


def request(method, path, value=None, raw=False):
    body = value if raw else json.dumps(value).encode() if value is not None else None
    req = urllib.request.Request(BASE + "/api/v1" + path, data=body, method=method,
                                 headers={**headers, "Content-Type": "application/octet-stream" if raw else "application/json"})
    with urllib.request.urlopen(req, timeout=20) as response:
        cookie = response.headers.get("Set-Cookie")
        if cookie:
            headers["Cookie"] = cookie.split(";", 1)[0]
        body = response.read()
        return json.loads(body) if body else None


session = request("POST", "/login", {"username": "admin", "password": "RigDeck-local-test-2026"})
headers["X-CSRF-Token"] = session["csrf"]
created = []
try:
    # Rigs download packages themselves now: the flight sheet carries the URL, not a catalogue id.
    suffix = uuid.uuid4().hex[:8]
    wallet = request("POST", "/catalog/wallets", {"name": "Custom wallet " + suffix,
                     "data": {"coin_symbol": "CUSTOMSMOKE" + suffix, "address": "local-fixture-only"}})
    created.extend(["/catalog/coins/" + wallet["data"]["coin_id"], "/catalog/wallets/" + wallet["id"]])
    miner = {"adapter": "hive-custom", "name": "fixture", "custom_name": "fixture",
             "url": "https://example.invalid/fixture.tar.gz"}
    sheet = request("POST", "/flight-sheets", {"name": "Custom flight " + suffix, "tasks": [{
        "instance": "custom1", "wallet_id": wallet["id"], "miner": miner,
        "config": {"algorithm": "", "wallet_template": "%WAL%.%WORKER_NAME%", "urls": [], "user_config": "fixture=true"}}]})
    created.append("/flight-sheets/" + sheet["id"])
    saved = next(item for item in request("GET", "/flight-sheets") if item["id"] == sheet["id"])
    assert saved["tasks"][0]["miner"]["url"] == miner["url"]
    assert saved["tasks"][0]["config"]["algorithm"] == ""
    assert saved["tasks"][0]["config"]["user_config"] == "fixture=true"
    try:
        request("POST", "/flight-sheets", {"name": "Bad " + suffix, "tasks": [{"instance": "c", "wallet_id": wallet["id"],
                "miner": {**miner, "url": "http://example.invalid/x.tar.gz"}, "config": {}}]})
        raise AssertionError("plain http without SHA256 must be rejected")
    except urllib.error.HTTPError as error:
        assert error.code == 400
    print("PASS: inline HiveOS custom miner, optional algorithm, flight round trip, http needs SHA256")
finally:
    for path in reversed(created):
        request("DELETE", path)
