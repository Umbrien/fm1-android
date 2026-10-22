package org.fm1.android

import android.app.Instrumentation
import android.content.Intent
import android.graphics.RectF
import android.media.AudioTrack
import android.os.Bundle
import android.os.Handler
import android.os.SystemClock
import android.view.InputDevice
import android.view.MotionEvent
import java.io.*
import java.util.Locale
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.math.*
import org.json.*

/** SDK-only device checks; every native handle remains on this one worker. */
class EngineInstrumentation : Instrumentation() {
    private val results = JSONArray()
    private var failures = 0
    private var compilerOnly = false
    private var uiOnly = false
    private var exchangeTests = false
    private var performanceOnly = false
    private var performanceSeconds = 30

    override fun onCreate(arguments: Bundle?) {
        super.onCreate(arguments)
        compilerOnly = arguments?.getString("compilerOnly") == "true"
        uiOnly = arguments?.getString("uiOnly") == "true"
        exchangeTests = arguments?.getString("exchange") == "true"
        performanceOnly = arguments?.getString("performanceOnly") == "true"
        performanceSeconds = arguments?.getString("performanceSeconds")?.toIntOrNull() ?: 30
        Thread(::runChecks, "fm1-instrumentation").start()
    }

    private fun check(name: String, test: () -> JSONObject) {
        val r = JSONObject()
        try {
            r.put("name", name).put("metrics", test()).put("passed", true)
        } catch (error: Throwable) {
            failures++
            r.put("name", name).put("passed", false).put("error", error.toString())
        }
        results.put(r)
        val status = Bundle()
        status.putString("stream", r.toString() + "\n")
        sendStatus(if (failures == 0) 0 else -1, status)
    }

    private fun require(value: Boolean, message: String) {
        if (!value) throw AssertionError(message)
    }

    private fun asset(kind: Int) =
        targetContext.assets.open("engines/$kind.fwsc").use { it.readBytes() }

    private fun advance(m: Long, frames: Int): Double {
        var remaining = frames
        val pcm = FloatArray(1024)
        var peak = 0.0
        while (remaining > 0) {
            val n = min(512, remaining)
            require(NativeEngine.render(m, pcm, n) == n, "incomplete native render")
            for (i in 0 until n * 2) {
                require(pcm[i].isFinite(), "nonfinite PCM")
                peak = max(peak, abs(pcm[i]).toDouble())
            }
            remaining -= n
        }
        return peak
    }

    private fun tap(m: Long, key: Int) {
        NativeEngine.key(m, key, true)
        advance(m, 6615)
        NativeEngine.key(m, key, false)
        advance(m, 1024)
    }

    private fun engine(kind: Int): JSONObject {
        val bytes = asset(kind)
        val m = NativeEngine.create(bytes, kind)
        try {
            require(m != 0L && NativeEngine.kind(m) == kind, "wrong compiled native kind")
            NativeEngine.volume(m, 716)
            advance(m, 23040)
            val key = if (kind == 4) 15 else 21
            NativeEngine.key(m, key, true)
            val peak = advance(m, 44544)
            require(peak > 0.00001, "silent note PCM")
            NativeEngine.key(m, key, false)
            NativeEngine.release(m)
            advance(m, 4096)
            val pixels = IntArray(57600)
            val leds = FloatArray(41)
            require(NativeEngine.snapshot(m, pixels, leds) != 0, "snapshot failed")
            val colors = pixels.count { it != pixels[0] }
            require(colors > 20, "blank LCD")
            for (led in leds) require(led.isFinite(), "nonfinite LED")
            val flash = NativeEngine.flash(m)
            require(flash.size == if (kind == 6) 536576 else 1048576, "wrong flash size")
            flash[flash.lastIndex] = (flash.last().toInt() xor 0x25).toByte()
            val candidate = NativeEngine.create(bytes, kind)
            try {
                require(
                    !NativeEngine.restore(candidate, flash.copyOf(flash.size - 1)),
                    "accepted truncated flash",
                )
                require(NativeEngine.restore(candidate, flash), "flash restore failed")
                require(
                    flash.contentEquals(NativeEngine.flash(candidate)),
                    "edited flash roundtrip differs",
                )
                NativeEngine.key(candidate, key, true)
                advance(candidate, 512)
                NativeEngine.release(candidate)
            } finally {
                NativeEngine.destroy(candidate)
            }
            return JSONObject()
                .put("kind", kind)
                .put("packageSHA256", ProjectSnapshot.digest(bytes))
                .put("peak", peak)
                .put("nonbackgroundPixels", colors)
                .put("flashBytes", flash.size)
        } finally {
            NativeEngine.destroy(m)
        }
    }

    private fun interpretedEngine(kind: Int): JSONObject {
        val m = NativeEngine.create(asset(kind), kind)
        try {
            require(NativeEngine.kind(m) == kind, "stock identity mismatch")
            val pcm = FloatArray(1024)
            var frames = 0L
            fun step(): Double {
                val n = NativeEngine.render(m, pcm, 512)
                require(n in 0..512, "stock returned invalid frame count")
                frames += n
                var peak = 0.0
                for (i in 0 until n * 2) {
                    require(pcm[i].isFinite(), "nonfinite stock PCM")
                    peak = max(peak, abs(pcm[i]).toDouble())
                }
                return peak
            }
            val start = SystemClock.elapsedRealtime()
            repeat(200) { step() }
            NativeEngine.key(m, 21, true)
            var peak = 0.0
            repeat(100) { peak = max(peak, step()) }
            NativeEngine.release(m)
            require(peak > 0.00001, "silent stock note")
            val pixels = IntArray(57600)
            val leds = FloatArray(41)
            require(NativeEngine.snapshot(m, pixels, leds) != 0, "stock snapshot failed")
            require(pixels.count { it != pixels[0] } > 20, "blank stock LCD")
            require(NativeEngine.flash(m).size == 1048576, "wrong stock flash size")
            val elapsed = SystemClock.elapsedRealtime() - start
            return JSONObject().put("kind", kind).put("backend", NativeEngine.backend(m)).put("compiledInstructions", NativeEngine.jitInstructions(m))
                .put("frames", frames).put("elapsedMs", elapsed).put("peak", peak)
                .also { if (kind == 12) it.put("soundModes", fm1vaSoundModes()) }
        } finally { NativeEngine.destroy(m) }
    }

