package org.fm1.android

/** Fixed storage on the producer/output paths; sorting happens only when diagnostics are read. */
internal class AudioTiming(private val capacity: Int = 512) {
    private val samples = LongArray(capacity)
    private var next = 0
    private var size = 0
    private var count = 0L
    private var maximum = 0L
    private var totalNanos = 0L

    @Synchronized fun record(nanos: Long) {
        val value = nanos.coerceAtLeast(0)
        samples[next] = value
        next = (next + 1) % capacity
        size = minOf(size + 1, capacity)
        count++
        maximum = maxOf(maximum, value)
        totalNanos += value
    }

    data class Snapshot(val count: Long, val windowCount: Int, val p95Micros: Double, val maxMicros: Double, val totalNanos: Long)

    @Synchronized fun snapshot(): Snapshot {
        val sorted = samples.copyOf(size).apply { sort() }
        val p95 = if (size == 0) 0L else sorted[((size * 95 + 99) / 100) - 1]
        return Snapshot(count, size, p95 / 1000.0, maximum / 1000.0, totalNanos)
    }
}
