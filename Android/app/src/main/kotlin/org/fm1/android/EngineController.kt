package org.fm1.android

import android.content.Context
import android.net.Uri
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.os.Debug
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.os.Process
import android.os.SystemClock
import android.util.Log
import java.io.*
import java.nio.file.Files
import java.util.Locale
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean
import org.json.JSONObject
import kotlin.math.abs

/** Serial native ownership, bounded prepared PCM, and durable private storage. */
class EngineController(context: Context) {
    @JvmField @Volatile var selectedKind = 2
    @JvmField @Volatile var paused = false
    @JvmField @Volatile var muted = false
    @JvmField @Volatile var status = "Starting instrument…"
    @JvmField @Volatile var pixels = IntArray(57600)
    @JvmField @Volatile var leds = FloatArray(41)
    @JvmField @Volatile var producedFrames = 0L
    @JvmField @Volatile var outputFrames = 0L
    @JvmField @Volatile var underflowFrames = 0L
    @JvmField @Volatile var underflowEvents = 0L
    @JvmField @Volatile var submittedFrames = 0L
    @JvmField @Volatile var audioWriteError = 0
    @JvmField @Volatile var peak = 0.0

    interface Listener {
        fun onFrame()

        fun onStatus()
    }

    fun interface ExportCallback {
        fun complete(file: File?, error: Exception?)
    }

    @Volatile private var frameListener: Listener? = null
    private val context = context.applicationContext
    private val directory = File(context.filesDir, "Instruments").apply { mkdirs() }
    private val projects = File(context.filesDir, "Projects").apply { mkdirs() }
    private val firmwareLibrary = FirmwareLibrary(context)
    @Volatile var selectedFirmwareId: String? = null
        private set
    @Volatile private var importedName = "Imported firmware"
    @Volatile var compiledInstructions = 0L
        private set
    private val prefs = context.getSharedPreferences("fm1", 0)
    private val main = Handler(Looper.getMainLooper())
    private val workerThread = HandlerThread("FM1 native owner", Process.THREAD_PRIORITY_AUDIO)
    private val worker: Handler
    private val persistence = OrderedPersistence(::atomicWrite) { error -> main.post { failure(error) } }
    private val renderTimes = AudioTiming()
    private val renderCpuTimes = AudioTiming()
    private val producerTickTimes = AudioTiming()
    private val producerTickCpuTimes = AudioTiming()
    private val pcmWaitTimes = AudioTiming()
    private val renderWakeTimes = AudioTiming()
    private val inputQueueTimes = AudioTiming()
    private val snapshotTimes = AudioTiming()
    private val checkpointTimes = AudioTiming()
    private val tickScheduled = AtomicBoolean(false)
    private val framePosted = AtomicBoolean(false)
    @Volatile private var renderRequestedAt = 0L
    private val ringLock = java.lang.Object()
    private val ring = FloatArray(8192 * 2)
    private val renderBuffer = FloatArray(1024)
    private var read = 0
    private var write = 0
    private var available = 0
    @Volatile private var audioGeneration = 0L
    @Volatile private var audioFlushes = 0L
    @Volatile private var submittedSinceFlush = 0L
    @Volatile private var runtimeBackend = 0
    @Volatile private var ringLowFrames = 1024
    @Volatile private var ringHighFrames = 0
    @Volatile private var closed = false
    @Volatile private var foreground = true
    @Volatile private var ready = false
    private var machine = 0L
    private var lastSnapshot = 0L
    private var lastSave = 0L
    private val snapshotPixels = IntArray(57600)
    private val snapshotLeds = FloatArray(41)
    @Volatile private var exportThread: Thread? = null
    @JvmField val audioBufferFrames: Int
    private var masterValue = prefs.getInt("master", 716)
    private val track: AudioTrack
    private val audioThread: Thread
    private val tick =
        object : Runnable {
            override fun run() {
                val requestedAt = renderRequestedAt
                tickScheduled.set(false)
                if (closed) return
                var madeProgress = false
                val tickStarted = System.nanoTime()
                val tickCpuStarted = Debug.threadCpuTimeNanos()
                try {
                    if (running()) {
                        if (requestedAt != 0L) renderWakeTimes.record(System.nanoTime() - requestedAt)
                        val need = synchronized(ringLock) { 1024 - available }
                        if (need >= 512) {
                            val started = System.nanoTime()
                            val cpuStarted = Debug.threadCpuTimeNanos()
                            val frames = try {
                                NativeEngine.render(machine, renderBuffer, 512)
                            } finally {
                                val cpuElapsed = Debug.threadCpuTimeNanos() - cpuStarted
                                val elapsed = System.nanoTime() - started
                                renderCpuTimes.record(cpuElapsed)
                                renderTimes.record(elapsed)
                            }
                            if (frames !in 0..512 || (runtimeBackend == 2 && frames != 512))
                                throw IOException("The instrument did not return complete audio.")
                            for (i in 0 until frames * 2) {
                                val value = renderBuffer[i]
                                if (!value.isFinite())
                                    throw IOException("The instrument returned invalid audio.")
                                peak = maxOf(peak, abs(value.toDouble()))
                            }
                            synchronized(ringLock) {
                                for (i in 0 until frames * 2) ring[(write * 2 + i) % ring.size] =
                                    renderBuffer[i]
                                write = (write + frames) % 8192
                                available += frames
                                ringHighFrames = maxOf(ringHighFrames, available)
                                ringLock.notifyAll()
                            }
                            producedFrames += frames
                            madeProgress = frames > 0
                        }
                        val now = SystemClock.uptimeMillis()
                        // Prefer a refilled ring for optional work. Bound deferral so an
                        // overloaded instrument still updates its UI and autosaves.
                        val prepared = synchronized(ringLock) { available }
                        if (now - lastSnapshot >= 33 && (prepared >= 1024 || now - lastSnapshot >= 200)) {
                            publishFrame()
                            compiledInstructions = NativeEngine.jitInstructions(machine)
                            runtimeBackend = NativeEngine.backend(machine)
                            lastSnapshot = now
                        }
                        if (now - lastSave >= 10000 && (prepared >= 1024 || now - lastSave >= 30000)
                            && !persistence.periodicPending()) {
                            checkpoint(periodic = true)
                            lastSave = now
                        }
                    }
                } catch (error: Exception) {
                    ready = false
                    clearRing()
                    failure(error)
                } finally {
                    val cpuElapsed = Debug.threadCpuTimeNanos() - tickCpuStarted
                    val elapsed = System.nanoTime() - tickStarted
                    producerTickCpuTimes.record(cpuElapsed)
                    producerTickTimes.record(elapsed)
                }
                if (running() && synchronized(ringLock) { available < 1024 })
                    requestRender(if (madeProgress) 0 else 1)
            }
        }