    private fun compiledFirmware(): JSONObject {
        val runs = JSONArray()
        // Exercise original packages through the binary path, including two source-native
        // catalogue entries. No catalogue-based dispatch can satisfy these checks.
        for (kind in intArrayOf(11, 12, 3, 4)) {
            val bytes = asset(kind)
            val compiled = NativeEngine.createBinary(bytes, true)
            val reference = NativeEngine.createBinary(bytes, false)
            try {
                require(NativeEngine.kind(compiled) == 13, "import did not use binary engine")
                val a = FloatArray(1024)
                val b = FloatArray(1024)
                var compiledNs = 0L
                var referenceNs = 0L
                var samples = 0L
                repeat(220) { step ->
                    if (step == 180) for (m in longArrayOf(compiled, reference)) NativeEngine.key(m, 21, true)
                    var start = System.nanoTime()
                    val na = NativeEngine.render(compiled, a, 512)
                    compiledNs += System.nanoTime() - start
                    start = System.nanoTime()
                    val nb = NativeEngine.render(reference, b, 512)
                    referenceNs += System.nanoTime() - start
                    require(na == nb && a.contentEquals(b), "kind $kind PCM mismatch at $step")
                    samples += na
                }
                require(NativeEngine.backend(compiled) == 3 && NativeEngine.jitInstructions(compiled) > 0,
                    "ARM64 code did not execute")
                require(NativeEngine.backend(reference) == 1 && NativeEngine.jitInstructions(reference) == 0L,
                    "reference compiled unexpectedly")
                require(NativeEngine.flash(compiled).contentEquals(NativeEngine.flash(reference)), "flash mismatch")
                val pa = IntArray(57600); val pb = IntArray(57600)
                NativeEngine.snapshot(compiled, pa, FloatArray(41))
                NativeEngine.snapshot(reference, pb, FloatArray(41))
                require(pa.contentEquals(pb), "LCD mismatch")
                runs.put(JSONObject().put("kind", kind).put("frames", samples)
                    .put("compiledMs", compiledNs / 1e6).put("referenceMs", referenceNs / 1e6)
                    .put("compiledInstructions", NativeEngine.jitInstructions(compiled)))
            } finally { NativeEngine.destroy(compiled); NativeEngine.destroy(reference) }
        }
        // Package validation, identity-based storage, corruption detection, and reload.
        val library = FirmwareLibrary(targetContext)
        val bytes = asset(11)
        val first = library.identify(bytes, "../../user firmware.fwsc")
        library.store(first, bytes)
        require(library.bytes(first.id).contentEquals(bytes), "import package reload mismatch")
        require(library.identify(bytes, "renamed.fwsc").id == first.id, "filename changed identity")
        val second = library.identify(asset(12), first.name)
        require(second.id != first.id, "different packages share an identity")
        var rejected = false
        try { NativeEngine.createBinary(ByteArray(1000), true).also { NativeEngine.destroy(it) } }
        catch (_: IllegalStateException) { rejected = true }
        require(rejected, "malformed firmware accepted")
        rejected = false
        try { library.bytes("../outside") } catch (_: IOException) { rejected = true }
        require(rejected, "invalid library identity accepted")
        // Exercise the same URI import, switch, failure recovery, reset and reopen path as UI.
        val fixture = File(targetContext.cacheDir, "compiler-import.fwsc").apply { writeBytes(bytes) }
        lateinit var controller: EngineController
        runOnMainSync { controller = EngineController(targetContext); controller.setPaused(true) }
        fun onWorker(action: () -> Unit) {
            val done = CountDownLatch(1); var failure: Throwable? = null
            (field(controller, "worker") as Handler).post {
                try { action() } catch (e: Throwable) { failure = e } finally { done.countDown() }
            }
            require(done.await(20, TimeUnit.SECONDS), "import worker timed out")
            failure?.let { throw it }
        }
        try {
            barrier(controller)
            controller.importFirmware(android.net.Uri.fromFile(fixture), "User stock.fwsc")
            barrier(controller)
            require(controller.selectedKind == 13 && controller.selectedFirmwareId == first.id,
                "URI import failed: ${controller.status}")
            require(controller.compiledInstructions > 0, "import had no compiled execution")
            var marker = 0.toByte()
            onWorker {
                val handle = field(controller, "machine") as Long
                val flash = NativeEngine.flash(handle)
                marker = (flash.last().toInt() xor 0x25).toByte(); flash[flash.lastIndex] = marker
                require(NativeEngine.restore(handle, flash), "cannot edit imported state")
            }
            controller.selectEngine(2); barrier(controller)
            require(controller.selectedFirmwareId == null && controller.selectedKind == 2, "native switch failed")
            controller.selectFirmware(first.id); barrier(controller)
            onWorker { require(NativeEngine.flash(field(controller, "machine") as Long).last() == marker, "import lost save") }
            fixture.writeBytes(ByteArray(1000))
            controller.importFirmware(android.net.Uri.fromFile(fixture), "Bad.fwsc"); barrier(controller)
            require(controller.selectedFirmwareId == first.id && field(controller, "ready") == true,
                "invalid import replaced the running instrument")
            controller.close()
            val closeDeadline = SystemClock.elapsedRealtime() + 20000
            while (field(controller, "closed") != true && SystemClock.elapsedRealtime() < closeDeadline) SystemClock.sleep(10)
            require(field(controller, "closed") == true, "import controller did not close")
            runOnMainSync { controller = EngineController(targetContext); controller.setPaused(true) }
            barrier(controller)
            require(controller.selectedFirmwareId == first.id, "import selection did not survive restart")
            onWorker { require(NativeEngine.flash(field(controller, "machine") as Long).last() == marker, "restart lost save") }
            controller.resetFirmware(13, first.id); barrier(controller)
            require(controller.selectedFirmwareId == first.id, "reset changed imported firmware")
            onWorker { require(NativeEngine.flash(field(controller, "machine") as Long).last() != marker, "reset kept old save") }
            controller.selectEngine(2); barrier(controller)
        } finally { controller.close(); fixture.delete() }
        return JSONObject().put("runs", runs).put("storageAndRejection", true)
    }

