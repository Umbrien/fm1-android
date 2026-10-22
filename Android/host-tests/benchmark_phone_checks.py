"""No device access: emulate subprocess transport to test failure retention and install guards."""
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import zipfile

spec = importlib.util.spec_from_file_location("benchmark_phone", Path(__file__).resolve().parents[1] / "benchmark_phone.py")
helper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helper)


class PhoneHelperChecks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="fm1-phone-helper-test-")
        self.root = Path(self.temp.name)
        self.calls = []
        self.wrong_package = False
        self.instrumentation = {"platform": "Android", "failures": 1, "results": [
            {"name": "physical-fm1va-continuity", "passed": False, "error": "host missing=512 frames"}]}
        self.performance = {"elapsedMs": 30000, "before": {}, "after": {"underflowFrames": 512}, "intervals": [{}] * 6}
        native = b"synthetic JNI test fixture"
        firmware = b"synthetic firmware test fixture"
        prepared = self.root / "Android/build"
        for path, data in {"lib/arm64-v8a/libfm1android.so": native, "assets/engines/12.fwsc": firmware,
                           "assets/notices/NOTICE": b"test"}.items():
            file = prepared / path
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_bytes(data)
        lock = self.root / "Core/native/packages.lock.json"
        lock.parent.mkdir(parents=True)
        lock.write_text(json.dumps({"packages": [{"kind": 12, "id": "test", "size": len(firmware),
                                                  "sha256": helper.sha(prepared / "assets/engines/12.fwsc")}]}))
        for tool in ("Android/gradlew", "Android/benchmark-phone.sh", "sdk/platform-tools/adb",
                     "sdk/build-tools/35.0.0/aapt", "sdk/build-tools/35.0.0/apksigner"):
            file = self.root / tool
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_text("synthetic tool fixture")
        for name in ("benchmark/app-benchmark-unsigned.apk", "androidTest/benchmark/app-benchmark-androidTest.apk"):
            file = self.root / "Android/app/build/outputs/apk" / name
            file.parent.mkdir(parents=True, exist_ok=True)
            with zipfile.ZipFile(file, "w") as archive:
                archive.writestr("lib/arm64-v8a/libfm1android.so", native)
                archive.writestr("assets/engines/12.fwsc", firmware)

    def tearDown(self):
        self.temp.cleanup()

    def fake_run(self, command, **kwargs):
        self.calls.append(command)
        output = ""
        if command[:2] == ["git", "rev-parse"]:
            output = "a" * 40
        elif command[0].endswith("signing.sh"):
            shutil.copy2(command[1], command[2])
        elif command[0].endswith("/aapt"):
            identity = helper.TEST if command[3].endswith("benchmark-tests.apk") else helper.APP
            if self.wrong_package:
                identity = "org.fm1.android"
            output = (f'A: package="{identity}"\nA: android:targetPackage(0x1)="{helper.APP}"\n'
                      f'A: android:name(0x2)="{helper.RUNNER}"\n')
        elif command[0].endswith("/adb"):
            self.assertEqual(command[1:3], ["-s", "selected-identity"])
            tail = command[3:]
            if tail == ["get-state"]:
                output = "device\n"
            elif tail == ["get-serialno"]:
                output = "selected-identity\n"
            elif tail[:2] == ["shell", "getprop"]:
                output = {"ro.product.cpu.abilist": "arm64-v8a,armeabi-v7a", "ro.product.model": "Reported Model",
                          "ro.kernel.qemu": "0", "ro.serialno": "hardware-identity"}.get(tail[-1], "test")
            elif tail[:3] == ["shell", "getconf", "PAGESIZE"]:
                output = "4096\n"
            elif tail[:3] == ["shell", "am", "instrument"]:
                output = "INSTRUMENTATION_RESULT: stream=" + json.dumps(self.instrumentation) + "\nINSTRUMENTATION_CODE: 0\n"
            elif tail[:3] == ["exec-out", "run-as", helper.APP]:
                output = (json.dumps(self.performance) if tail[-1] == "files/performance.json" else
                          json.dumps(self.instrumentation) if tail[-1] == "files/instrumentation.json" else "benchmark-state")
            # All package-directed commands use the isolated constant, never the owner's package.
            self.assertNotIn("org.fm1.android", tail)
        if kwargs.get("stdout") is not subprocess.PIPE:
            kwargs["stdout"].write(output)
            output = None
        elif not kwargs.get("text", False):
            output = output.encode()
        error = "" if kwargs.get("text", False) else b""
        return subprocess.CompletedProcess(command, 0, output, error)

    def invoke(self):
        with patch.object(helper, "ROOT", self.root), patch.dict(os.environ, {"ANDROID_HOME": str(self.root / "sdk")}):
            with patch.object(helper.subprocess, "run", self.fake_run), patch("sys.stdout", io.StringIO()), patch("sys.stderr", io.StringIO()):
                return helper.main(["selected-identity"])

    def test_adb_success_does_not_hide_assertion_failure_and_json_survives(self):
        self.assertEqual(self.invoke(), 1)
        directory = next((self.root / "Android/build/phone-verification").iterdir())
        receipt = json.loads((directory / "receipt.json").read_text())
        self.assertEqual(receipt["status"], "failed")
        self.assertEqual(receipt["adbInstrumentationExitCode"], 0)
        self.assertEqual(json.loads((directory / "performance.json").read_text()), self.performance)
        self.assertEqual(json.loads((directory / "instrumentation.json").read_text()), self.instrumentation)
        self.assertEqual(receipt["adbSelectedSerial"], "selected-identity")
        self.assertEqual(receipt["device"]["ro.product.model"], "Reported Model")

    def test_wrong_apk_identity_stops_before_any_device_access(self):
        self.wrong_package = True
        self.assertEqual(self.invoke(), 1)
        self.assertFalse(any(command[0].endswith("/adb") for command in self.calls))

    def test_only_complete_single_performance_pass_is_success(self):
        self.assertFalse(helper.validate_receipt(None))
        self.assertFalse(helper.validate_receipt(self.instrumentation))
        self.assertFalse(helper.validate_receipt({"failures": 0, "results": [None]}))
        valid = {"platform": "Android", "failures": 0, "results": [{"name": "physical-fm1va-continuity", "passed": True}]}
        self.assertTrue(helper.validate_receipt(valid))
        self.assertEqual(helper.extract_receipt("prefix " + json.dumps(valid) + "\n trailing {broken"), valid)

    def test_inactive_zero_underrun_receipt_cannot_false_pass(self):
        self.instrumentation["failures"] = 0
        self.instrumentation["results"][0]["passed"] = True
        before = {"running": False, "generation": 6, "producedFrames": 512, "acceptedGeneratedFrames": 0,
                  "playbackHeadFramesSinceFlush": 132300, "sampleRate": 44100, "underflowFrames": 9216, "osUnderruns": 0}
        after = dict(before, playbackHeadFramesSinceFlush=1455300)
        self.performance = {"elapsedMs": 30016, "before": before, "after": after, "intervals": [{}] * 6}
        self.assertEqual(self.invoke(), 1)
        directory = next((self.root / "Android/build/phone-verification").iterdir())
        receipt = json.loads((directory / "receipt.json").read_text())
        self.assertFalse(receipt["continuityValidation"]["passed"])
        self.assertTrue(any("running" in reason for reason in receipt["continuityValidation"]["reasons"]))
        self.assertTrue(any("producedFrames" in reason for reason in receipt["continuityValidation"]["reasons"]))
        self.assertEqual(json.loads((directory / "performance.json").read_text()), self.performance)

    def test_sustained_progress_passes_and_phone_is_woken_before_instrumentation(self):
        self.instrumentation["failures"] = 0
        self.instrumentation["results"][0]["passed"] = True
        before = {"running": True, "generation": 6, "producedFrames": 5000, "acceptedGeneratedFrames": 4500,
                  "playbackHeadFramesSinceFlush": 4000, "sampleRate": 44100, "underflowFrames": 0, "osUnderruns": 0,
                  "inputQueue": {"count": 0}}
        after = dict(before, inputQueue={"count": 48})
        for key in ("producedFrames", "acceptedGeneratedFrames", "playbackHeadFramesSinceFlush"):
            after[key] += 1323000
        self.performance = {"elapsedMs": 30000, "before": before, "after": after, "intervals": [{}] * 6}
        self.assertEqual(self.invoke(), 0)
        commands = [command[3:] for command in self.calls if command[0].endswith("/adb")]
        wake = commands.index(["shell", "input", "keyevent", "KEYCODE_WAKEUP"])
        dismiss = commands.index(["shell", "wm", "dismiss-keyguard"])
        instrument = next(index for index, command in enumerate(commands) if command[:3] == ["shell", "am", "instrument"])
        self.assertLess(wake, dismiss)
        self.assertLess(dismiss, instrument)
        installs = [command for command in commands if command[0] == "install"]
        self.assertEqual(len(installs), 2)
        self.assertTrue(all("--no-incremental" in command for command in installs))
        for changed in (dict(self.performance, elapsedMs=29999),
                        dict(self.performance, after=dict(after, generation=7)),
                        dict(self.performance, after=dict(after, acceptedGeneratedFrames=4500)),
                        dict(self.performance, after=dict(after, playbackHeadFramesSinceFlush=4000)),
                        dict(self.performance, after=dict(after, playbackHeadFramesSinceFlush=3000)),
                        dict(self.performance, after=dict(after, inputQueue={"count": 47})),
                        dict(self.performance, after=dict(after, underflowFrames=1)),
                        dict(self.performance, after=dict(after, osUnderruns=1))):
            self.assertFalse(helper.validate_performance(changed)["passed"])
        # A genuine unsigned 32-bit wrap still represents normal progress and must pass.
        wrapped_before = dict(before, playbackHeadFramesSinceFlush=0xffffffff - 500000)
        wrapped_after = dict(after, playbackHeadFramesSinceFlush=(wrapped_before["playbackHeadFramesSinceFlush"] + 1323000) & 0xffffffff)
        self.assertTrue(helper.validate_performance(dict(self.performance, before=wrapped_before, after=wrapped_after))["passed"])
        # A short successful receipt cannot satisfy a requested five-minute run.
        self.assertFalse(helper.validate_performance(self.performance, 300)["passed"])
        long_after = dict(before, inputQueue={"count": 480})
        for key in ("producedFrames", "acceptedGeneratedFrames", "playbackHeadFramesSinceFlush"):
            long_after[key] += 44100 * 300
        long_run = dict(self.performance, elapsedMs=300000, after=long_after, intervals=[{}] * 60)
        self.assertTrue(helper.validate_performance(long_run, 300)["passed"])
        self.assertFalse(helper.validate_performance(dict(long_run, after=dict(long_after, inputQueue={"count": 48})), 300)["passed"])


if __name__ == "__main__":
    unittest.main()
