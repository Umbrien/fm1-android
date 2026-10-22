package org.fm1.android

/** AudioTrack reports samples, which may end between stereo frames on a partial write. */
internal class PcmWriteCursor {
    var offset = 0
        private set
    private var length = 0
    private var generated = 0
    var acceptedSamples = 0L
        private set
    var acceptedGeneratedSamples = 0L
        private set
    val remaining get() = length - offset

    fun begin(samples: Int, generatedSamples: Int) {
        require(samples >= 0 && generatedSamples in 0..samples)
        length = samples
        generated = generatedSamples
        offset = 0
    }

    fun accept(samples: Int) {
        require(samples in 1..remaining)
        acceptedGeneratedSamples += minOf(samples, (generated - offset).coerceAtLeast(0))
        acceptedSamples += samples
        offset += samples
    }
}