    private fun fm1vaSoundModes(): JSONArray {
        val results = JSONArray()
        val displays = mutableSetOf<Int>()
        for (mode in 1..2) {
            val m = NativeEngine.create(asset(12), 12)
            try {
                val pcm = FloatArray(1024)
                fun blocks(count: Int): Double {
                    var peak = 0.0
                    repeat(count) {
                        val frames = NativeEngine.render(m, pcm, 512)
                        require(frames in 0..512, "invalid FM-1+VA frame count")
                        for (i in 0 until frames * 2) {
                            require(pcm[i].isFinite(), "nonfinite FM-1+VA mode PCM")
                            peak = max(peak, abs(pcm[i]).toDouble())
                        }
                    }
                    return peak
                }
                fun press(id: Int) {
                    NativeEngine.key(m, id, true); blocks(20)
                    NativeEngine.key(m, id, false); blocks(20)
                }
                fun turn(delta: Int) { NativeEngine.encoder(m, 0, delta); blocks(30) }
                blocks(200)
                // Real v102 panel flow from its manual: EDIT > Erase Preset >
                // Erase > VA/8-Bit > SEL. The original saved preset stays untouched.
                press(6); turn(32); turn(-1); press(3); turn(1); press(3)
                repeat(mode) { turn(1) }
                press(3); press(8)
                val pixels = IntArray(57600)
                require(NativeEngine.snapshot(m, pixels, FloatArray(41)) != 0, "mode LCD missing")
                require(displays.add(pixels.contentHashCode()), "VA and 8-Bit screens unexpectedly match")
                NativeEngine.key(m, 21, true)
                val peak = blocks(100)
                NativeEngine.release(m)
                require(peak > 0.00001, "silent FM-1+VA sound mode $mode")
                results.put(JSONObject().put("mode", if (mode == 1) "VA" else "8-Bit").put("peak", peak))
            } finally { NativeEngine.destroy(m) }
        }
        return results
    }

    private fun fingerprint(s: ProjectSnapshot) = buildString {
        append(s.master)
        for ((name, bytes) in s.parts) {
            append(name)
            append(ProjectSnapshot.digest(bytes))
        }
    }

    private fun workspace(): JSONObject {
        val bytes = asset(2)
        val m = NativeEngine.create(bytes, 2)
        try {
            advance(m, 22050)
            repeat(3) {
                NativeEngine.encoder(m, 1, 1)
                advance(m, 2205)
            }
            tap(m, 13)
            for (key in intArrayOf(14, 16, 18, 20, 21)) {
                tap(m, key)
                advance(m, 8820)
            }
            tap(m, 13)
            advance(m, 132300)
            tap(m, 12)
            NativeEngine.release(m)
            NativeEngine.transport(m, false)
            advance(m, 512)
            val original = ProjectSnapshot.capture(m, 716)
            val originalHash = fingerprint(original)
            val backup = original.envelope("Device test backup", true)
            val imported = ProjectSnapshot.fromEnvelope(backup, "test.fm1backup")
            require(fingerprint(imported) == originalHash, "backup envelope changed parts")
            val legacy = JSONObject(String(backup, Charsets.UTF_8))
            legacy.getJSONObject("snapshot").put("packageSHA256", "7c9d269d15d1c1a48e0ebada7ffa2c2aa196c19d51056fd3f760d322322b335f")
            val legacySnapshot = ProjectSnapshot.fromEnvelope(legacy.toString().toByteArray(Charsets.UTF_8), "legacy.fm1backup")
            val legacyCandidate = legacySnapshot.create(bytes)
            try {
                val upgraded = ProjectSnapshot.capture(legacyCandidate, 716)
                for ((name, data) in original.parts) if (name != "flash") require(
                    data.contentEquals(upgraded.parts[name]), "legacy workspace changed $name",
                )
            } finally { NativeEngine.destroy(legacyCandidate) }
            legacy.getJSONObject("snapshot").put("packageSHA256", "0".repeat(64))
            var wrongReleaseRejected = false
            try { ProjectSnapshot.fromEnvelope(legacy.toString().toByteArray(Charsets.UTF_8), "wrong.fm1backup") }
            catch (expected: Exception) { wrongReleaseRejected = true }
            require(wrongReleaseRejected, "unknown firmware workspace accepted")
            val corrupt = JSONObject(String(backup, Charsets.UTF_8))
            corrupt
                .getJSONObject("snapshot")
                .getJSONArray("parts")
                .getJSONObject(0)
                .put("sha256", "bad")
            var rejected = false
            try {
                ProjectSnapshot.fromEnvelope(
                    corrupt.toString().toByteArray(Charsets.UTF_8),
                    "bad.fm1backup",
                )
            } catch (expected: Exception) {
                rejected = true
            }
            require(rejected, "bad workspace checksum accepted")
            val customized = original.copy()
            val settings = customized.parts.getValue("object1")
            require(settings.size >= 16, "missing settings fixture")
            settings[8] = (if (settings[8].toInt() == 0) 1 else 0).toByte()
            customized.master = 271
            var candidate = customized.create(bytes)
            try {
                val captured = ProjectSnapshot.capture(candidate, 271)
                require(
                    settings.contentEquals(captured.parts["object1"]),
                    "edited global settings lost",
                )
                require(
                    customized.parts["object0"].contentEquals(captured.parts["object0"]),
                    "candidate project changed",
                )
            } finally {
                NativeEngine.destroy(candidate)
            }
            val project =
                ProjectSnapshot.fromEnvelope(
                    original.envelope("Device project", false),
                    "test.fm1project",
                )
            val overlay = project.overlay(customized)
            require(
                overlay.master == 271 && overlay.parts["object1"].contentEquals(settings),
                "project overlay lost current globals",
            )
            val tone = ByteArray(44 + 22050 * 2)
            SampleAudio.header(22050, 22050, 1).copyInto(tone, 0, 0, 44)
            for (i in 0 until 22050) SampleAudio.put16(
                tone,
                44 + i * 2,
                (12000 * sin(2 * PI * 440 * i / 22050)).toInt(),
            )
            val decoded = SampleAudio.decode(tone, "TEST440")
            val sample = SampleAudio.encode(decoded, 0.0, 0.5, true, 60)
            require(
                sample.size == 81920 &&
                    sample[0] == 'F'.code.toByte() &&
                    sample[1] == 'S'.code.toByte(),
                "invalid FSMP encoding",
            )
            val preview = SampleAudio.decode(SampleAudio.preview(decoded, 0.0, 0.5), "preview")
            require(abs(preview.duration() - 0.5) < 0.001, "sample trim duration")
            val sampled = customized.copy()
            sampled.parts["sample0"] = sample
            candidate = sampled.create(bytes)
            try {
                require(
                    sample.contentEquals(NativeEngine.readPart(candidate, 2, 0)),
                    "native sample write differs",
                )
            } finally {
                NativeEngine.destroy(candidate)
            }
            val song = original.copy()
            song.parts["object2"] = original.parts.getValue("object0").clone()
            song.parts["object3"] = original.parts.getValue("object0").clone()
            song.parts["arrangement"] = byteArrayOf(0, 0, 3, 0, 0, 0, 0, 0, 0, 1, 2, 0, 1, 1, 2)
            val before = fingerprint(song)
            val loopBytes = AudioExport.render(targetContext, song, bytes, false, 1, 0).readBytes()
            require(loopBytes.size == 44 + 44100 * 4, "loop frame count differs")
            require(wavPeak(loopBytes) > 0.001, "silent recorded loop export")
            val songBytes = AudioExport.render(targetContext, song, bytes, true, 1, 0).readBytes()
            val longDuration = (songBytes.size - 44) / 176400.0
            require(
                longDuration > 2 && wavPeak(songBytes) > 0.001,
                "whole song did not export audible arrangement",
            )
            val shortSong = song.copy()
            shortSong.parts.getValue("arrangement")[14] = 1
            val shortBytes =
                AudioExport.render(targetContext, shortSong, bytes, true, 1, 0).readBytes()
            val shortDuration = (shortBytes.size - 44) / 176400.0
            require(
                abs(longDuration / shortDuration - 1.5) < 0.015,
                "song A1+B2 / A1+B1 duration ratio differs",
            )
            require(fingerprint(song) == before, "export modified source snapshot")
            require(
                fingerprint(ProjectSnapshot.capture(m, 716)) == originalHash,
                "candidate/export modified original machine",
            )
            val invalid = song.copy()
            invalid.parts.getValue("arrangement")[2] = 1
            rejected = false
            try {
                AudioExport.render(targetContext, invalid, bytes, true, 1, 0)
            } catch (expected: Exception) {
                rejected = true
            }
            require(rejected, "missing song section accepted")
            return JSONObject()
                .put("backupBytes", backup.size)
                .put("sampleBytes", sample.size)
                .put("loopFrames", 44100)
                .put("songSeconds", longDuration)
                .put("shortSongSeconds", shortDuration)
                .put("sourceUnchanged", true)
                .put("checksumRejected", true)
        } finally {
            NativeEngine.destroy(m)
        }
    }