    init {
        selectedKind = prefs.getInt("instrument", 2).takeIf { it in ENGINE_KINDS } ?: 2
        muted = prefs.getBoolean("muted", false)
        workerThread.start()
        worker = Handler(workerThread.looper)
        val minimum =
            AudioTrack.getMinBufferSize(
                44100,
                AudioFormat.CHANNEL_OUT_STEREO,
                AudioFormat.ENCODING_PCM_FLOAT,
            )
        track =
            AudioTrack.Builder()
                .setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_MEDIA)
                        .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                        .build()
                )
                .setAudioFormat(
                    AudioFormat.Builder()
                        .setSampleRate(44100)
                        .setChannelMask(AudioFormat.CHANNEL_OUT_STEREO)
                        .setEncoding(AudioFormat.ENCODING_PCM_FLOAT)
                        .build()
                )
                .setBufferSizeInBytes(maxOf(minimum, 1024 * 8))
                .setTransferMode(AudioTrack.MODE_STREAM)
                .setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
                .build()
        track.setBufferSizeInFrames(maxOf(512, minimum / 8))
        audioBufferFrames = track.bufferSizeInFrames
        audioThread = Thread(::audioLoop, "FM1 PCM output").also { it.start() }
        worker.post {
            val imported = prefs.getString("firmwareId", null)
            if (imported != null) load(13, firmwareId = imported) else load(selectedKind)
            requestRender()
        }
    }

    fun setListener(listener: Listener?) {
        frameListener = listener
    }

    private fun notifyStatus() {
        main.post { frameListener?.onStatus() }
    }

    private fun message(value: String) {
        status = value
        notifyStatus()
    }

    private fun failure(error: Exception) {
        Log.e("FM1", "Operation failed", error)
        message(error.message ?: "The operation could not be completed.")
    }

    private fun clearRing() =
        synchronized(ringLock) {
            read = 0
            write = 0
            available = 0
            audioGeneration++
            ringLock.notifyAll()
        }

    private fun running() = ready && !paused && foreground && !closed

    private fun requestRender(delayMillis: Long = 0) {
        if (!closed && tickScheduled.compareAndSet(false, true)) {
            renderRequestedAt = System.nanoTime()
            worker.postDelayed(tick, delayMillis)
        }
    }

    /** Host timing and accepted output only; this does not measure acoustic touch latency. */
    fun audioDiagnostics(): JSONObject {
        fun timing(value: AudioTiming): JSONObject {
            val s = value.snapshot()
            return JSONObject().put("count", s.count).put("windowCount", s.windowCount)
                .put("windowP95Micros", s.p95Micros).put("lifetimeMaxMicros", s.maxMicros)
                .put("totalNanos", s.totalNanos)
        }
        val result = JSONObject().put("capturedAtElapsedNanos", SystemClock.elapsedRealtimeNanos())
            .put("running", running()).put("generation", audioGeneration)
            .put("ready", ready).put("foreground", foreground).put("paused", paused)
            .put("closed", closed).put("status", status)
            .put("producedFrames", producedFrames).put("acceptedGeneratedFrames", outputFrames)
            .put("acceptedTotalFrames", submittedFrames).put("underflowFrames", underflowFrames)
            .put("underflowEvents", underflowEvents).put("writeError", audioWriteError)
            .put("ringFrames", synchronized(ringLock) { available }).put("ringLowFrames", ringLowFrames)
            .put("ringHighFrames", ringHighFrames).put("ringTargetFrames", 1024)
            .put("flushes", audioFlushes).put("backend", runtimeBackend).put("jitInstructions", compiledInstructions)
            .put("render", timing(renderTimes)).put("producerWake", timing(renderWakeTimes))
            .put("renderCpu", timing(renderCpuTimes))
            .put("producerTick", timing(producerTickTimes)).put("producerTickCpu", timing(producerTickCpuTimes))
            .put("pcmWait", timing(pcmWaitTimes))
            .put("inputQueue", timing(inputQueueTimes)).put("snapshot", timing(snapshotTimes))
            .put("checkpointCapture", timing(checkpointTimes)).put("storageWrite", timing(persistence.writeTimes))
        try {
            result.put("sampleRate", track.sampleRate).put("bufferFrames", track.bufferSizeInFrames)
                .put("bufferCapacityFrames", track.bufferCapacityInFrames).put("performanceMode", track.performanceMode)
                .put("osUnderruns", track.underrunCount).put("acceptedFramesSinceFlush", submittedSinceFlush)
                .put("playbackHeadFramesSinceFlush", Integer.toUnsignedLong(track.playbackHeadPosition))
            track.routedDevice?.let { result.put("routeType", it.type).put("routeId", it.id) }
        } catch (_: IllegalStateException) { result.put("trackReleased", true) }
        return result
    }

    private fun audioLoop() {
        Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO)
        val output = FloatArray(1024)
        var generation = audioGeneration
        val cursor = PcmWriteCursor()
        var acceptedSinceFlush = 0L
        // Keep a full mixer period in reserve even on a route without fast output.
        val playbackDeadline = PcmPlaybackDeadline(track.sampleRate, audioBufferFrames,
            maxOf(512, track.sampleRate / 50))
        track.play()
        while (!closed) {
            if (generation != audioGeneration) {
                track.pause()
                track.flush()
                generation = audioGeneration
                acceptedSinceFlush = 0
                submittedSinceFlush = 0
                playbackDeadline.reset()
                audioFlushes++
                track.play()
            }
            var count = 0
            val play = running()
            if (play) requestRender()
            // Choose PCM before the queued audio really needs replacing, rather than
            // committing silence before a potentially blocking write with PCM queued.
            val waitStarted = System.nanoTime()
            val deadline = if (play) playbackDeadline.deadlineNanos(acceptedSinceFlush / 2,
                Integer.toUnsignedLong(track.playbackHeadPosition), waitStarted) else waitStarted
            var waited = false
            synchronized(ringLock) {
                while (!closed && running() && generation == audioGeneration && available < 512) {
                    val remaining = deadline - System.nanoTime()
                    if (remaining <= 0) break
                    waited = true
                    ringLock.wait(remaining / 1_000_000, (remaining % 1_000_000).toInt())
                }
                // A reset/selection can arrive between the flush check and this lock.
                // Leave its new PCM in the ring until output has acknowledged that generation.
                if (play && running() && generation == audioGeneration) {
                    count = minOf(512, available)
                    for (i in 0 until count * 2) output[i] = ring[(read * 2 + i) % ring.size]
                    read = (read + count) % 8192
                    available -= count
                    ringLowFrames = minOf(ringLowFrames, available)
                }
            }
            if (waited) pcmWaitTimes.record(System.nanoTime() - waitStarted)
            if (generation != audioGeneration) continue
            if (play && !running()) continue
            if (play) requestRender()
            output.fill(0f, count * 2, output.size)
            if (muted) output.fill(0f)
            if (play) {
                underflowFrames += 512 - count
                if (count < 512) underflowEvents++
            }
            cursor.begin(output.size, count * 2)
            while (cursor.remaining > 0 && !closed && generation == audioGeneration) {
                val sent = track.write(output, cursor.offset, cursor.remaining, AudioTrack.WRITE_BLOCKING)
                if (sent < 0) {
                    audioWriteError = sent
                    ready = false
                    worker.post { failure(IOException("Audio output stopped ($sent).")) }
                    break
                }
                if (sent == 0) {
                    Thread.sleep(1)
                    continue
                }
                cursor.accept(sent)
                acceptedSinceFlush += sent
                outputFrames = cursor.acceptedGeneratedSamples / 2
                submittedFrames = cursor.acceptedSamples / 2
                submittedSinceFlush = acceptedSinceFlush / 2
            }
            if (audioWriteError != 0) break
        }
        track.stop()
        track.release()
    }

    private fun packageBytes(kind: Int) =
        context.assets.open("engines/$kind.fwsc").use { readBounded(it, 4 * 1024 * 1024) }

    private fun saveFile(kind: Int, firmwareId: String? = selectedFirmwareId): File {
        if (firmwareId != null && !firmwareId.matches(Regex("[0-9a-f]{64}")))
            throw IOException("Invalid firmware identity.")
        return File(directory, if (firmwareId != null) "import-$firmwareId.flash"
            else "$kind${if (kind == 6) ".preview.flash" else ".flash"}")
    }

    fun selectedName(): String = if (selectedFirmwareId != null) importedName
        else ENGINE_NAMES.getOrElse(ENGINE_KINDS.indexOf(selectedKind)) { "Imported firmware" }

    fun importedFirmwares(): List<Pair<String, String>> = firmwareLibrary.entries().map { it.id to it.name }

    fun selectFirmware(id: String) {
        message("Loading and compiling firmware…")
        worker.post { load(13, firmwareId = id) }
    }

    fun importFirmware(uri: Uri, name: String) {
        message("Checking and compiling firmware…")
        worker.post {
            try {
                val bytes = context.contentResolver.openInputStream(uri).use {
                    if (it == null) throw IOException("Cannot open firmware file.")
                    readBounded(it, 4 * 1024 * 1024)
                }
                val entry = firmwareLibrary.identify(bytes, name)
                load(13, firmwareId = entry.id, importedBytes = bytes, entry = entry)
            } catch (error: Exception) { failure(error) }
        }
    }

    private fun warmBinary(candidate: Long) {
        val deadline = SystemClock.elapsedRealtime() + 15000
        val pcm = FloatArray(1024)
        var frames = 0
        while (frames < 8192 && SystemClock.elapsedRealtime() < deadline) {
            val count = NativeEngine.render(candidate, pcm, 512)
            if (count !in 0..512 || (0 until count * 2).any { !pcm[it].isFinite() })
                throw IOException("Firmware produced invalid audio during startup.")
            frames += count
        }
        if (frames < 8192) throw IOException("Firmware did not start audio within 15 seconds. It may require unsupported hardware.")
        if (NativeEngine.jitInstructions(candidate) == 0L)
            throw IOException("ARM64 compilation is unavailable for this firmware or device.")
    }

    private fun load(kind: Int, reset: Boolean = false, firmwareId: String? = null,
                     importedBytes: ByteArray? = null, entry: FirmwareLibrary.Entry? = null) {
        var candidate = 0L
        val wasReady = ready
        ready = false
        clearRing()
        try {
            if (machine != 0L && !reset) checkpoint(wait = true)
            val imported = if (firmwareId != null) entry ?: firmwareLibrary.entry(firmwareId) else null
            val bytes = if (firmwareId != null) importedBytes ?: firmwareLibrary.bytes(firmwareId) else packageBytes(kind)
            candidate = if (firmwareId != null) NativeEngine.createBinary(bytes, true) else NativeEngine.create(bytes, kind)
            val save = saveFile(kind, firmwareId)
            if (!reset && save.isFile && !NativeEngine.restore(candidate, Files.readAllBytes(save.toPath())))
                throw IOException("Saved instrument data could not be restored. It has been kept.")
            NativeEngine.volume(candidate, masterValue)
            if (firmwareId != null) warmBinary(candidate)
            if (imported != null && importedBytes != null) firmwareLibrary.store(imported, bytes)
            // Commit the fresh state before replacing the running instance. Do not
            // checkpoint the old instance over it during a reset.
            if (reset) persistence.save(save, NativeEngine.flash(candidate), wait = true)
            if (machine != 0L) {
                NativeEngine.release(machine)
                NativeEngine.destroy(machine)
            }
            machine = candidate
            candidate = 0
            selectedKind = kind
            selectedFirmwareId = firmwareId
            if (imported != null) importedName = imported.name
            compiledInstructions = NativeEngine.jitInstructions(machine)
            runtimeBackend = NativeEngine.backend(machine)
            prefs.edit().putInt("instrument", kind).putString("firmwareId", firmwareId).apply()
            clearRing()
            ready = true
            lastSave = SystemClock.uptimeMillis()
            lastSnapshot = lastSave
            peak = 0.0
            publishFrame()
            requestRender()
            message(if (firmwareId != null) "$importedName · ARM64 compiled" else "")
        } catch (error: Exception) {
            if (candidate != 0L) NativeEngine.destroy(candidate)
            ready = wasReady
            failure(error)
        }
    }

    private fun publishFrame() {
        if (machine == 0L) return
        val started = System.nanoTime()
        NativeEngine.snapshot(machine, snapshotPixels, snapshotLeds)
        val pixelsChanged = !pixels.contentEquals(snapshotPixels)
        if (!pixelsChanged && leds.contentEquals(snapshotLeds)) {
            snapshotTimes.record(System.nanoTime() - started)
            return
        }
        if (pixelsChanged) pixels = snapshotPixels.clone()
        leds = snapshotLeds.clone()
        snapshotTimes.record(System.nanoTime() - started)
        if (framePosted.compareAndSet(false, true)) main.post {
            framePosted.set(false)
            frameListener?.onFrame()
        }
    }

    private fun checkpoint(wait: Boolean = false, periodic: Boolean = false) {
        if (machine == 0L) return
        // Capture the destination with the bytes, before another instrument can be selected.
        val file = saveFile(selectedKind, selectedFirmwareId)
        val started = System.nanoTime()
        val bytes = NativeEngine.flash(machine)
        checkpointTimes.record(System.nanoTime() - started)
        persistence.save(file, bytes, wait, periodic)
    }

    fun selectEngine(kind: Int) {
        if (kind !in ENGINE_KINDS) return
        message("Loading instrument…")
        worker.post { load(kind) }
    }

    fun resetFirmware(kind: Int, firmwareId: String? = selectedFirmwareId) {
        if (kind !in ENGINE_KINDS && kind != 13) return
        worker.post {
            // The confirmation belongs to this firmware, even if another load
            // was queued while its dialog was visible.
            if (selectedKind == kind && selectedFirmwareId == firmwareId) load(kind, reset = true, firmwareId = firmwareId)
        }
    }

    fun key(id: Int, down: Boolean) {
        val enqueued = System.nanoTime()
        worker.post {
            if (machine != 0L && running()) {
                inputQueueTimes.record(System.nanoTime() - enqueued)
                NativeEngine.key(machine, id, down)
            }
        }
    }

    fun encoder(id: Int, delta: Int) {
        val enqueued = System.nanoTime()
        worker.post {
            if (machine != 0L) {
                inputQueueTimes.record(System.nanoTime() - enqueued)
                NativeEngine.encoder(machine, id, delta.coerceIn(-32, 32))
            }
        }
    }

    fun setMaster(value: Int) {
        masterValue = value.coerceIn(0, 1023)
        prefs.edit().putInt("master", masterValue).apply()
        worker.post { if (machine != 0L) NativeEngine.volume(machine, masterValue) }
    }

    fun getMaster() = masterValue

    fun releaseAll() {
        worker.post { if (machine != 0L) NativeEngine.release(machine) }
    }

    fun setPaused(value: Boolean) {
        paused = value
        worker.post {
            clearRing()
            if (machine != 0L) {
                NativeEngine.release(machine)
                try {
                    checkpoint()
                } catch (error: Exception) {
                    failure(error)
                }
            }
            if (!value) requestRender()
        }
        notifyStatus()
    }

    fun setMuted(value: Boolean) {
        muted = value
        prefs.edit().putBoolean("muted", value).apply()
        notifyStatus()
    }

    fun setForeground(value: Boolean) {
        foreground = value
        worker.post {
            clearRing()
            if (machine != 0L) {
                NativeEngine.release(machine)
                try {
                    checkpoint()
                } catch (error: Exception) {
                    failure(error)
                }
            }
            if (value) requestRender()
        }
    }

    fun listProjects(): Array<String> =
        projects
            .listFiles { _, name -> name.endsWith(".fm1project") || name.endsWith(".fm1backup") }
            ?.sortedByDescending { it.lastModified() }
            ?.map { it.name }
            ?.toTypedArray() ?: emptyArray()

    fun saveProject(name: String, backup: Boolean) {
        worker.post {
            try {
                requireSloop()
                val snapshot = ProjectSnapshot.capture(machine, masterValue)
                val file =
                    File(
                        projects,
                        "${safeName(name)}-${System.currentTimeMillis()}${if (backup) ".fm1backup" else ".fm1project"}",
                    )
                atomicWrite(file, snapshot.envelope(name, backup))
                message(if (backup) "Workspace backup saved." else "Project copy saved.")
            } catch (error: Exception) {
                failure(error)
            }
        }
    }

    fun openProject(filename: String) {
        worker.post {
            try {
                requireSloop()
                applyProject(Files.readAllBytes(projectFile(filename).toPath()), filename)
            } catch (error: Exception) {
                failure(error)
            }
        }
    }

    fun exportProject(filename: String): ByteArray =
        Files.readAllBytes(projectFile(filename).toPath())

    fun importProject(bytes: ByteArray, filename: String) {
        worker.post {
            try {
                requireSloop()
                ProjectSnapshot.fromEnvelope(bytes, filename)
                val target =
                    File(
                        projects,
                        "${UUID.randomUUID()}${if (filename.lowercase(Locale.ROOT).endsWith(".fm1backup")) ".fm1backup" else ".fm1project"}",
                    )
                atomicWrite(target, bytes)
                message("File imported. Open the saved copy when ready.")
            } catch (error: Exception) {
                failure(error)
            }
        }
    }

    private fun applyProject(bytes: ByteArray, filename: String) {
        var next = ProjectSnapshot.fromEnvelope(bytes, filename)
        val current = ProjectSnapshot.capture(machine, masterValue)
        if (!filename.lowercase(Locale.ROOT).endsWith(".fm1backup")) next = next.overlay(current)
        var candidate = next.create(packageBytes(2))
        try {
            atomicWrite(
                File(projects, "Recovery-${System.currentTimeMillis()}.fm1backup"),
                current.envelope("Recovery before opening", true),
            )
            persistence.save(saveFile(2), NativeEngine.flash(candidate), wait = true)
            NativeEngine.release(machine)
            NativeEngine.destroy(machine)
            machine = candidate
            candidate = 0
            masterValue = next.master
            prefs.edit().putInt("master", masterValue).apply()
            clearRing()
            publishFrame()
            message("Opened as a copy. Previous work is in a recovery backup.")
        } finally {
            if (candidate != 0L) NativeEngine.destroy(candidate)
        }
    }

    fun importSample(
        source: SampleAudio.Source,
        start: Double,
        end: Double,
        normalize: Boolean,
        root: Int,
        slot: Int,
    ) {
        worker.post {
            try {
                requireSloop()
                val current = ProjectSnapshot.capture(machine, masterValue)
                val next = current.copy()
                next.parts["sample$slot"] = SampleAudio.encode(source, start, end, normalize, root)
                var candidate = next.create(packageBytes(2))
                try {
                    atomicWrite(
                        File(projects, "Recovery-${System.currentTimeMillis()}.fm1backup"),
                        current.envelope("Before sample import", true),
                    )
                    persistence.save(saveFile(2), NativeEngine.flash(candidate), wait = true)
                    NativeEngine.destroy(machine)
                    machine = candidate
                    candidate = 0
                    clearRing()
                    publishFrame()
                    message("Sample imported to USR${slot + 1}. Recovery backup saved.")
                } finally {
                    if (candidate != 0L) NativeEngine.destroy(candidate)
                }
            } catch (error: Exception) {
                failure(error)
            }
        }
    }

    fun cancelExport() {
        exportThread?.interrupt()
    }

    fun exportAudio(wholeSong: Boolean, seconds: Int, tail: Int, callback: ExportCallback) {
        worker.post {
            try {
                requireSloop()
                if (exportThread?.isAlive == true)
                    throw IOException("Another audio export is still running.")
                val snapshot = ProjectSnapshot.capture(machine, masterValue)
                val data = packageBytes(2)
                exportThread =
                    Thread(
                            {
                                try {
                                    val file =
                                        AudioExport.render(
                                            context,
                                            snapshot,
                                            data,
                                            wholeSong,
                                            seconds,
                                            tail,
                                        )
                                    main.post { callback.complete(file, null) }
                                } catch (error: Exception) {
                                    main.post { callback.complete(null, error) }
                                }
                            },
                            "FM1 offline render",
                        )
                        .also { it.start() }
            } catch (error: Exception) {
                main.post { callback.complete(null, error) }
            }
        }
    }

    private fun requireSloop() {
        if (machine == 0L || selectedKind != 2)
            throw IOException("Project files require SLOOP 2.8.")
    }

    private fun projectFile(name: String): File =
        File(projects, name).also {
            if (it.canonicalFile.parentFile != projects.canonicalFile)
                throw IOException("Invalid project filename.")
        }

    fun close() {
        foreground = false
        synchronized(ringLock) { ringLock.notifyAll() }
        cancelExport()
        worker.post {
            try {
                checkpoint(wait = true)
            } catch (error: Exception) {
                failure(error)
            }
            if (machine != 0L) {
                NativeEngine.destroy(machine)
                machine = 0
            }
            closed = true
            synchronized(ringLock) { ringLock.notifyAll() }
            persistence.close()
            workerThread.quitSafely()
        }
    }

    companion object {
        @JvmField
        val ENGINE_NAMES =
            arrayOf(
                "SLOOP 2.8",
                "Felucca 1.6.1",
                "X0X 1.1.0",
                "Melodee 1.4.3",
                "FM1 Open V13 Preview",
                "Skiff 1.4",
                "FoMni 0.4",
                "Jangada 1.0.1",
                "Clipper 1.1 Experimental",
                "M-VAVE Stock V15 (ARM64 JIT)",
                "FM-1+VA 102 (ARM64 JIT)",
                "ChoralRoot 0.26.1",
                "GHOULBOX 2.3.2",
                "JIANT 0.9.9",
            )
        @JvmField val ENGINE_KINDS = intArrayOf(2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 15, 16)

        fun safeName(value: String) =
            value.trim().replace(Regex("[^a-zA-Z0-9 _.-]"), "_").ifEmpty { "Project" }.take(80)

        fun readBounded(input: InputStream, maximum: Int): ByteArray {
            val out = ByteArrayOutputStream()
            val buffer = ByteArray(16384)
            while (true) {
                val count = input.read(buffer)
                if (count == -1) break
                if (out.size() + count > maximum) throw IOException("The file is too large.")
                out.write(buffer, 0, count)
            }
            return out.toByteArray()
        }

        fun atomicWrite(file: File, bytes: ByteArray) {
            val temporary = File(file.path + ".tmp")
            FileOutputStream(temporary).use {
                it.write(bytes)
                it.fd.sync()
            }
            if (!temporary.renameTo(file)) throw IOException("Could not finish saving the file.")
        }
    }
}
