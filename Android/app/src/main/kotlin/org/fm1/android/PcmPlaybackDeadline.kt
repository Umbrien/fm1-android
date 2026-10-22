package org.fm1.android

/** Output-thread accounting for AudioTrack's unsigned 32-bit playback head. */
internal class PcmPlaybackDeadline(
    private val sampleRate: Int,
    private val capacityFrames: Int,
    private val cushionFrames: Int,
) {
    private var lastHead = 0L
    private var playedFrames = 0L
    private var valid = true

    init { require(sampleRate > 0 && capacityFrames > 0 && cushionFrames >= 0) }

    fun reset() {
        lastHead = 0
        playedFrames = 0
        valid = true
    }

    /**
     * Capture once per output chunk. The caller must retain this absolute deadline
     * across notifications, so a stale playback head cannot prolong the wait.
     * An unexpected reset or impossible position disables waiting until a flush.
     */
    fun deadlineNanos(acceptedFrames: Long, rawHead: Long, nowNanos: Long): Long {
        if (!valid) return nowNanos
        val head = rawHead and 0xffff_ffffL
        val delta = (head - lastHead) and 0xffff_ffffL
        if (acceptedFrames < playedFrames || delta > acceptedFrames - playedFrames) {
            valid = false
            return nowNanos
        }
        playedFrames += delta
        lastHead = head
        val queued = minOf(acceptedFrames - playedFrames, capacityFrames.toLong())
        val spare = (queued - cushionFrames).coerceAtLeast(0)
        return nowNanos + spare * 1_000_000_000L / sampleRate
    }
}