    private fun wavPeak(b: ByteArray): Double {
        require(
            b.size > 44 && b[0] == 'R'.code.toByte() && b[8] == 'W'.code.toByte(),
            "invalid WAV",
        )
        var peak = 0.0
        for (i in 44 until b.size - 1 step 2) peak =
            max(
                peak,
                abs(((b[i].toInt() and 255) or (b[i + 1].toInt() shl 8)).toShort().toInt()) /
                    32768.0,
            )
        return peak
    }

    private fun exchangeAsset(name: String) =
        context.assets.open("exchange/$name").use { it.readBytes() }

    private fun overlayFresh(project: ProjectSnapshot, bytes: ByteArray): ProjectSnapshot {
        val baseline = NativeEngine.create(bytes, 2)
        try {
            advance(baseline, 22050)
            return project.overlay(ProjectSnapshot.capture(baseline, 716))
        } finally {
            NativeEngine.destroy(baseline)
        }
    }

    private fun crossPlatform(): JSONObject {
        val bytes = asset(2)
        val metrics = JSONObject()
        var backup: ProjectSnapshot? = null
        for (extension in arrayOf("fm1project", "fm1backup")) {
            val filename = "swift-generated.$extension"
            val incoming = ProjectSnapshot.fromEnvelope(exchangeAsset(filename), filename)
            val expected = if (extension == "fm1backup") 517 else 716
            require(incoming.master == expected, "Swift envelope master differs for $extension")
            require(incoming.parts.size == 18, "Swift component count differs")
            val restored =
                if (extension == "fm1project") overlayFresh(incoming, bytes) else incoming
            val candidate = restored.create(bytes)
            try {
                advance(candidate, 512)
                require(
                    NativeEngine.kind(candidate) == 2,
                    "Swift imported candidate is not native SLOOP",
                )
                val captured = ProjectSnapshot.capture(candidate, expected)
                require(
                    incoming.parts["object0"].contentEquals(captured.parts["object0"]),
                    "Swift project object0 changed during native import",
                )
                for (i in 0..3) require(
                    incoming.parts["sample$i"].contentEquals(captured.parts["sample$i"]),
                    "Swift sample changed during native import",
                )
                metrics.put(
                    extension,
                    JSONObject()
                        .put("master", incoming.master)
                        .put("components", incoming.parts.size)
                        .put("object0Bytes", incoming.parts.getValue("object0").size)
                        .put("candidateNativeKind", NativeEngine.kind(candidate)),
                )
                if (extension == "fm1backup") backup = captured
            } finally {
                NativeEngine.destroy(candidate)
            }
        }
        val iosExport =
            ProjectSnapshot.fromEnvelope(
                exchangeAsset("ios-simulator.fm1project"),
                "ios-simulator.fm1project",
            )
        require(iosExport.parts.size == 18, "actual iOS export component count differs")
        val iosCandidate = overlayFresh(iosExport, bytes).create(bytes)
        try {
            advance(iosCandidate, 512)
            require(NativeEngine.kind(iosCandidate) == 2, "actual iOS export native kind differs")
            metrics.put(
                "iosSimulatorUIExport",
                JSONObject()
                    .put("master", iosExport.master)
                    .put("components", iosExport.parts.size)
                    .put("object0Bytes", iosExport.parts.getValue("object0").size)
                    .put("candidateNativeKind", NativeEngine.kind(iosCandidate)),
            )
        } finally {
            NativeEngine.destroy(iosCandidate)
        }
        require(backup != null, "Swift backup fixture absent")
        val exported = backup!!.envelope("Android cross-platform workspace", true)
        val directory = targetContext.getExternalFilesDir(null)
        require(directory != null, "external private exchange directory unavailable")
        val output = File(directory, "android-generated.fm1backup")
        FileOutputStream(output).use {
            it.write(exported)
            it.fd.sync()
        }
        metrics
            .put("androidEnvelopeBytes", exported.size)
            .put("androidEnvelopeSHA256", ProjectSnapshot.digest(exported))
            .put("androidEnvelopePath", output.absolutePath)
        return metrics
    }

