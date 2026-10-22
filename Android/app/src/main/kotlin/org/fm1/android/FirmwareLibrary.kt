package org.fm1.android

import android.content.Context
import java.io.File
import java.io.IOException
import java.security.MessageDigest

/** Original packages and saves use content identities, never user-supplied paths. */
internal class FirmwareLibrary(context: Context) {
    data class Entry(val id: String, val name: String)
    private val directory = File(context.filesDir, "Firmware").apply { mkdirs() }

    fun identify(bytes: ByteArray, name: String) = Entry(
        MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) },
        name.replace(Regex("[\\p{Cntrl}]"), "").trim().take(120).ifEmpty { "Imported firmware" },
    )

    private fun file(id: String, suffix: String): File {
        if (!id.matches(Regex("[0-9a-f]{64}"))) throw IOException("Invalid firmware identity.")
        return File(directory, "$id.$suffix")
    }

    fun entries(): List<Entry> = directory.listFiles()?.filter {
        it.name.matches(Regex("[0-9a-f]{64}\\.fwsc"))
    }?.map { entry(it.name.removeSuffix(".fwsc")) }?.sortedBy { it.name } ?: emptyList()

    fun entry(id: String): Entry {
        if (!file(id, "fwsc").isFile) throw IOException("Imported firmware is missing.")
        val label = file(id, "name")
        return Entry(id, if (label.isFile) label.inputStream().use {
            EngineController.readBounded(it, 1024).toString(Charsets.UTF_8)
        } else "Firmware ${id.take(8)}")
    }

    fun bytes(id: String): ByteArray {
        val bytes = file(id, "fwsc").inputStream().use { EngineController.readBounded(it, 4 * 1024 * 1024) }
        if (identify(bytes, "").id != id) throw IOException("Saved firmware package has changed.")
        return bytes
    }

    fun store(entry: Entry, bytes: ByteArray) {
        if (identify(bytes, entry.name).id != entry.id) throw IOException("Firmware identity mismatch.")
        EngineController.atomicWrite(file(entry.id, "name"), entry.name.toByteArray())
        EngineController.atomicWrite(file(entry.id, "fwsc"), bytes)
    }
}
