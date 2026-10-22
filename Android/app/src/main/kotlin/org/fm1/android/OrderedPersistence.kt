package org.fm1.android

import java.io.File
import java.util.concurrent.Executors
import java.util.concurrent.Future

/** One write order for periodic saves and explicit switch/reset/close barriers. */
internal class OrderedPersistence(
    private val write: (File, ByteArray) -> Unit,
    private val failed: (Exception) -> Unit,
) {
    private val executor = Executors.newSingleThreadExecutor { task -> Thread(task, "FM1 durable storage") }
    private data class Saved(val path: String, val bytes: ByteArray)
    @Volatile private var saved: Saved? = null
    private var periodic: Future<*>? = null
    val writeTimes = AudioTiming()

    // Called only by the native owner. Avoid taking another large snapshot while IO is pending.
    fun periodicPending() = periodic?.isDone == false

    fun save(file: File, bytes: ByteArray, wait: Boolean = false, periodicSave: Boolean = false) {
        // Ownership of these immutable bytes transfers to this queue. All writes, including
        // explicit reset writes, use the same executor so an older save cannot win a race.
        val future = executor.submit {
            val previous = saved
            // Explicit barriers always rewrite: reset must repair even a missing or damaged
            // destination. Only background checkpoints may reuse a successful prior write.
            if (!wait && file.isFile && previous?.path == file.path && previous.bytes.contentEquals(bytes)) return@submit
            val started = System.nanoTime()
            try {
                write(file, bytes)
                saved = Saved(file.path, bytes)
            } catch (error: Exception) {
                if (!wait) failed(error)
                throw error
            } finally {
                writeTimes.record(System.nanoTime() - started)
            }
        }
        if (periodicSave) periodic = future
        if (wait) {
            try { future.get() }
            catch (error: java.util.concurrent.ExecutionException) {
                throw (error.cause as? Exception ?: error)
            }
        }
    }

    fun close() { executor.shutdown() }
}