    private fun field(instance: Any, name: String): Any {
        val f = instance.javaClass.getDeclaredField(name)
        f.isAccessible = true
        return f.get(instance)
    }

    private fun barrier(controller: EngineController) {
        val done = CountDownLatch(1)
        (field(controller, "worker") as Handler).post { done.countDown() }
        require(done.await(20, TimeUnit.SECONDS), "engine worker did not settle")
    }

    private fun firmwareReset(): JSONObject {
        lateinit var controller: EngineController
        runOnMainSync {
            controller = EngineController(targetContext)
            controller.setPaused(true)
        }
        fun onWorker(action: () -> Unit) {
            val done = CountDownLatch(1)
            var error: Throwable? = null
            (field(controller, "worker") as Handler).post {
                try { action() } catch (e: Throwable) { error = e } finally { done.countDown() }
            }
            require(done.await(10, TimeUnit.SECONDS), "reset test worker timed out")
            error?.let { throw it }
        }
        try {
            barrier(controller)
            for (kind in EngineController.ENGINE_KINDS) {
                controller.selectEngine(kind)
                barrier(controller)
                require(controller.status.isEmpty(), "load before reset failed: ${controller.status}")
                var fresh = byteArrayOf()
                onWorker {
                    val candidate = NativeEngine.create(asset(kind), kind)
                    try { fresh = NativeEngine.flash(candidate) }
                    finally { NativeEngine.destroy(candidate) }
                    val handle = field(controller, "machine") as Long
                    val edited = NativeEngine.flash(handle)
                    edited[edited.lastIndex] = (fresh.last().toInt() xor 0x25).toByte()
                    require(NativeEngine.restore(handle, edited), "could not seed edited state")
                }
                controller.setPaused(true)
                barrier(controller)
                val directory = File(targetContext.filesDir, "Instruments")
                val save = File(directory, "$kind${if (kind == 6) ".preview.flash" else ".flash"}")
                require(save.readBytes().last() != fresh.last(), "edited state was not saved")
                val otherStates = directory.listFiles()!!.filter { it != save }
                    .associate { it.name to ProjectSnapshot.digest(it.readBytes()) }
                val projectFiles = controller.listProjects().associateWith {
                    ProjectSnapshot.digest(File(targetContext.filesDir, "Projects/$it").readBytes())
                }
                controller.resetFirmware(kind)
                barrier(controller)
                require(controller.status.isEmpty(), "reset failed: ${controller.status}")
                require(save.readBytes().contentEquals(fresh), "reset did not persist factory flash")
                require(controller.selectedKind == kind && controller.paused, "reset changed host state")
                for ((name, hash) in otherStates) require(
                    ProjectSnapshot.digest(File(directory, name).readBytes()) == hash,
                    "reset kind $kind changed another firmware $name",
                )
                for ((name, hash) in projectFiles) require(
                    ProjectSnapshot.digest(File(targetContext.filesDir, "Projects/$name").readBytes()) == hash,
                    "reset changed a saved project",
                )
                controller.selectEngine(kind)
                barrier(controller)
                onWorker {
                    require(
                        NativeEngine.flash(field(controller, "machine") as Long).last() == fresh.last(),
                        "reload restored the discarded state",
                    )
                }
            }
            return JSONObject().put("resetAndReloadKinds", JSONArray(EngineController.ENGINE_KINDS.toList()))
        } finally {
            controller.close()
            val thread = field(controller, "workerThread") as Thread
            thread.join(10000)
            require(!thread.isAlive, "reset test controller did not close")
        }
    }

    private fun delay(ms: Long) = SystemClock.sleep(ms)

    private fun held(panel: PanelView, id: Int): Int {
        var n = -1
        runOnMainSync { n = (field(panel, "held") as IntArray)[id] }
        return n
    }

    private fun point(panel: PanelView, id: Int): FloatArray {
        var result: FloatArray? = null
        runOnMainSync {
            for (control in field(panel, "controls") as ArrayList<*>) {
                val a = control!!
                if (field(a, "type") as Int == 0 && field(a, "id") as Int == id) {
                    val r = field(a, "r") as RectF
                    val xy = IntArray(2)
                    panel.getLocationOnScreen(xy)
                    val scale = field(panel, "scale") as Float
                    result =
                        floatArrayOf(
                            xy[0] + r.centerX() * scale,
                            xy[1] + (r.bottom - r.height() * .1f) * scale,
                        )
                    break
                }
            }
        }
        require(result != null, "missing panel key $id")
        return result!!
    }

    private fun pointer(down: Long, action: Int, vararg points: FloatArray) {
        val properties =
            Array(points.size) { i ->
                MotionEvent.PointerProperties().apply {
                    id = i
                    toolType = MotionEvent.TOOL_TYPE_FINGER
                }
            }
        val coordinates =
            Array(points.size) { i ->
                MotionEvent.PointerCoords().apply {
                    x = points[i][0]
                    y = points[i][1]
                    pressure = 1f
                    size = .05f
                }
            }
        val event =
            MotionEvent.obtain(
                down,
                SystemClock.uptimeMillis(),
                action,
                points.size,
                properties,
                coordinates,
                0,
                0,
                1f,
                1f,
                0,
                0,
                InputDevice.SOURCE_TOUCHSCREEN,
                0,
            )
        sendPointerSync(event)
        event.recycle()
        waitForIdleSync()
        delay(35)
    }

