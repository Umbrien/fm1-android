package org.fm1.android

import java.io.File
import java.io.IOException
import java.nio.file.Files
import java.util.Collections
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Pure JVM checks: no Android process, firmware, phone, or user save is touched. */
fun main() {
    val cursor = PcmWriteCursor()
    cursor.begin(8, 6)
    val source = floatArrayOf(1f, 2f, 3f, 4f, 5f, 6f, 0f, 0f)
    val sent = mutableListOf<Float>()
    for (partial in intArrayOf(1, 2, 1, 4)) {
        sent += source.slice(cursor.offset until cursor.offset + partial)
        cursor.accept(partial)
    }
    check(sent == source.toList() && cursor.remaining == 0)
    check(cursor.acceptedSamples == 8L && cursor.acceptedGeneratedSamples == 6L)
    // A flush can abandon an unaccepted suffix; it must never count as accepted output.
    cursor.begin(8, 8)
    cursor.accept(3)
    cursor.begin(8, 0)
    cursor.accept(8)
    check(cursor.acceptedSamples == 19L && cursor.acceptedGeneratedSamples == 9L)

    val playback = PcmPlaybackDeadline(44100, 1772, 882)
    val now = 10_000_000_000L
    check(playback.deadlineNanos(0, 0, now) == now) // Startup cannot wait for nonexistent output.
    val deadline = playback.deadlineNanos(1772, 0, now)
    check(deadline == now + 890L * 1_000_000_000 / 44100)
    // A 20ms render burst can recover while queued PCM still covers the mixer
    // cushion. A later completion must use silence rather than extend the wait.
    check(now + 20_000_000 < deadline)
    check(now + 21_000_000 > deadline)
    check(playback.deadlineNanos(1772, 1000, now) == now) // Preserve the mixer cushion.
    check(playback.deadlineNanos(1772, 0, now) == now) // Unexpected reset disables waiting.
    check(playback.deadlineNanos(3544, 100, now) == now)
    playback.reset()
    check(playback.deadlineNanos(1772, 0, now) == deadline)
    // A genuine wrap is accepted only when cumulative submitted output can cover it.
    playback.reset()
    check(playback.deadlineNanos(0xffff_ffffL, 0xffff_fff0L, now) == now)
    check(playback.deadlineNanos(0x1_0000_1000L, 16, now) == deadline)
    playback.reset()
    check(playback.deadlineNanos(100, 101, now) == now)
    check(playback.deadlineNanos(1772, 0, now) == now) // Impossible head cannot create extra time.

    val timing = AudioTiming(4)
    for (value in longArrayOf(1000, 2000, 3000, 4000, 5000)) timing.record(value)
    check(timing.snapshot() == AudioTiming.Snapshot(5, 4, 5.0, 5.0, 15000))
    // Cumulative totals retain evicted samples and clamp invalid negative durations.
    timing.record(-1000)
    check(timing.snapshot() == AudioTiming.Snapshot(6, 4, 5.0, 5.0, 15000))

    val directory = Files.createTempDirectory("fm1-audio-host-").toFile()
    val entered = CountDownLatch(1)
    val release = CountDownLatch(1)
    val writes = Collections.synchronizedList(mutableListOf<String>())
    val errors = Collections.synchronizedList(mutableListOf<Exception>())
    val owner = Thread.currentThread()
    val persistence = OrderedPersistence({ file, bytes ->
        check(Thread.currentThread() !== owner)
        if (bytes[0] == 1.toByte()) {
            entered.countDown()
            check(release.await(5, TimeUnit.SECONDS))
        }
        if (bytes[0] == 9.toByte()) throw IOException("injected write failure")
        writes += "${file.name}:${bytes[0]}"
        file.writeBytes(bytes)
    }, { errors += it })
    try {
        val first = File(directory, "first.flash")
        val other = File(directory, "other.flash")
        persistence.save(first, byteArrayOf(1), periodicSave = true)
        check(entered.await(5, TimeUnit.SECONDS) && persistence.periodicPending())
        persistence.save(other, byteArrayOf(2))
        persistence.save(first, byteArrayOf(3)) // Reset must win over the older periodic save.
        release.countDown()
        persistence.save(first, byteArrayOf(3)) // Duplicate background checkpoint is suppressed.
        persistence.save(other, byteArrayOf(4), wait = true) // Explicit queue barrier.
        check(writes.toList() == listOf("first.flash:1", "other.flash:2", "first.flash:3", "other.flash:4"))
        check(first.readBytes().contentEquals(byteArrayOf(3)) && other.readBytes().contentEquals(byteArrayOf(4)))
        persistence.save(first, byteArrayOf(9))
        persistence.save(other, byteArrayOf(4), wait = true)
        check(errors.size == 1 && first.readBytes().contentEquals(byteArrayOf(3)))
        var propagated = false
        try { persistence.save(first, byteArrayOf(9), wait = true) }
        catch (_: IOException) { propagated = true }
        check(propagated)
        persistence.save(first, byteArrayOf(5), wait = true)
        check(first.readBytes().contentEquals(byteArrayOf(5)))
        first.writeBytes(byteArrayOf(0))
        persistence.save(first, byteArrayOf(5), wait = true)
        check(first.readBytes().contentEquals(byteArrayOf(5))) // Reset repairs identical cached bytes.
    } finally {
        release.countDown()
        persistence.close()
        directory.deleteRecursively()
    }
    println("Audio host checks passed: partial writes, playback deadlines/wrap/reset, accepted counters, timing window, ordered identity saves, reset barrier, IO failures and recovery")
}
