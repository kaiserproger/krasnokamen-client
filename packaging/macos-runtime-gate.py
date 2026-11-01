#!/usr/bin/env python3
"""Manual macOS arm64 packaged startup/UI gate; Python stdlib only.

Invocation (after release.yml has created the signed app archive):
  python3 packaging/macos-runtime-gate.py --package ABS_ZIP \
    --work-dir "$RUNNER_TEMP/solaris-macos-runtime" \
    --evidence-dir "$RUNNER_TEMP/solaris-macos-evidence"

Requires a macOS arm64 window session/Metal device, ditto, matching packaged
launcher/client, source tools/mcp_client.py, and the approved public manifest.
The launcher owns hash verification for both bootstrap modes. No server/gameplay,
OS capture/input/focus, fixture scene, or physical scanout is exercised. The client
intentionally omits --server: src/launch.rs makes that flag direct-join, bypassing
the normal title UI. Only the launcher gets the loopback --server prefill.

All unfiltered output, MCP descriptors/tokens/transcripts and installed resources
stay in the private work directory, never the evidence directory. Evidence is an
explicit allowlist (also in release.yml); gate.log contains operation summaries,
not child output. Failure cleanup may kill a process, but can never pass the gate.
No tests/builds/publication are performed by this script.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import signal
import struct
import subprocess
import sys
import threading
import urllib.parse

SOURCE = Path(__file__).resolve().parents[1]
# Resolve the toolkit from known source, never the extracted app/resources cwd.
sys.path.insert(0, str(SOURCE / "tools"))
from mcp_client import Client

MANIFEST = "http://135.106.208.217:8080/static/solaris-content.json"
TITLE = "net.minecraft.client.gui.screens.TitleScreen"
OPTIONS = "net.minecraft.client.gui.screens.options.OptionsScreen"
MAX_LOG = 8 * 1024 * 1024
MAX_PNG = 32 * 1024 * 1024


class GateError(RuntimeError):
    """Only controlled, secret-free diagnostics may reach curated evidence."""


def require(condition, message):
    if not condition:
        raise GateError(message)


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def png_receipt(path):
    require(path.is_file() and not path.is_symlink(), "capture PNG missing or symlinked")
    require(0 < path.stat().st_size <= MAX_PNG, "capture PNG size outside bound")
    raw = path.read_bytes()
    require(raw[:8] == b"\x89PNG\r\n\x1a\n", "capture PNG signature invalid")
    require(len(raw) >= 33 and raw[8:16] == b"\x00\x00\x00\rIHDR", "capture PNG IHDR missing")
    width, height = struct.unpack(">II", raw[16:24])
    require(1 <= width <= 4096 and 1 <= height <= 4096, "capture PNG dimensions outside bound")
    return {"file": path.name, "sha256": hashlib.sha256(raw).hexdigest(),
            "bytes": len(raw), "width": width, "height": height}


class Process:
    """Bounded output pump; readiness comes from the pushed descriptor line."""

    def __init__(self, command, cwd, env, log):
        self.ready = threading.Event()
        self.descriptor = None
        self.error = None
        self.operation = log.name.removesuffix(".private.log")
        self.diagnostics = set()
        self.proc = subprocess.Popen(command, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                                     stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        self.thread = threading.Thread(target=self.pump, args=(log,), daemon=True)
        self.thread.start()

    def pump(self, log):
        total = 0
        try:
            with log.open("wb") as stream:
                while line := self.proc.stdout.readline(16385):
                    total += len(line)
                    require(len(line) <= 16384 and total <= MAX_LOG, "child output exceeds bound")
                    stream.write(line)
                    # Preserve only observed, source-owned harmless phrases, not
                    # arbitrary log suffixes (paths, URLs, credentials or content).
                    for phrase in (
                        b"window closed before launcher window capture completed",
                        b"writing launcher window capture",
                        b"window error:",
                        b"gpu_adapter_unavailable", b"gpu_device_unavailable", b"gpu_completion_failed",
                        b"gpu_missing_required_portable_target_formats",
                        b"render_target_dimensions_or_memory_budget_exceeded",
                        b"surface_out_of_memory", b"surface_failure_device_failed",
                        b"checksum mismatch", b"cannot reach the Solaris content manifest",
                        b"does not match the hash pinned in this launcher",
                    ):
                        if phrase in line:
                            self.diagnostics.add(phrase.decode("ascii"))
                    if line.startswith(b"automation descriptor: "):
                        require(self.descriptor is None, "duplicate automation descriptor event")
                        self.descriptor = Path(line.split(b": ", 1)[1].decode().strip())
                        self.ready.set()
        except Exception:
            self.error = "child output pump failed or exceeded bound"
            self.proc.kill()
        finally:
            self.proc.stdout.close()
            self.ready.set()

    def wait(self, timeout):
        try:
            code = self.proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            raise GateError(f"process completion deadline exceeded ({timeout}s); clean exit not observed") from None
        self.thread.join(timeout=5)
        require(not self.thread.is_alive() and self.error is None, "child output pump failed")
        require(code == 0, f"process exited nonzero ({code})")
        return code

    def stop_on_failure(self):
        if self.proc.returncode is None:
            self.proc.kill()
            self.proc.wait(timeout=10)
        self.thread.join(timeout=5)


class Gate:
    def __init__(self, args):
        self.work = args.work_dir.resolve()
        self.evidence = args.evidence_dir.resolve()
        self.package = args.package.resolve()
        self.processes = []
        self.phase = "prerequisites"
        self.report = {"status": "failed", "scope": "packaged macOS arm64 startup/title/options UI only",
                       "physical_scanout_verified": False, "full_game_parity": False,
                       "monitor_parity": False, "processes": [], "captures": []}

    def save(self, name, value):
        (self.evidence / name).write_text(json.dumps(value, indent=2) + "\n")

    def log(self, message):
        with (self.evidence / "gate.log").open("a") as stream:
            stream.write(message + "\n")

    def start(self, name, command):
        self.phase = name
        self.log(name + ": started")
        process = Process([str(part) for part in command], self.work, self.env,
                          self.work / (name + ".private.log"))
        self.processes.append(process)
        return process

    def complete(self, name, process, timeout):
        try:
            code = process.wait(timeout)
        except GateError as error:
            self.report["processes"].append({"operation": name, "exit_code": process.proc.returncode,
                                             "clean_exit": False, "failure": str(error)})
            self.log(name + ": FAILED: " + str(error))
            raise
        self.report["processes"].append({"operation": name, "exit_code": code, "clean_exit": True})
        self.log(name + ": clean exit 0")

    def run(self):
        require(platform.system() == "Darwin" and platform.machine() == "arm64",
                "requires macOS arm64 hosted window session")
        require(not self.work.is_relative_to(SOURCE) and not self.evidence.is_relative_to(SOURCE),
                "runtime/evidence must be outside checkout")
        require(not self.work.is_relative_to(self.evidence) and not self.evidence.is_relative_to(self.work),
                "private runtime and curated evidence must be disjoint")
        self.work.mkdir(parents=True, exist_ok=False, mode=0o700)
        # Do not inherit resource overrides, launcher overrides, screenshot hooks,
        # credentials, or a backend override that could change what is exercised.
        self.env = {key: os.environ[key] for key in ("PATH", "TMPDIR", "LANG", "LC_ALL") if key in os.environ}
        home = self.work / "home"
        home.mkdir()
        self.env.update(HOME=str(home), XDG_DATA_HOME=str(home / "data"),
                        XDG_CONFIG_HOME=str(home / "config"), XDG_CACHE_HOME=str(home / "cache"))
        self.report["system"] = {"os": platform.system(), "release": platform.release(),
                                 "macos": platform.mac_ver()[0], "machine": platform.machine()}
        self.report["runner"] = {key: os.environ.get(key) for key in
                                  ("ImageOS", "ImageVersion", "RUNNER_OS", "RUNNER_ARCH", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT")}
        revisions = {}
        for name, root, expected in (("native", SOURCE, os.environ.get("GITHUB_SHA")),
                                     ("core", SOURCE / "solaris-server", os.environ.get("SOLARIS_REVISION")),
                                     ("plugins", SOURCE / "solaris-default-plugins", os.environ.get("SOLARIS_PLUGINS_REVISION"))):
            actual = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"],
                                             stderr=subprocess.DEVNULL, timeout=10).decode().strip()
            require(re.fullmatch(r"[0-9a-f]{40}", actual) and actual == expected,
                    name + " checkout does not match workflow commit/pin")
            revisions[name] = actual
        self.report["commits"] = revisions
        self.report["source_hashes"] = {name: sha256(SOURCE / name) for name in
                                         ("Cargo.lock", "compatibility.lock", "tools/mcp_client.py", "packaging/macos-runtime-gate.py")}
        self.report["manifest"] = MANIFEST
        self.report["package_sha256"] = sha256(self.package)
        extract = self.work / "extracted"
        extract.mkdir()
        process = self.start("extract", ["ditto", "-x", "-k", self.package, extract])
        self.complete("extract", process, 60)
        bins = extract / "Solaris.app/Contents/MacOS"
        launcher, native = bins / "solaris-launcher", bins / "solaris-client"
        for binary in (launcher, native):
            require(binary.is_file() and not binary.is_symlink() and os.access(binary, os.X_OK),
                    "extracted executable missing, symlinked or not executable")
        self.report["executables"] = {binary.name: sha256(binary) for binary in (launcher, native)}
        process = self.start("gpu-diagnostics", [native, "doctor", "--json"])
        self.complete("gpu-diagnostics", process, 60)
        doctor = json.loads((self.work / "gpu-diagnostics.private.log").read_text())
        selected = doctor.get("selected", {})
        fields = ("name", "backend", "vendor", "device", "device_type", "driver", "driver_info")
        require(all(isinstance(selected.get(key), (str, int)) for key in fields),
                "GPU diagnostic adapter identity missing")
        self.report["gpu_diagnostics"] = {
            "adapter": {key: selected[key] for key in fields},
            "device_creation": doctor.get("device_creation"),
            "gpu_buffer_roundtrip": doctor.get("gpu_buffer_roundtrip"),
            "native_window_verified": False,
            "scope": "adapter diagnostic only; never substitutes for window captures",
        }
        data, runtime, artifacts = (self.work / name for name in ("data", "runtime", "artifacts"))
        common = [launcher, "--data-dir", data, "--manifest", MANIFEST, "--server", "127.0.0.1:25565"]
        for name, flag in (("bootstrap", "--headless-bootstrap"), ("server-content", "--singleplayer-runtime")):
            process = self.start(name, common + [flag])
            self.complete(name, process, 600)
        for label, tab in (("home", "play"), ("settings", "settings")):
            name = "launcher-" + label
            # Capture privately; copy only after complete PNG validation and exit 0.
            image = self.work / (name + ".png")
            process = self.start(name, common + ["--tab", tab, "--capture-window", image])
            self.complete(name, process, 120)
            receipt = png_receipt(image)
            (self.evidence / image.name).write_bytes(image.read_bytes())
            receipt.update(scene="normal launcher " + label, capture="own eframe/wgpu screenshot event",
                           display_scanout_verified=False)
            self.report["captures"].append(receipt)
        require(self.report["captures"][0]["sha256"] != self.report["captures"][1]["sha256"],
                "launcher Home and Settings captures are identical")
        self.native_ui(native, data, runtime, artifacts)
        self.report["status"] = "passed"
        self.report["visual_review_required"] = True

    def native_ui(self, native, data, runtime, artifacts):
        process = self.start("native-ui", [native, "play", "--windowed", "--automation", "--profile", "offline-test",
                                            "--data-dir", data, "--runtime-dir", runtime, "--artifact-dir", artifacts,
                                            "--mcp-http", "127.0.0.1:0"])
        require(process.ready.wait(120), "automation descriptor event deadline exceeded")
        descriptor = process.descriptor
        require(descriptor is not None and descriptor.is_absolute() and descriptor.resolve().is_relative_to(runtime),
                "automation descriptor event absent or outside isolated runtime")
        client = Client(descriptor, self.work / "mcp-private")
        try:
            endpoint = urllib.parse.urlsplit(client.endpoint)
            require(endpoint.scheme == "http" and endpoint.hostname == "127.0.0.1", "MCP endpoint is not loopback HTTP")
            initialized = client.initialize()
            require(initialized["result"]["protocolVersion"] == "2025-11-25", "unexpected MCP protocol")
            # Descriptor output only announces transport availability. Discover
            # the presentation-independent bootstrap wait, not graphical tools.
            bootstrap = client.request("tools/list", {})
            require(bootstrap and "result" in bootstrap, "MCP bootstrap discovery failed")
            wait_tool = next((entry for entry in bootstrap["result"]["tools"]
                              if entry["name"] == "minecraft_wait_for_state_change"), None)
            require(wait_tool is not None, "required startup state-change wait absent")
            wait_schema = wait_tool["inputSchema"]
            require(wait_schema.get("type") == "object" and wait_schema.get("additionalProperties") is False
                    and set(wait_schema.get("properties", {})) == {"observed_version", "timeout_seconds"}
                    and wait_schema["properties"]["observed_version"].get("type") == "integer"
                    and wait_schema["properties"]["timeout_seconds"].get("type") == "number"
                    and wait_schema.get("required") == ["observed_version"],
                    "startup state-change wait schema changed")
            # Version zero is the pre-observation baseline (Session::new_mode
            # initializes at one). This owner-queued causal barrier returns the
            # exact lifecycle snapshot, not descriptor readiness or a timer.
            # App::start_automation installs/publishes Title before the owner can
            # drain this request; lifecycle::Kind::State replies with current_screen.
            self.phase = "native-title-readiness"
            readiness = self.tool(client, "minecraft_wait_for_state_change",
                                  {"observed_version": 0, "timeout_seconds": 120.0})
            require(readiness.get("current_screen") == TITLE and readiness.get("in_play") is False
                    and readiness.get("disconnect_reason") == ""
                    and isinstance(readiness.get("state_version"), int) and readiness["state_version"] > 0,
                    "startup state-change snapshot did not confirm normal Title readiness")
            self.report["readiness"] = {
                "signal": "minecraft_wait_for_state_change owner-completed lifecycle snapshot",
                "state_version": readiness["state_version"], "current_screen": readiness["current_screen"],
                "in_play": False, "descriptor_is_readiness": False,
            }
            # Only now assert presentation-filtered discovery and exact Title UI.
            catalog = client.request("tools/list", {})
            require(catalog and "result" in catalog, "MCP discovery failed")
            tools = {entry["name"]: entry for entry in catalog["result"]["tools"]}
            expected = {"minecraft_observe": set(), "client_capture": {"name"},
                        "minecraft_click_screen_button": {"expected_screen_class", "expected_title", "button_label"}}
            for name, fields in expected.items():
                require(name in tools, "required MCP tool absent: " + name)
                schema = tools[name]["inputSchema"]
                require(schema.get("type") == "object" and schema.get("additionalProperties") is False
                        and set(schema.get("properties", {})) == fields and set(schema.get("required", [])) == fields
                        and all(schema["properties"][field].get("type") == "string" for field in fields),
                        "required MCP tool schema changed: " + name)
            self.save("discovery.json", {"protocol": "2025-11-25", "required_tools": [tools[name] for name in expected]})
            capabilities_reply = client.request("resources/read", {"uri": "solaris-client://capabilities"})
            require(capabilities_reply and "result" in capabilities_reply, "MCP capabilities discovery failed")
            capabilities = json.loads(capabilities_reply["result"]["contents"][0]["text"])
            instance = capabilities.get("instance_id")
            require(isinstance(instance, str) and re.fullmatch(r"[A-Za-z0-9_-]{1,128}", instance)
                    and instance == client.descriptor.get("instance_id")
                    and capabilities.get("profile") == "offline-test"
                    and capabilities.get("control_locally_authorized") is True,
                    "MCP instance/profile/control capabilities mismatch")
            self.report["instance_id"] = instance
            # Current canonical writes auto-acquire/renew their authenticated
            # transport session's lease (automation/control.rs::admit). Do not use
            # obsolete controller_acquire/client_action fields from older drivers.
            self.phase = "native-title"
            title = self.screen(client, TITLE)
            self.capture(client, "native-title", title, artifacts)
            self.click(client, title, "Options...")
            self.phase = "native-options"
            options = self.screen(client, OPTIONS)
            self.capture(client, "native-options", options, artifacts)
            require(self.report["captures"][-1]["png"]["sha256"] != self.report["captures"][-2]["png"]["sha256"],
                    "native title and Options captures are identical")
            self.click(client, options, "Done")
            self.phase = "native-quit"
            title = self.screen(client, TITLE)
            self.click(client, title, "Quit Game")
            # No release/disconnect request after Quit: the ordinary Shutdown
            # owns transport teardown. Missing reply/exit is a failure, not success.
            self.complete("native-ui", process, 30)
        finally:
            # Client.close() calls a legacy release tool only if acquire() was
            # used. No canonical lease token is ever copied into evidence.
            client.close()

    def tool(self, client, name, arguments=None):
        try:
            return client.tool(name, arguments)
        except Exception as error:
            # Keep exact safe owner failure codes without copying arbitrary MCP
            # payloads (which can contain session/credential material) to upload.
            payload = error.args[0] if error.args else None
            code = payload.get("structuredContent", {}).get("code") if isinstance(payload, dict) else None
            safe_codes = {"window_not_focused", "controller_busy", "controller_releasing", "lease_expired",
                          "stale_context", "controller_lease_revoked", "owner_result_unknown",
                          "capture_busy", "capture_target_unavailable", "command_queue_full"}
            detail = code if isinstance(code, str) and code in safe_codes else type(error).__name__
            raise GateError("MCP " + name + " failed: " + detail) from None

    def screen(self, client, expected_class):
        observed = self.tool(client, "minecraft_observe")
        screen = observed.get("screen", {})
        require(observed.get("in_play") is False and not observed.get("disconnect_reason"),
                "native normal startup unexpectedly joined or failed a session")
        require(screen.get("open") is True and screen.get("class") == expected_class
                and isinstance(screen.get("title"), str) and screen["title"], "expected normal UI screen absent")
        return {key: screen[key] for key in ("open", "class", "title")}

    def click(self, client, screen, label):
        result = self.tool(client, "minecraft_click_screen_button", {"expected_screen_class": screen["class"],
                             "expected_title": screen["title"], "button_label": label})
        require(result.get("status") == "ok", "ordinary UI button did not complete: " + label)
        self.log("ordinary UI button completed: " + label)

    def capture(self, client, name, screen, artifacts):
        receipt = self.tool(client, "client_capture", {"name": name})
        required = {"render_target": "actual_window_scene_target", "gpu_completion": True,
                    "offscreen_only": False, "present_requested": True, "display_scanout_verified": False}
        require(all(receipt.get(key) == value for key, value in required.items()), "capture is not completed own-window target")
        image = artifacts / (name + ".png")
        require(receipt.get("file_name") == image.name and Path(receipt.get("path", "")).resolve() == image.resolve(),
                "capture receipt image path mismatch")
        png = png_receipt(image)
        require((png["width"], png["height"]) == (receipt.get("width"), receipt.get("height")),
                "capture PNG dimensions disagree with GPU receipt")
        adapter = receipt.get("previous_render_metrics", {}).get("adapter", {})
        require(adapter.get("backend") == "Metal" and isinstance(adapter.get("name"), str),
                "actual window Metal backend identity missing")
        metadata = dict(required, screen=screen, png=png, instance_id=self.report["instance_id"],
                        adapter={key: adapter[key] for key in ("name", "backend", "vendor", "device", "device_type", "driver", "driver_info")})
        for key in ("frame_id", "state_revision", "connection_epoch", "dimension_epoch", "resource_epoch", "scene_revision"):
            value = receipt.get(key)
            require(isinstance(value, (str, int)) and str(value).isdigit(), "capture epoch/frame identity missing")
            metadata[key] = value
        (self.evidence / image.name).write_bytes(image.read_bytes())
        self.save(name + ".json", metadata)
        self.report["captures"].append(metadata)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--evidence-dir", type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    gate = Gate(args)
    # One overall wall bound in addition to individual pushed-event/exit bounds.
    def deadline(_signum, _frame):
        raise GateError("overall runtime gate deadline exceeded")
    signal.signal(signal.SIGALRM, deadline)
    signal.alarm(1800)
    gate.evidence.mkdir(parents=True, exist_ok=False, mode=0o700)
    try:
        gate.run()
    except Exception as error:
        gate.report["failure"] = {"phase": gate.phase, "kind": type(error).__name__,
                                  "reason": str(error) if isinstance(error, GateError) else "private diagnostic withheld; operation failed"}
        gate.log(gate.phase + ": FAILED (" + type(error).__name__ + ")")
    finally:
        signal.alarm(0)
        for process in gate.processes:
            try:
                process.stop_on_failure()
            except Exception as error:
                gate.report["status"] = "failed"
                gate.report.setdefault("cleanup_errors", []).append(type(error).__name__)
        gate.report["diagnostics"] = []
        for process in gate.processes:
            messages = sorted(process.diagnostics)
            if messages:
                gate.report["diagnostics"].append({"operation": process.operation, "observed_phrases": messages})
                for message in messages:
                    gate.log(process.operation + ": observed diagnostic: " + message)
        gate.save("report.json", gate.report)
    # Never print raw child errors/MCP payloads into public Actions logs.
    print("macOS packaged startup/UI gate: " + gate.report["status"])
    return 0 if gate.report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