    private fun activity(): JSONObject {
        val activity =
            startActivitySync(
                Intent(targetContext, MainActivity::class.java)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            )
                as MainActivity
        try {
            waitForIdleSync()
            delay(900)
            val controller = field(activity, "engine") as EngineController
            val panel = field(activity, "panel") as PanelView
            runOnMainSync {
                controller.selectEngine(2)
                controller.setPaused(false)
                controller.setForeground(true)
            }
            barrier(controller)
            delay(600)
            require(
                controller.selectedKind == 2 && controller.status.isEmpty(),
                "activity native engine not ready: ${controller.status}",
            )
            val a = point(panel, 21)
            val b = point(panel, 23)
            val c = point(panel, 25)
            var down = SystemClock.uptimeMillis()
            pointer(down, MotionEvent.ACTION_DOWN, a)
            delay(180)
            require(held(panel, 21) == 1, "bottom note down missed")
            pointer(
                down,
                MotionEvent.ACTION_POINTER_DOWN or (1 shl MotionEvent.ACTION_POINTER_INDEX_SHIFT),
                a,
                a,
            )
            require(held(panel, 21) == 2, "same-key touch reference count lost")
            pointer(
                down,
                MotionEvent.ACTION_POINTER_UP or (1 shl MotionEvent.ACTION_POINTER_INDEX_SHIFT),
                a,
                a,
            )
            require(held(panel, 21) == 1, "one pointer released another pointer's note")
            pointer(down, MotionEvent.ACTION_UP, a)
            require(held(panel, 21) == 0, "same-key final release stuck")
            down = SystemClock.uptimeMillis()
            pointer(down, MotionEvent.ACTION_DOWN, a)
            pointer(
                down,
                MotionEvent.ACTION_POINTER_DOWN or (1 shl MotionEvent.ACTION_POINTER_INDEX_SHIFT),
                a,
                b,
            )
            pointer(down, MotionEvent.ACTION_MOVE, c, b)
            require(
                held(panel, 21) == 0 && held(panel, 25) == 1 && held(panel, 23) == 1,
                "multitouch glissando routing failed",
            )
            pointer(
                down,
                MotionEvent.ACTION_POINTER_UP or (1 shl MotionEvent.ACTION_POINTER_INDEX_SHIFT),
                c,
                b,
            )
            pointer(down, MotionEvent.ACTION_UP, c)
            for (key in intArrayOf(21, 23, 25)) require(
                held(panel, key) == 0,
                "glissando note stuck",
            )
            repeat(8) {
                down = SystemClock.uptimeMillis()
                pointer(down, MotionEvent.ACTION_DOWN, a)
                pointer(down, MotionEvent.ACTION_UP, a)
            }
            barrier(controller)
            delay(300)
            require(controller.peak > 0.00001, "actual activity note generated no PCM")
            require(controller.outputFrames > 0, "AudioTrack received no generated PCM")
            // Measure warmed continuous playback separately from loading and lifecycle gaps.
            down = SystemClock.uptimeMillis()
            pointer(down, MotionEvent.ACTION_DOWN, a)
            delay(1000)
            val systemTrack = field(controller, "track") as AudioTrack
            val osUnderrunsStart = systemTrack.underrunCount
            val osHeadStart = Integer.toUnsignedLong(systemTrack.playbackHeadPosition)
            val warmStart = SystemClock.elapsedRealtime()
            val warmProduced = controller.producedFrames
            val warmOutput = controller.outputFrames
            val warmUnderflow = controller.underflowFrames
            delay(5000)
            val warmMillis = SystemClock.elapsedRealtime() - warmStart
            val warmFrames = controller.producedFrames - warmProduced
            val warmOutputFrames = controller.outputFrames - warmOutput
            val warmUnderflowFrames = controller.underflowFrames - warmUnderflow
            val osUnderruns = systemTrack.underrunCount - osUnderrunsStart
            val osHeadFrames =
                (Integer.toUnsignedLong(systemTrack.playbackHeadPosition) - osHeadStart) and
                    0xffffffffL
            pointer(down, MotionEvent.ACTION_UP, a)
            val warmDiagnostic =
                "warmed audio did not keep generating/outputting PCM: elapsedMs=$warmMillis, produced=$warmFrames, output=$warmOutputFrames, underflow=$warmUnderflowFrames, status=${controller.status}, ready=${field(controller,"ready")}, foreground=${field(controller,"foreground")}, paused=${controller.paused}, audioBufferFrames=${field(controller,"audioBufferFrames")}, osUnderruns=$osUnderruns, osPlaybackHeadFrames=$osHeadFrames"
            val warmed =
                JSONObject()
                    .put("osUnderrunDelta", osUnderruns)
                    .put("osPlaybackHeadFrames", osHeadFrames)
                    .put("osSampleRate", systemTrack.sampleRate)
                    .put("osBufferCapacityFrames", systemTrack.bufferCapacityInFrames)
                    .put("elapsedMs", warmMillis)
                    .put("producedFrames", warmFrames)
                    .put("outputFrames", warmOutputFrames)
                    .put("underflowFrames", warmUnderflowFrames)
                    .put("audioTrackBufferFrames", field(controller, "audioBufferFrames") as Int)
            // A held note is cancelled by the real Activity lifecycle callbacks.
            down = SystemClock.uptimeMillis()
            pointer(down, MotionEvent.ACTION_DOWN, a)
            runOnMainSync { callActivityOnPause(activity) }
            barrier(controller)
            require(held(panel, 21) == 0, "Activity pause retained held key")
            var stopped = controller.producedFrames
            delay(160)
            require(controller.producedFrames == stopped, "background engine kept rendering")
            runOnMainSync { callActivityOnResume(activity) }
            delay(160)
            require(controller.producedFrames > stopped, "foreground did not resume playback")
            pointer(down, MotionEvent.ACTION_CANCEL, a)
            runOnMainSync { controller.setPaused(true) }
            barrier(controller)
            stopped = controller.producedFrames
            runOnMainSync {
                callActivityOnPause(activity)
                callActivityOnResume(activity)
            }
            barrier(controller)
            delay(160)
            require(
                controller.paused && controller.producedFrames == stopped,
                "explicit pause lost across foreground transition",
            )
            runOnMainSync { controller.setPaused(false) }
            delay(160)
            require(controller.producedFrames > stopped, "explicit resume failed")
            val activityPeak = controller.peak
            // Real controller save/open/reload writes a durable candidate before replacement.
            runOnMainSync { controller.setPaused(true) }
            barrier(controller)
            runOnMainSync { controller.setMaster(271) }
            barrier(controller)
            val name = "Instrumentation durable ${System.currentTimeMillis()}"
            runOnMainSync { controller.saveProject(name, true) }
            barrier(controller)
            require(
                controller.status.lowercase(Locale.ROOT).contains("saved"),
                "controller backup failed: ${controller.status}",
            )
            val savedName =
                controller.listProjects().lastOrNull {
                    it.startsWith(EngineController.safeName(name))
                }
            require(savedName != null, "saved controller backup missing")
            runOnMainSync { controller.setMaster(812) }
            barrier(controller)
            runOnMainSync { controller.openProject(savedName!!) }
            barrier(controller)
            require(
                controller.status.contains("Opened"),
                "controller saved-copy open failed: ${controller.status}",
            )
            require(controller.getMaster() == 271, "backup did not restore edited master")
            runOnMainSync { controller.selectEngine(2) }
            barrier(controller)
            require(
                controller.selectedKind == 2 && controller.status.isEmpty(),
                "controller reload failed: ${controller.status}",
            )
            runOnMainSync { activity.finish() }
            delay(350)
            val reopened =
                startActivitySync(
                    Intent(targetContext, MainActivity::class.java)
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                )
                    as MainActivity
            try {
                waitForIdleSync()
                delay(400)
                val restored = field(reopened, "engine") as EngineController
                barrier(restored)
                require(
                    restored.getMaster() == 271,
                    "recreated Activity lost restored backup master",
                )
                require(
                    restored.selectedKind == 2 && restored.status.isEmpty(),
                    "recreated Activity did not restore native instrument",
                )
            } finally {
                val restored = field(reopened, "engine") as EngineController
                runOnMainSync { reopened.finish() }
                waitForIdleSync()
                (field(restored, "workerThread") as Thread).join(10000)
                require(!(field(restored, "workerThread") as Thread).isAlive, "recreated Activity worker did not close")
            }
            require(
                warmFrames > 44100 * 3 && warmOutputFrames > 44100 * 3,
                "$warmDiagnostic, lifecycleAndEditedMasterRecreationPassed=true",
            )
            return JSONObject()
                .put("warmedPlayback", warmed)
                .put("bottomTaps", 8)
                .put("sameKeyReferenceCount", true)
                .put("multitouchGlissando", true)
                .put("activityPauseReleasedTouches", true)
                .put("explicitPauseRetained", true)
                .put("automaticResume", true)
                .put("savedCopyReload", true)
                .put("restoredMasterAfterActivityRecreation", 271)
                .put("producedFrames", controller.producedFrames)
                .put("outputFrames", controller.outputFrames)
                .put("underflowFrames", controller.underflowFrames)
                .put("peak", activityPeak)
        } finally {
            val owner = field(activity, "engine") as EngineController
            runOnMainSync { activity.finish() }
            waitForIdleSync()
            (field(owner, "workerThread") as Thread).join(10000)
            require(!(field(owner, "workerThread") as Thread).isAlive, "Activity worker did not close")
        }
    }

