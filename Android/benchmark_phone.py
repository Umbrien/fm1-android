#!/usr/bin/env python3
"""Reproducible transport/provenance wrapper for the isolated Android performance test."""
import argparse
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile

APP = "org.fm1.android.benchmark"
TEST = APP + ".tests"
RUNNER = "org.fm1.android.EngineInstrumentation"
ROOT = Path(__file__).resolve().parent.parent


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def extract_receipt(output):
    """Decode complete JSON objects from am-instrument output, including failed checks."""
    decoder = json.JSONDecoder()
    receipts = []
    for match in re.finditer(r"\{", output):
        try:
            value, _ = decoder.raw_decode(output[match.start():])
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict) and value.get("platform") == "Android" and isinstance(value.get("results"), list):
            receipts.append(value)
    return receipts[-1] if receipts else None


def validate_receipt(receipt):
    if not isinstance(receipt, dict) or receipt.get("failures") != 0:
        return False
    results = receipt.get("results", [])
    return (isinstance(results, list) and len(results) == 1 and isinstance(results[0], dict)
            and results[0].get("name") == "physical-fm1va-continuity"
            and results[0].get("passed") is True)


def validate_performance(value, duration_seconds=30):
    """Independently reject inactive/slow playback even if old instrumentation says pass."""
    validation = {"passed": False, "reasons": [], "toleranceFrames": 4096, "deltas": {}}
    reasons = validation["reasons"]
    if not isinstance(value, dict) or not isinstance(value.get("before"), dict) or not isinstance(value.get("after"), dict):
        reasons.append("Missing before/after diagnostics objects")
        return validation
    before, after = value["before"], value["after"]
    elapsed = value.get("elapsedMs")
    elapsed_valid = (isinstance(elapsed, int) and not isinstance(elapsed, bool)
                     or isinstance(elapsed, float) and math.isfinite(elapsed)) and elapsed >= duration_seconds * 1000
    if not elapsed_valid:
        reasons.append(f"Measurement duration must be at least {duration_seconds * 1000} ms")
    intervals = duration_seconds // 5
    if not isinstance(value.get("intervals"), list) or len(value["intervals"]) != intervals:
        reasons.append(f"Expected {intervals} measurement intervals")
    if before.get("running") is not True or after.get("running") is not True:
        reasons.append("Engine must be running before and after measurement; unlock the phone if necessary")
    generation = before.get("generation")
    if (type(generation) is not int or generation < 0 or type(after.get("generation")) is not int
            or generation != after.get("generation")):
        reasons.append("Output generation changed or is unavailable")
    rate = before.get("sampleRate")
    rate_valid = type(rate) is int and rate > 0 and type(after.get("sampleRate")) is int and rate == after.get("sampleRate")
    if not rate_valid:
        reasons.append("Sample rate changed or is unavailable")
    for key in ("producedFrames", "acceptedGeneratedFrames", "playbackHeadFramesSinceFlush", "underflowFrames", "osUnderruns"):
        a, b = before.get(key), after.get(key)
        if type(a) is not int or type(b) is not int or a < 0 or b < 0:
            reasons.append(f"Missing or invalid frame/underrun counter: {key}")
            continue
        delta = (b - a) & 0xffffffff if key == "playbackHeadFramesSinceFlush" else b - a
        validation["deltas"][key] = delta
        if key in ("underflowFrames", "osUnderruns") and delta != 0:
            reasons.append(f"Steady-playback deficit: {key} delta={delta}")
    input_before, input_after = before.get("inputQueue"), after.get("inputQueue")
    input_counts = [value.get("count") if isinstance(value, dict) else None for value in (input_before, input_after)]
    if any(type(value) is not int or value < 0 for value in input_counts):
        reasons.append("Missing applied-input counters for the four-note workload")
    else:
        edges = input_counts[1] - input_counts[0]
        validation["deltas"]["appliedInputEdges"] = edges
        if edges < intervals * 8:
            reasons.append(f"Four-note workload incomplete: applied key edges={edges}, minimum={intervals * 8}")
    if elapsed_valid and rate_valid:
        minimum = elapsed * rate / 1000 - 4096
        maximum_played = elapsed * rate / 1000 + 4096
        validation["minimumProgressFrames"] = minimum
        validation["maximumPlayedFrames"] = maximum_played
        for key in ("producedFrames", "acceptedGeneratedFrames", "playbackHeadFramesSinceFlush"):
            delta = validation["deltas"].get(key)
            if delta is not None and delta < minimum:
                reasons.append(f"Insufficient sustained progress: {key} delta={delta}, minimum={minimum}")
        played = validation["deltas"].get("playbackHeadFramesSinceFlush")
        if played is not None and played > maximum_played:
            reasons.append(f"Implausible playback-head progress: delta={played}, maximum={maximum_played}; possible head reset")
    validation["passed"] = not reasons
    return validation


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("serial", help="Required exact adb serial; no automatic device selection")
    parser.add_argument("--duration-seconds", type=int, default=30,
                        help="Measurement duration, 30–600 seconds in five-second intervals (default: 30)")
    parser.add_argument("--prepared-build", type=Path, default=ROOT / "Android/build",
                        help="Prepared assets/engines, assets/notices and lib/arm64-v8a/libfm1android.so (default: Android/build)")
    args = parser.parse_args(argv)
    if not args.serial or any(character.isspace() for character in args.serial):
        parser.error("serial must be a nonempty adb serial without whitespace")
    if args.duration_seconds not in range(30, 601, 5):
        parser.error("duration must be 30–600 seconds in five-second intervals")
    base = ROOT / "Android/build/phone-verification"
    base.mkdir(parents=True, exist_ok=True)
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    out = Path(tempfile.mkdtemp(prefix=stamp + "-", dir=base))
    receipt = {"schemaVersion": 1, "startedUtc": stamp, "status": "incomplete", "phase": "prepare",
               "adbSelectedSerial": args.serial, "applicationId": APP, "testApplicationId": TEST,
               "preparedBuild": str(args.prepared_build.resolve()),
               "requestedDurationSeconds": args.duration_seconds,
               "nativeProvenance": "Prebuilt JNI identified by SHA-256; current source revision does not prove its build origin.",
               "benchmarkState": "Isolated benchmark saves retained; only prior receipt files removed."}

    def run(command, log=None, check=True, binary=False):
        if log:
            with (out / log).open("w") as stream:
                result = subprocess.run([str(part) for part in command], cwd=ROOT,
                                        stdout=stream, stderr=subprocess.STDOUT, text=not binary)
        else:
            result = subprocess.run([str(part) for part in command], cwd=ROOT,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=not binary)
        if check and result.returncode:
            detail = f"; see {log}" if log else f": {result.stderr.strip()}"
            raise RuntimeError(f"Command failed ({result.returncode}): {' '.join(map(str, command))}{detail}")
        return result

    try:
        sdk = Path(os.environ.get("ANDROID_HOME", str(Path.home() / "Library/Android/sdk")))
        tools = sdk / "build-tools/35.0.0"
        adb = Path(os.environ.get("ADB", str(sdk / "platform-tools/adb")))
        for tool in (adb, tools / "aapt", tools / "apksigner", ROOT / "Android/gradlew"):
            if not tool.is_file():
                raise RuntimeError(f"Required tool missing: {tool}. Configure ANDROID_HOME/ADB and the documented toolchain.")
        receipt["source"] = {
            "head": run(["git", "rev-parse", "HEAD"]).stdout.strip(),
            "status": run(["git", "status", "--porcelain=v1"]).stdout.splitlines(),
            "trackedDiffSha256": hashlib.sha256(run(["git", "diff", "HEAD", "--binary"]).stdout.encode()).hexdigest(),
            "helperSha256": sha(__file__),
            "launcherSha256": sha(ROOT / "Android/benchmark-phone.sh"),
        }
        prepared = args.prepared_build.resolve()
        expected = json.loads((ROOT / "Core/native/packages.lock.json").read_text())["packages"]
        native = prepared / "lib/arm64-v8a/libfm1android.so"
        if not native.is_file():
            raise RuntimeError(f"Prepared JNI library missing: {native}. Build native code and assets first with Android/build.sh --core.")
        payloads = {"lib/arm64-v8a/libfm1android.so": {"sha256": sha(native), "bytes": native.stat().st_size}}
        for package in expected:
            relative = f"assets/engines/{package['kind']}.fwsc"
            path = prepared / relative
            if not path.is_file() or path.stat().st_size != package["size"] or sha(path) != package["sha256"]:
                raise RuntimeError(f"Prepared firmware missing or differs from package lock: {path}. Run Android/build.sh --core first.")
            payloads[relative] = {"sha256": package["sha256"], "bytes": package["size"], "id": package["id"]}
        if not (prepared / "assets/notices").is_dir() or not any((prepared / "assets/notices").iterdir()):
            raise RuntimeError(f"Prepared notices missing: {prepared / 'assets/notices'}")
        receipt["preparedPayloads"] = payloads
        destination = ROOT / "Android/build"
        if prepared != destination.resolve():
            for relative in payloads:
                target = destination / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(prepared / relative, target)
            shutil.copytree(prepared / "assets/notices", destination / "assets/notices", dirs_exist_ok=True)
        receipt["phase"] = "build"
        command = [ROOT / "Android/gradlew", "-p", ROOT / "Android", "-Pfm1Performance", "--console=plain",
                   "assembleBenchmark", "assembleBenchmarkAndroidTest"]
        receipt["buildCommand"] = list(map(str, command))
        print(f"Building isolated benchmark; receipts: {out}", flush=True)
        run(command, "build.txt")
        receipt["phase"] = "sign-and-validate"
        apks = {
            APP: (ROOT / "Android/app/build/outputs/apk/benchmark/app-benchmark-unsigned.apk", out / "benchmark.apk"),
            TEST: (ROOT / "Android/app/build/outputs/apk/androidTest/benchmark/app-benchmark-androidTest.apk", out / "benchmark-tests.apk"),
        }
        receipt["apks"] = {}
        for identity, (unsigned, signed) in apks.items():
            if not unsigned.is_file():
                raise RuntimeError(f"Gradle did not produce expected APK: {unsigned}")
            run([ROOT / "Android/signing.sh", unsigned, signed], f"sign-{identity}.txt")
            run([tools / "apksigner", "verify", "--verbose", "--print-certs", signed], f"signature-{identity}.txt")
            manifest = run([tools / "aapt", "dump", "xmltree", signed, "AndroidManifest.xml"]).stdout
            (out / f"manifest-{identity}.txt").write_text(manifest)
            actual = re.search(r'\bA: package="([^"]+)"', manifest)
            if not actual or actual.group(1) != identity:
                raise RuntimeError(f"Refusing installation: APK package differs from isolated identity {identity}")
            if identity == TEST:
                target = re.search(r'android:targetPackage\([^)]*\)="([^"]+)"', manifest)
                runner = re.search(r'android:name\([^)]*\)="([^"]+)"', manifest)
                if not target or target.group(1) != APP or not runner or runner.group(1) != RUNNER:
                    raise RuntimeError("Refusing installation: instrumentation target/runner differs from isolated benchmark")
            receipt["apks"][identity] = {"file": signed.name, "sha256": sha(signed), "bytes": signed.stat().st_size}
        with zipfile.ZipFile(apks[APP][1]) as apk:
            for relative, expected_payload in payloads.items():
                if hashlib.sha256(apk.read(relative)).hexdigest() != expected_payload["sha256"]:
                    raise RuntimeError(f"Packaged APK payload differs from prepared input: {relative}")
        receipt["phase"] = "device"
        device_command = [adb, "-s", args.serial]
        def device(*arguments, **kwargs):
            return run(device_command + list(arguments), **kwargs)
        state = device("get-state", check=False)
        if state.returncode or state.stdout.strip() != "device":
            raise RuntimeError(f"Explicit device is unavailable or unauthorized: {args.serial}; {state.stderr.strip()}")
        metadata = {"adbReportedSerial": device("get-serialno").stdout.strip()}
        properties = ["ro.serialno", "ro.product.model", "ro.product.manufacturer", "ro.product.device",
                      "ro.product.board", "ro.build.fingerprint", "ro.build.version.sdk", "ro.product.cpu.abilist", "ro.kernel.qemu"]
        for property_name in properties:
            metadata[property_name] = device("shell", "getprop", property_name).stdout.strip()
        metadata["kernelPageSize"] = device("shell", "getconf", "PAGESIZE").stdout.strip()
        receipt["device"] = metadata
        (out / "device.json").write_text(json.dumps(metadata, indent=2) + "\n")
        if metadata["ro.kernel.qemu"] == "1" or args.serial.startswith("emulator-"):
            raise RuntimeError("This helper requires a physical phone; emulator results must be recorded separately.")
        if "arm64-v8a" not in metadata["ro.product.cpu.abilist"].split(","):
            raise RuntimeError("The selected phone does not report arm64-v8a support.")
        receipt["phase"] = "install-isolated"
        for identity, (_, signed) in apks.items():
            device("install", "--no-incremental", "-r", signed, log=f"install-{identity}.txt")
        device("shell", "am", "force-stop", APP)
        def benchmark_state():
            state = {}
            for relative in ("files/Instruments/12.flash", "shared_prefs/fm1.xml"):
                captured = device("exec-out", "run-as", APP, "cat", relative, check=False, binary=True)
                state[relative] = ({"sha256": hashlib.sha256(captured.stdout).hexdigest(), "bytes": len(captured.stdout)}
                                   if captured.returncode == 0 else {"unavailable": captured.stderr.decode(errors="replace").strip()})
            return state
        receipt["benchmarkStateBefore"] = benchmark_state()
        # Constant package and paths only: never clear or access the owner's application data.
        device("shell", "run-as", APP, "rm", "-f", "files/performance.json", "files/instrumentation.json")
        receipt["phase"] = "wake-phone"
        device("shell", "input", "keyevent", "KEYCODE_WAKEUP", log="wake.txt")
        device("shell", "wm", "dismiss-keyguard", log="dismiss-keyguard.txt")
        receipt["wakeRequested"] = True
        receipt["phase"] = "instrumentation"
        result = device("shell", "am", "instrument", "-w", "-e", "performanceOnly", "true",
                        "-e", "performanceSeconds", str(args.duration_seconds), TEST + "/" + RUNNER,
                        log="instrumentation.txt", check=False)
        receipt["adbInstrumentationExitCode"] = result.returncode
        output = (out / "instrumentation.txt").read_text(errors="replace")
        parsed = extract_receipt(output)
        valid_performance = False
        for name in ("performance.json", "instrumentation.json"):
            pulled = device("exec-out", "run-as", APP, "cat", "files/" + name, check=False)
            if pulled.returncode == 0:
                (out / name).write_text(pulled.stdout)
                try:
                    value = json.loads(pulled.stdout)
                    if name == "instrumentation.json":
                        parsed = value
                    else:
                        receipt["continuityValidation"] = validate_performance(value, args.duration_seconds)
                        valid_performance = receipt["continuityValidation"]["passed"]
                except json.JSONDecodeError:
                    receipt.setdefault("retrievalErrors", {})[name] = "Device returned invalid JSON; raw file retained."
            else:
                receipt.setdefault("retrievalErrors", {})[name] = pulled.stderr.strip()
        if parsed is not None:
            (out / "instrumentation.json").write_text(json.dumps(parsed, indent=2) + "\n")
            receipt["instrumentation"] = parsed
        receipt["benchmarkStateAfter"] = benchmark_state()
        passed = result.returncode == 0 and validate_receipt(parsed) and valid_performance
        if not passed:
            raise RuntimeError("Physical continuity failed or its complete receipts are unavailable; see instrumentation.txt and continuityValidation. Unlock the phone if it remains securely locked, then rerun.")
        receipt["status"] = "passed"
        receipt["phase"] = "complete"
        return_code = 0
    except Exception as error:
        receipt["status"] = "failed"
        receipt["errorType"] = type(error).__name__
        receipt["error"] = str(error)
        print(str(error), file=sys.stderr)
        return_code = 1
    finally:
        receipt["finishedUtc"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        (out / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
        print(f"{receipt['status']}: {out / 'receipt.json'}", flush=True)
    return return_code


if __name__ == "__main__":
    sys.exit(main())