    private fun materialSystemUi(): JSONObject {
        val activity = startActivitySync(Intent(targetContext, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) as MainActivity
        var night = false
        try {
            waitForIdleSync()
            runOnMainSync {
                val surface = com.google.android.material.color.MaterialColors.getColor(activity,
                    com.google.android.material.R.attr.colorSurface, "test")
                val foreground = com.google.android.material.color.MaterialColors.getColor(activity,
                    com.google.android.material.R.attr.colorOnSurface, "test")
                val primary = com.google.android.material.color.MaterialColors.getColor(activity,
                    androidx.appcompat.R.attr.colorPrimary, "test")
                require(androidx.core.graphics.ColorUtils.calculateContrast(foreground, surface) >= 4.5,
                    "system text lacks accessible surface contrast")
                night = activity.resources.configuration.uiMode and android.content.res.Configuration.UI_MODE_NIGHT_MASK ==
                    android.content.res.Configuration.UI_MODE_NIGHT_YES
                require((androidx.core.graphics.ColorUtils.calculateLuminance(surface) < 0.5) == night,
                    "Material surface does not follow system light/dark mode")
                if (android.os.Build.VERSION.SDK_INT >= 34 &&
                    com.google.android.material.color.DynamicColors.isDynamicColorAvailable()) {
                    val expected = activity.getColor(if (night) android.R.color.system_primary_dark
                        else android.R.color.system_primary_light)
                    require(primary == expected, "Material primary does not match the system dynamic palette")
                }
                for (name in arrayOf("firmware", "transport", "mute", "folder", "collapse")) {
                    val button = field(activity, name)
                    require(button is com.google.android.material.button.MaterialButton,
                        "$name is not a Material button")
                }
                val originalFinish = field(activity, "finish") as Int
                val finishField = MainActivity::class.java.getDeclaredField("finish").apply { isAccessible = true }
                val apply = MainActivity::class.java.getDeclaredMethod("applyFinish").apply { isAccessible = true }
                try {
                    finishField.setInt(activity, (originalFinish + 1) % 6)
                    apply.invoke(activity)
                    val firmware = field(activity, "firmware") as com.google.android.material.button.MaterialButton
                    require(firmware.currentTextColor == primary, "panel finish changed shell text color")
                } finally {
                    finishField.setInt(activity, originalFinish)
                    apply.invoke(activity)
                }
                MainActivity::class.java.getDeclaredMethod("chooseFinish").apply { isAccessible = true }.invoke(activity)
                val dialog = field(activity, "finishDialog") as androidx.appcompat.app.AlertDialog
                require(dialog.listView.count == 6 && dialog.listView.checkedItemPosition == originalFinish,
                    "Material panel selector lost selection or finishes")
                dialog.dismiss()
                val fieldMethod = MainActivity::class.java.getDeclaredMethod("textField", String::class.java,
                    String::class.java, Boolean::class.javaPrimitiveType).apply { isAccessible = true }
                val input = fieldMethod.invoke(activity, "Root note", "60", true) as Pair<*, *>
                require(input.first is com.google.android.material.textfield.TextInputLayout &&
                    input.second is com.google.android.material.textfield.TextInputEditText,
                    "form does not use Material text fields")
                val dropdownMethod = MainActivity::class.java.getDeclaredMethod("dropdown", String::class.java,
                    Array<String>::class.java).apply { isAccessible = true }
                val dropdown = dropdownMethod.invoke(activity, "Render", arrayOf("Loop", "Whole song")) as Pair<*, *>
                require(dropdown.second is com.google.android.material.textfield.MaterialAutoCompleteTextView,
                    "selector does not use a Material exposed dropdown")
            }
            return JSONObject().put("nightMode", night).put("finishIndependent", true)
                .put("materialDialogsAndFields", true)
        } finally {
            val controller = field(activity, "engine") as EngineController
            runOnMainSync { activity.finish() }
            waitForIdleSync()
            (field(controller, "workerThread") as Thread).join(10000)
        }
    }

    /** Explicitly isolated package: never run this workload against the owner's saves. */
    private fun physicalPerformance(): JSONObject {
        require(targetContext.packageName == "org.fm1.android.benchmark", "Performance tests require the isolated benchmark APK")
        require(performanceSeconds in 30..600 && performanceSeconds % 5 == 0, "Performance duration must be 30–600 seconds in five-second intervals")
        val activity = startActivitySync(Intent(targetContext, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) as MainActivity
        val controller = field(activity, "engine") as EngineController
        try {
            runOnMainSync {
                controller.selectEngine(12)
                controller.setPaused(false)
                controller.setMuted(false)
                controller.setMaster(716)
            }
            barrier(controller)
            require(controller.selectedKind == 12, "FM-1+VA did not load")
            delay(3000)
            val before = controller.audioDiagnostics()
            require(before.getBoolean("running"), "Benchmark is not foreground and running; unlock the phone and leave its benchmark Activity visible")
            val start = SystemClock.elapsedRealtime()
            val slices = JSONArray()
            repeat(performanceSeconds / 5) {
                for (key in intArrayOf(21, 25, 28, 32)) controller.key(key, true)
                delay(4000)
                for (key in intArrayOf(21, 25, 28, 32)) controller.key(key, false)
                delay(1000)
                slices.put(controller.audioDiagnostics())
            }
            // Include the bounded wait in elapsed time; starvation must fail the progress gate.
            val finalBarrier = CountDownLatch(1)
            (field(controller, "worker") as Handler).post { finalBarrier.countDown() }
            val workerSettled = finalBarrier.await(20, TimeUnit.SECONDS)
            val after = controller.audioDiagnostics()
            val elapsed = SystemClock.elapsedRealtime() - start
            val result = JSONObject().put("elapsedMs", elapsed).put("before", before).put("after", after).put("intervals", slices)
                .put("requestedDurationSeconds", performanceSeconds)
                .put("finalWorkerBarrierSettled", workerSettled).put("requestedAppMaster", 716)
                .put("appMaster", controller.getMaster()).put("muted", controller.muted)
            // Keep the receipt even on failure, so deficits cannot disappear behind an assertion.
            File(targetContext.filesDir, "performance.json").writeText(result.toString(2))
            val underflow = after.getLong("underflowFrames") - before.getLong("underflowFrames")
            val underruns = after.getInt("osUnderruns") - before.getInt("osUnderruns")
            require(workerSettled, "Final key releases did not settle within 20 seconds")
            require(!controller.muted && controller.getMaster() == 716, "App master/mute changed during measurement")
            require(elapsed >= performanceSeconds * 1000L, "Measurement was shorter than $performanceSeconds seconds")
            require(after.getBoolean("running") && slices.let { a -> (0 until a.length()).all { a.getJSONObject(it).getBoolean("running") } },
                "Benchmark stopped or left foreground during measurement")
            require(after.getLong("generation") == before.getLong("generation"), "Audio stream reset during measurement")
            require(after.getInt("sampleRate") == before.getInt("sampleRate"), "Audio sample rate changed during measurement")
            val expected = elapsed * after.getInt("sampleRate") / 1000
            val produced = after.getLong("producedFrames") - before.getLong("producedFrames")
            val accepted = after.getLong("acceptedGeneratedFrames") - before.getLong("acceptedGeneratedFrames")
            val played = (after.getLong("playbackHeadFramesSinceFlush") - before.getLong("playbackHeadFramesSinceFlush")) and 0xffffffffL
            require(produced >= expected - 4096 && accepted >= expected - 4096 && played in (expected - 4096)..(expected + 4096),
                "Audio failed to progress at real time: expected=$expected produced=$produced accepted=$accepted played=$played")
            val appliedEdges = after.getJSONObject("inputQueue").getLong("count") - before.getJSONObject("inputQueue").getLong("count")
            val expectedEdges = performanceSeconds / 5 * 8
            require(appliedEdges >= expectedEdges, "Four-note workload was not fully applied: expected $expectedEdges key edges, applied=$appliedEdges")
            require(underflow == 0L && underruns == 0, "Steady playback failed: host missing=$underflow frames, AudioTrack underruns=$underruns; see performance.json")
            require(controller.status.isEmpty(), "runtime error: ${controller.status}")
            return result
        } finally {
            runOnMainSync { activity.finish() }
            waitForIdleSync()
            (field(controller, "workerThread") as Thread).join(20000)
        }
    }

    private fun runChecks() {
        if (performanceOnly) check("physical-fm1va-continuity", ::physicalPerformance)
        else if (compilerOnly) check("compiler-binary-parity-and-import", ::compiledFirmware)
        else {
        if (!uiOnly) {
            for (kind in 2..10) check("engine-$kind") { engine(kind) }
            check("engine-11-stock") { interpretedEngine(11) }
            check("engine-12-fm1va") { interpretedEngine(12) }
            check("compiler-binary-parity-and-import", ::compiledFirmware)
            check("sloop-workspace-sample-loop-song", ::workspace)
            if (exchangeTests) check("cross-platform-workspace", ::crossPlatform)
        }
        check("material-system-ui", ::materialSystemUi)
        check("activity-multitouch-audio-lifecycle-persistence", ::activity)
        check("firmware-factory-reset-persistence", ::firmwareReset)
        }
        val result = Bundle()
        try {
            val receipt =
                JSONObject()
                    .put("platform", "Android")
                    .put("model", android.os.Build.MODEL)
                    .put("fingerprint", android.os.Build.FINGERPRINT)
                    .put("sdk", android.os.Build.VERSION.SDK_INT)
                    .put("abi", android.os.Build.SUPPORTED_ABIS[0])
                    .put("failures", failures)
                    .put("exchangeTestsRequested", exchangeTests)
                    .put(
                        "exchangeTestStatus",
                        if (exchangeTests) "included"
                        else "not requested; external fixtures not supplied",
                    )
                    .put("results", results)
            File(targetContext.filesDir, "instrumentation.json").outputStream().use {
                it.write(receipt.toString(2).toByteArray(Charsets.UTF_8))
            }
            result.putString("stream", receipt.toString(2) + "\n")
        } catch (error: Exception) {
            failures++
            result.putString("stream", error.toString())
        }
        finish(if (failures == 0) -1 else 0, result)
    }
}
