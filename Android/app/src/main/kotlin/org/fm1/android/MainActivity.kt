package org.fm1.android

import android.app.*
import android.content.*
import android.graphics.*
import android.media.MediaPlayer
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.view.*
import android.widget.*
import androidx.appcompat.app.AppCompatActivity
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.content.res.AppCompatResources
import com.google.android.material.button.MaterialButton
import com.google.android.material.checkbox.MaterialCheckBox
import com.google.android.material.color.DynamicColors
import com.google.android.material.color.MaterialColors
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import com.google.android.material.progressindicator.CircularProgressIndicator
import com.google.android.material.slider.Slider
import com.google.android.material.textfield.MaterialAutoCompleteTextView
import com.google.android.material.textfield.TextInputEditText
import com.google.android.material.textfield.TextInputLayout
import com.google.android.material.textview.MaterialTextView
import java.io.*
import kotlin.math.*

/** Landscape native shell; storage access is explicitly initiated through Android's picker. */
class MainActivity : AppCompatActivity(), EngineController.Listener {
    private lateinit var engine: EngineController
    private lateinit var panel: PanelView
    private lateinit var column: LinearLayout
    private lateinit var toolbar: LinearLayout
    private lateinit var root: FrameLayout
    private lateinit var firmware: MaterialButton
    private lateinit var transport: MaterialButton
    private lateinit var mute: MaterialButton
    private lateinit var folder: MaterialButton
    private lateinit var collapse: MaterialButton
    private lateinit var preferences: SharedPreferences
    private var finish = 0
    private var collapsed = false
    private var foreground = false
    private var exportBytes: ByteArray? = null
    private var exportFile: File? = null
    private var audition: MediaPlayer? = null
    private var finishDialog: AlertDialog? = null
    private var zoom: View? = null

    companion object {
        private const val FIRMWARE = 104
        private const val SAMPLE = 103
        private const val IMPORT = 101
        private const val EXPORT = 102
    }

    private fun color(attribute: Int) = MaterialColors.getColor(this, attribute, "FM1 theme")
    private fun accent() = color(androidx.appcompat.R.attr.colorPrimary)

    override fun onCreate(state: Bundle?) {
        DynamicColors.applyToActivityIfAvailable(this)
        super.onCreate(state)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        window.decorView.systemUiVisibility =
            View.SYSTEM_UI_FLAG_FULLSCREEN or
                View.SYSTEM_UI_FLAG_HIDE_NAVIGATION or
                View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY or
                View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN or
                View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION or
                View.SYSTEM_UI_FLAG_LAYOUT_STABLE
        preferences = getSharedPreferences("panel", MODE_PRIVATE)
        finish = preferences.getInt("finish", 0).coerceIn(0, 5)
        collapsed = preferences.getBoolean("collapsed", false)
        engine = EngineController(this)
        engine.setListener(this)
        root = FrameLayout(this)
        root.setBackgroundColor(color(com.google.android.material.R.attr.colorSurface))
        root.setPadding(dp(8f), dp(4f), dp(6f), dp(2f))
        column = LinearLayout(this)
        column.orientation = LinearLayout.VERTICAL
        val cp = FrameLayout.LayoutParams(-1, -1)
        cp.rightMargin = dp(48f)
        root.addView(column, cp)
        toolbar = LinearLayout(this)
        toolbar.gravity = Gravity.CENTER_VERTICAL
        toolbar.setPadding(dp(8f), 0, dp(8f), 0)
        column.addView(toolbar, LinearLayout.LayoutParams(-1, dp(48f)))
        firmware = MaterialButton(this, null, R.attr.systemButtonStyle)
        firmware.icon = AppCompatResources.getDrawable(this, R.drawable.ic_expand_more)
        firmware.iconGravity = MaterialButton.ICON_GRAVITY_TEXT_END
        firmware.setOnClickListener { library() }
        firmware.setSingleLine(true)
        firmware.ellipsize = android.text.TextUtils.TruncateAt.END
        firmware.gravity = Gravity.LEFT or Gravity.CENTER_VERTICAL
        toolbar.addView(firmware, LinearLayout.LayoutParams(0, -1, 1f))
        transport =
            iconButton(R.drawable.ic_pause, "Pause instrument") {
                engine.setPaused(!engine.paused)
                refreshChrome()
            }
        toolbar.addView(transport, LinearLayout.LayoutParams(dp(48f), -1))
        mute =
            iconButton(R.drawable.ic_volume_up, "Mute output") {
                engine.setMuted(!engine.muted)
                refreshChrome()
            }
        toolbar.addView(mute, LinearLayout.LayoutParams(dp(48f), -1))
        mute.contentDescription = "Mute or unmute output"
        val colors = iconButton(R.drawable.ic_palette, "Panel color") { chooseFinish() }
        colors.contentDescription = "Panel color"
        toolbar.addView(colors, LinearLayout.LayoutParams(dp(48f), -1))
        val info = iconButton(R.drawable.ic_info, "About FM–1") { about() }
        info.contentDescription = "About FM–1"
        toolbar.addView(info, LinearLayout.LayoutParams(dp(48f), -1))
        val reset = iconButton(R.drawable.ic_restart, "Reset firmware") { confirmFirmwareReset() }
        reset.contentDescription = "Reset firmware"
        toolbar.addView(reset, LinearLayout.LayoutParams(dp(48f), -1))
        panel = PanelView(this)
        panel.setMasterIndicator(engine.getMaster())
        column.addView(panel, LinearLayout.LayoutParams(-1, 0, 1f))
        panel.setActions(
            object : PanelView.Actions {
                override fun key(id: Int, down: Boolean) = engine.key(id, down)

                override fun encoder(id: Int, steps: Int) = engine.encoder(id, steps)

                override fun master(value: Int) = engine.setMaster(value)

                override fun enlarge() = enlargeDisplay()
            }
        )
        val side = LinearLayout(this)
        side.orientation = LinearLayout.VERTICAL
        val sp = FrameLayout.LayoutParams(dp(48f), -2, Gravity.RIGHT or Gravity.TOP)
        sp.topMargin = dp(12f)
        root.addView(side, sp)
        collapse =
            iconButton(R.drawable.ic_expand_less, "Hide top controls") {
                collapsed = !collapsed
                preferences.edit().putBoolean("collapsed", collapsed).apply()
                applyFinish()
            }
        side.addView(collapse, LinearLayout.LayoutParams(-1, dp(48f)))
        folder = iconButton(R.drawable.ic_folder, "Projects and files") { files() }
        side.addView(folder, LinearLayout.LayoutParams(-1, dp(48f)))
        setContentView(root)
        root.setOnApplyWindowInsetsListener { _, insets ->
            root.setPadding(
                max(dp(8f), insets.systemWindowInsetLeft),
                dp(4f),
                max(dp(6f), insets.systemWindowInsetRight),
                dp(2f),
            )
            insets
        }
        root.addOnLayoutChangeListener { _, _, _, _, _, _, _, _, _ ->
            panel.setReferenceHeight(
                (root.height - root.paddingTop - root.paddingBottom - dp(48f)).toFloat()
            )
        }
        applyFinish()
    }

    private fun dp(n: Float) = (n * resources.displayMetrics.density).roundToInt()

    private fun iconButton(icon: Int, label: String, click: View.OnClickListener) =
        MaterialButton(this, null, R.attr.systemButtonStyle).apply {
            this.icon = AppCompatResources.getDrawable(this@MainActivity, icon)
            iconGravity = MaterialButton.ICON_GRAVITY_TEXT_START
            iconPadding = 0
            iconSize = dp(24f)
            minWidth = dp(48f)
            minimumWidth = dp(48f)
            setPadding(dp(12f), 0, dp(12f), 0)
            insetTop = 0
            insetBottom = 0
            contentDescription = label
            tooltipText = label
            setOnClickListener(click)
        }

    private fun applyFinish() {
        panel.setFinish(finish)
        toolbar.visibility = if (collapsed) View.GONE else View.VISIBLE
        collapse.setIconResource(if (collapsed) R.drawable.ic_expand_more else R.drawable.ic_expand_less)
        collapse.contentDescription = if (collapsed) "Show top controls" else "Hide top controls"
        collapse.tooltipText = collapse.contentDescription
        refreshChrome()
    }

    private fun refreshChrome() {
        if (!::firmware.isInitialized) return
        val name = engine.selectedName()
        firmware.text = name
        firmware.contentDescription = "$name, select firmware"
        transport.setIconResource(if (engine.paused) R.drawable.ic_play_arrow else R.drawable.ic_pause)
        transport.contentDescription = if (engine.paused) "Resume instrument" else "Pause instrument"
        transport.tooltipText = transport.contentDescription
        mute.setIconResource(if (engine.muted) R.drawable.ic_volume_off else R.drawable.ic_volume_up)
        mute.contentDescription = if (engine.muted) "Unmute output" else "Mute output"
        mute.tooltipText = mute.contentDescription
        folder.visibility = if (engine.selectedKind == 2) View.VISIBLE else View.GONE
    }

    override fun onFrame() {
        if (::panel.isInitialized) panel.updateFrame(engine.pixels, engine.leds)
        zoom?.invalidate()
    }

    override fun onStatus() {
        if (::panel.isInitialized) panel.setMasterIndicator(engine.getMaster())
        refreshChrome()
        if (engine.status.isNotEmpty())
            Toast.makeText(this, engine.status, Toast.LENGTH_LONG).show()
    }

    private fun library() {
        try {
            val imported = engine.importedFirmwares()
            val names = EngineController.ENGINE_NAMES + imported.map { "${it.second} · ${it.first.take(8)}" }
            val selected = if (engine.selectedFirmwareId == null) EngineController.ENGINE_KINDS.indexOf(engine.selectedKind)
                else imported.indexOfFirst { it.first == engine.selectedFirmwareId }.let {
                    if (it < 0) -1 else EngineController.ENGINE_KINDS.size + it
                }
            MaterialAlertDialogBuilder(this)
                .setTitle("Instruments")
                .setSingleChoiceItems(names, selected) { dialog, i ->
                    dialog.dismiss()
                    panel.releaseTouches()
                    if (i < EngineController.ENGINE_KINDS.size) engine.selectEngine(EngineController.ENGINE_KINDS[i])
                    else engine.selectFirmware(imported[i - EngineController.ENGINE_KINDS.size].first)
                }
                .setNeutralButton("Open .fwsc…") { _, _ ->
                    startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT)
                        .setType("*/*").addCategory(Intent.CATEGORY_OPENABLE), FIRMWARE)
                }
                .setPositiveButton("Audio diagnostics") { _, _ -> audioDiagnostics() }
                .setNegativeButton("Close", null)
                .show()
        } catch (e: Exception) { error(e) }
    }

    private fun audioDiagnostics() {
        val snapshot = engine.audioDiagnostics().put("instrument", engine.selectedName()).toString(2)
        val text = TextView(this).apply {
            setPadding(dp(16f), dp(8f), dp(16f), dp(8f))
            typeface = Typeface.MONOSPACE
            textSize = 12f
            setTextIsSelectable(true)
            text = snapshot
        }
        MaterialAlertDialogBuilder(this)
            .setTitle("Audio diagnostics")
            .setView(ScrollView(this).apply { addView(text) })
            .setPositiveButton("Copy") { _, _ ->
                (getSystemService(CLIPBOARD_SERVICE) as ClipboardManager)
                    .setPrimaryClip(ClipData.newPlainText("FM1 audio diagnostics", snapshot))
            }
            .setNeutralButton("Refresh") { _, _ -> audioDiagnostics() }
            .setNegativeButton("Close", null)
            .show()
    }

    private fun confirmFirmwareReset() {
        val kind = engine.selectedKind
        val name = engine.selectedName()
        val firmwareId = engine.selectedFirmwareId
        MaterialAlertDialogBuilder(this)
            .setTitle("Reset $name?")
            .setMessage(
                "Restart this firmware from a fresh boot and erase all its current and " +
                    "autosaved emulator state, including edits, songs, patterns, and samples. " +
                    "This cannot be undone.\n\n" +
                    "Saved project files and other firmware states will be kept."
            )
            .setNegativeButton("Cancel", null)
            .setPositiveButton("Reset firmware") { _, _ ->
                panel.releaseTouches()
                engine.resetFirmware(kind, firmwareId)
            }
            .show()
    }

    private fun chooseFinish() {
        finishDialog = MaterialAlertDialogBuilder(this)
            .setTitle("Panel color")
            .setSingleChoiceItems(PanelView.FINISH_NAMES, finish) { dialog, index ->
                finish = index
                preferences.edit().putInt("finish", finish).apply()
                applyFinish()
                dialog.dismiss()
            }
            .setNegativeButton("Close", null)
            .show()
    }

    private fun enlargeDisplay() {
        zoom =
            object : View(this) {
                val p = Paint()
                val bmp = Bitmap.createBitmap(240, 240, Bitmap.Config.ARGB_8888)

                override fun onDraw(c: Canvas) {
                    c.drawColor(Color.BLACK)
                    val pixels = engine.pixels
                    if (pixels.size == 57600) {
                        bmp.setPixels(pixels, 0, 240, 0, 0, 240, 240)
                        val s = min(width, height).toFloat()
                        c.drawBitmap(
                            bmp,
                            null,
                            RectF(
                                (width - s) / 2,
                                (height - s) / 2,
                                (width + s) / 2,
                                (height + s) / 2,
                            ),
                            p,
                        )
                    }
                }
            }
        val dialog =
            MaterialAlertDialogBuilder(this)
                .setTitle("Instrument display")
                .setView(zoom)
                .setPositiveButton("Done", null)
                .create()
        dialog.setOnDismissListener { zoom = null }
        dialog.show()
        dialog.window!!.setLayout(
            resources.displayMetrics.widthPixels * 4 / 5,
            resources.displayMetrics.heightPixels * 9 / 10,
        )
    }

    private fun about() {
        MaterialAlertDialogBuilder(this)
            .setTitle("FM–1")
            .setMessage(
                "Unofficial native FM–1 instrument. Twelve source-compiled engines plus ARM64-recompiled firmware. Use Instruments → Open .fwsc to import an FM-1 package. Each imported file has separate saved state.\n\nPlay the 27 keys; drag knobs vertically. Hold function buttons for firmware combinations. Tap the display to enlarge it. MASTER controls the instrument; the speaker control mutes the output.\n\nProjects and samples stay on this device unless you explicitly export them. No account or telemetry. Backgrounding pauses audio.\n\nGPL-3.0-only. Includes fm1-emu (MIT) and source-native engines by their respective authors. Not affiliated with M-VAVE. Imported firmware, Stock V15 and FM-1+VA use on-device ARM64 recompilation with FM-1 hardware emulation. Compatibility and real-time speed depend on the firmware and phone. Portable FM1 Open preview may differ from hardware and does not include the original factory sound bank."
            )
            .setNeutralButton("Credits and licenses") { _, _ -> licenses() }
            .setPositiveButton("Done", null)
            .show()
    }

    private fun licenses() {
        try {
            val names =
                assets.list("notices") ?: throw IOException("License notices are unavailable.")
            names.sort()
            MaterialAlertDialogBuilder(this)
                .setTitle("Credits and licenses")
                .setItems(names) { _, i ->
                    try {
                        val notice =
                            assets.open("notices/" + names[i]).use {
                                String(
                                    EngineController.readBounded(it, 1024 * 1024),
                                    Charsets.UTF_8,
                                )
                            }
                        val text = label(notice)
                        text.setTextIsSelectable(true)
                        text.setPadding(dp(18f), dp(12f), dp(18f), dp(12f))
                        val scroll = ScrollView(this)
                        scroll.addView(text)
                        MaterialAlertDialogBuilder(this)
                            .setTitle(names[i])
                            .setView(scroll)
                            .setPositiveButton("Done", null)
                            .show()
                    } catch (e: Exception) {
                        error(e)
                    }
                }
                .setNegativeButton("Done", null)
                .show()
        } catch (e: Exception) {
            error(e)
        }
    }

    private fun files() {
        val options =
            arrayOf(
                "Save project copy…",
                "Back up workspace…",
                "Saved projects and backups…",
                "Import project or backup…",
                "Samples · import WAV…",
                "Audio · export WAV…",
            )
        MaterialAlertDialogBuilder(this)
            .setTitle("Projects and files · SLOOP")
            .setItems(options) { _, i ->
                when (i) {
                    0,
                    1 -> nameCopy(i == 1)
                    2 -> saved()
                    4 ->
                        startActivityForResult(
                            Intent(Intent.ACTION_OPEN_DOCUMENT)
                                .setType("audio/*")
                                .addCategory(Intent.CATEGORY_OPENABLE),
                            SAMPLE,
                        )
                    5 -> audioExport()
                    else ->
                        startActivityForResult(
                            Intent(Intent.ACTION_OPEN_DOCUMENT)
                                .setType("*/*")
                                .addCategory(Intent.CATEGORY_OPENABLE),
                            IMPORT,
                        )
                }
            }
            .setNegativeButton("Done", null)
            .show()
    }

    private fun nameCopy(backup: Boolean) {
        val (nameField, name) = textField(if (backup) "Workspace backup" else "Project name")
        MaterialAlertDialogBuilder(this)
            .setTitle(if (backup) "Back up workspace" else "Save project copy")
            .setView(formContent(nameField))
            .setMessage(
                if (backup) "Saves sounds, user samples and instrument settings."
                else
                    "Saves the loop, sections, song and shared sounds. Stop the song and finish SONG REC first."
            )
            .setPositiveButton("Save") { _, _ ->
                val value = name.text.toString().trim()
                if (value.isNotEmpty()) engine.saveProject(value, backup)
            }
            .setNegativeButton("Cancel", null)
            .show()
    }

    private fun saved() {
        val names = engine.listProjects()
        if (names.isEmpty()) {
            MaterialAlertDialogBuilder(this)
                .setTitle("Saved work")
                .setMessage("No saved copies yet. Use Save project copy or Back up workspace.")
                .setPositiveButton("Done", null)
                .show()
            return
        }
        MaterialAlertDialogBuilder(this)
            .setTitle("Saved work")
            .setItems(names) { _, i ->
                MaterialAlertDialogBuilder(this)
                    .setTitle(names[i])
                    .setItems(arrayOf("Open…", "Export…")) { _, choice ->
                        if (choice == 0)
                            MaterialAlertDialogBuilder(this)
                                .setTitle("Open this saved copy?")
                                .setMessage(
                                    "Replaces current work. A recovery backup is saved first."
                                )
                                .setPositiveButton("Back up and open") { _, _ ->
                                    engine.openProject(names[i])
                                }
                                .setNegativeButton("Cancel", null)
                                .show()
                        else
                            try {
                                exportBytes = engine.exportProject(names[i])
                                startActivityForResult(
                                    Intent(Intent.ACTION_CREATE_DOCUMENT)
                                        .setType("application/octet-stream")
                                        .addCategory(Intent.CATEGORY_OPENABLE)
                                        .putExtra(Intent.EXTRA_TITLE, names[i]),
                                    EXPORT,
                                )
                            } catch (e: Exception) {
                                error(e)
                            }
                    }
                    .show()
            }
            .setNegativeButton("Close", null)
            .show()
    }

    override fun onActivityResult(request: Int, result: Int, data: Intent?) {
        super.onActivityResult(request, result, data)
        val uri = data?.data
        if (result != RESULT_OK || uri == null) {
            exportBytes = null
            exportFile?.delete()
            exportFile = null
            return
        }
        try {
            when (request) {
                EXPORT -> {
                    contentResolver.openOutputStream(uri).use { out ->
                        if (out == null) throw IOException("Cannot open the selected destination.")
                        val file = exportFile
                        if (file != null) file.inputStream().use { it.copyTo(out, 16384) }
                        else out.write(exportBytes!!)
                    }
                    exportBytes = null
                    exportFile?.delete()
                    exportFile = null
                    Toast.makeText(this, "Export saved", Toast.LENGTH_SHORT).show()
                }
                FIRMWARE -> {
                    panel.releaseTouches()
                    engine.importFirmware(uri, displayName(uri))
                }
                SAMPLE -> loadSample(uri)
                IMPORT -> {
                    val bytes =
                        contentResolver.openInputStream(uri).use { input ->
                            if (input == null) throw IOException("Cannot open the selected file.")
                            ByteArrayOutputStream().use { output ->
                                val buffer = ByteArray(8192)
                                while (true) {
                                    val count = input.read(buffer)
                                    if (count == -1) break
                                    if (output.size() + count > 4 * 1024 * 1024) {
                                        throw IOException("Project file exceeds 4 MiB.")
                                    }
                                    output.write(buffer, 0, count)
                                }
                                output.toByteArray()
                            }
                        }
                    val filename = displayName(uri)
                    ProjectSnapshot.fromEnvelope(bytes, filename)
                    MaterialAlertDialogBuilder(this)
                        .setTitle("Import saved work?")
                        .setMessage(
                            "The file is checked before it is saved to your library. Your current instrument keeps playing."
                        )
                        .setPositiveButton("Import") { _, _ ->
                            engine.importProject(bytes, filename)
                        }
                        .setNegativeButton("Cancel", null)
                        .show()
                }
            }
        } catch (e: Exception) {
            exportBytes = null
            error(e)
        }
    }

    private fun displayName(uri: Uri): String {
        contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null).use {
            cursor ->
            if (cursor != null && cursor.moveToFirst()) return cursor.getString(0)
        }
        return uri.lastPathSegment ?: "import"
    }

    private fun loadSample(uri: Uri) {
        val loading = progressDialog("Sample", "Reading WAV…")
        Thread(
                {
                    try {
                        val name = displayName(uri)
                        val bytes =
                            contentResolver.openInputStream(uri).use { input ->
                                if (input == null)
                                    throw IOException("Cannot open the selected audio.")
                                EngineController.readBounded(input, 32 * 1024 * 1024)
                            }
                        val source = SampleAudio.decode(bytes, name)
                        runOnUiThread {
                            loading.dismiss()
                            sampleEditor(source, name)
                        }
                    } catch (e: Exception) {
                        runOnUiThread {
                            loading.dismiss()
                            error(e)
                        }
                    }
                },
                "FM1 sample reader",
            )
            .start()
    }

    private fun stopAudition() {
        audition?.release()
        audition = null
    }

    private fun label(value: String) =
        MaterialTextView(this).apply {
            text = value
            setTextAppearance(com.google.android.material.R.style.TextAppearance_Material3_BodyMedium)
            setPadding(dp(4f), dp(5f), dp(4f), dp(3f))
        }

    private fun textField(hint: String, value: String = "", numeric: Boolean = false): Pair<TextInputLayout, TextInputEditText> {
        val field = TextInputLayout(this)
        field.hint = hint
        val input = TextInputEditText(field.context)
        input.setSingleLine()
        if (numeric) input.inputType = android.text.InputType.TYPE_CLASS_NUMBER
        input.setText(value)
        field.addView(input, LinearLayout.LayoutParams(-1, -2))
        return field to input
    }

    private fun formContent(view: View) = LinearLayout(this).apply {
        orientation = LinearLayout.VERTICAL
        setPadding(dp(24f), dp(8f), dp(24f), dp(8f))
        addView(view)
    }

    private fun dropdown(hint: String, entries: Array<String>): Pair<TextInputLayout, MaterialAutoCompleteTextView> {
        val field = TextInputLayout(this, null, com.google.android.material.R.attr.textInputOutlinedExposedDropdownMenuStyle)
        field.hint = hint
        val input = MaterialAutoCompleteTextView(field.context)
        input.inputType = android.text.InputType.TYPE_NULL
        input.setSimpleItems(entries)
        input.setText(entries[0], false)
        input.tag = 0
        input.setOnItemClickListener { _, _, position, _ -> input.tag = position }
        field.addView(input, LinearLayout.LayoutParams(-1, -2))
        return field to input
    }

    private fun progressDialog(title: String, message: String, cancel: (() -> Unit)? = null): AlertDialog {
        val content = LinearLayout(this).apply {
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(24f), dp(16f), dp(24f), dp(16f))
            addView(CircularProgressIndicator(this@MainActivity).apply { isIndeterminate = true },
                LinearLayout.LayoutParams(dp(48f), dp(48f)).apply { marginEnd = dp(20f) })
            addView(label(message), LinearLayout.LayoutParams(0, -2, 1f))
        }
        val builder = MaterialAlertDialogBuilder(this).setTitle(title).setView(content)
            .setCancelable(cancel != null)
        if (cancel != null) builder.setNegativeButton("Cancel") { _, _ -> cancel() }
        return builder.create().apply {
            setCanceledOnTouchOutside(false)
            if (cancel != null) setOnCancelListener { cancel() }
            show()
        }
    }

    private fun sampleEditor(source: SampleAudio.Source, name: String) {
        val content = LinearLayout(this)
        content.orientation = LinearLayout.VERTICAL
        content.setPadding(dp(16f), dp(4f), dp(16f), dp(8f))
        val scroll = ScrollView(this)
        scroll.addView(content)
        val duration = source.duration()
        val trim = doubleArrayOf(0.0, min(duration, 7.38))
        val position = label("")
        content.addView(position)
        val waveform =
            object : View(this) {
                val p = Paint()

                override fun onDraw(c: Canvas) {
                    c.drawColor(color(com.google.android.material.R.attr.colorSurfaceContainerHighest))
                    val length = source.samples.size
                    p.color = accent()
                    p.strokeWidth = 1f
                    for (x in 0 until width) {
                        val begin = (x.toLong() * length / width).toInt()
                        val end = max(begin + 1, ((x + 1).toLong() * length / width).toInt())
                        var peak = 0f
                        for (j in begin until min(length, end)) peak =
                            max(peak, abs(source.samples[j]))
                        val height = peak * height * .45f
                        c.drawLine(
                            x.toFloat(),
                            this.height / 2f - height,
                            x.toFloat(),
                            this.height / 2f + height,
                            p,
                        )
                    }
                    p.color = (accent() and 0x00ffffff) or 0x33000000
                    c.drawRect(
                        (trim[0] / duration * width).toFloat(),
                        0f,
                        (trim[1] / duration * width).toFloat(),
                        height.toFloat(),
                        p,
                    )
                }
            }
        content.addView(waveform, LinearLayout.LayoutParams(-1, dp(64f)))
        content.addView(label("Trim start"))
        val start = Slider(this).apply { valueTo = 10000f; contentDescription = "Trim start" }
        content.addView(start)
        content.addView(label("Trim end · maximum 7.38 seconds"))
        val end = Slider(this).apply { valueTo = 10000f; contentDescription = "Trim end" }
        end.value = (trim[1] / duration * 10000).toFloat()
        content.addView(end)
        val update = {
            position.text =
                String.format(
                    java.util.Locale.US,
                    "%s · %.2f s source · %.3f–%.3f s selected",
                    name,
                    duration,
                    trim[0],
                    trim[1],
                )
            waveform.invalidate()
        }
        start.addOnChangeListener { _, value, user ->
            if (user) {
                trim[0] = min(duration - .001, value / 10000.0 * duration)
                trim[1] = max(trim[0] + .001, min(trim[1], trim[0] + 7.38))
                end.value = (trim[1] / duration * 10000).toFloat().coerceIn(0f, 10000f)
                update()
            }
        }
        end.addOnChangeListener { slider, value, user ->
            if (user) {
                trim[1] = max(trim[0] + .001, min(value / 10000.0 * duration, min(duration, trim[0] + 7.38)))
                slider.value = (trim[1] / duration * 10000).toFloat().coerceIn(0f, 10000f)
                update()
            }
        }
        val selectors = LinearLayout(this)
        content.addView(selectors)
        val (slotField, slot) = dropdown("Sample slot", arrayOf("USR1", "USR2", "USR3", "USR4"))
        selectors.addView(slotField, LinearLayout.LayoutParams(0, -2, 1f).apply { marginEnd = dp(12f) })
        val (rootField, root) = textField("Root MIDI 0–127", "60", true)
        selectors.addView(rootField, LinearLayout.LayoutParams(0, -2, 1f))
        content.addView(label("Root note · MIDI 60 = C4"))
        val normalize = MaterialCheckBox(this)
        normalize.text = "Normalize sample"
        content.addView(normalize)
        val preview = MaterialButton(this)
        preview.text = "Audition selection"
        content.addView(preview)
        preview.setOnClickListener {
            try {
                stopAudition()
                val wav = SampleAudio.preview(source, trim[0], trim[1])
                val temporary = File(cacheDir, "sample-audition.wav")
                temporary.outputStream().use { it.write(wav) }
                audition =
                    MediaPlayer().apply {
                        setDataSource(temporary.path)
                        prepare()
                        setOnCompletionListener { stopAudition() }
                        start()
                    }
            } catch (e: Exception) {
                error(e)
            }
        }
        update()
        val dialog =
            MaterialAlertDialogBuilder(this)
                .setTitle("Import WAV sample")
                .setView(scroll)
                .setNegativeButton("Cancel", null)
                .setPositiveButton("Review import", null)
                .create()
        dialog.setOnDismissListener { stopAudition() }
        dialog.setOnShowListener {
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener {
                try {
                    val note = root.text.toString().toInt()
                    if (note !in 0..127) throw IOException("Choose a root note from 0 to 127.")
                    MaterialAlertDialogBuilder(this)
                        .setTitle("Replace USR${(slot.tag as Int)+1}?")
                        .setMessage(
                            "A recovery backup is saved first. The selected trim is converted to mono 22.05 kHz for SLOOP."
                        )
                        .setPositiveButton("Back up and import") { _, _ ->
                            stopAudition()
                            engine.importSample(
                                source,
                                trim[0],
                                trim[1],
                                normalize.isChecked,
                                note,
                                (slot.tag as Int),
                            )
                            dialog.dismiss()
                        }
                        .setNegativeButton("Cancel", null)
                        .show()
                } catch (e: Exception) {
                    error(e)
                }
            }
        }
        dialog.show()
        dialog.window!!.setLayout(
            resources.displayMetrics.widthPixels * 4 / 5,
            resources.displayMetrics.heightPixels * 9 / 10,
        )
    }

    private fun audioExport() {
        val form = LinearLayout(this)
        form.orientation = LinearLayout.VERTICAL
        form.setPadding(dp(16f), dp(4f), dp(16f), dp(8f))
        val (modeField, mode) = dropdown("Render", arrayOf("Loop", "Whole song"))
        form.addView(modeField)
        form.addView(label("Loop music · 1–120 seconds"))
        val (secondsField, seconds) = textField("Music seconds", "15", true)
        form.addView(secondsField)
        form.addView(label("Effect tail · 0–10 seconds"))
        val (tailField, tail) = textField("Tail seconds", "2", true)
        form.addView(tailField)
        form.addView(
            label(
                "Stereo WAV · 44.1 kHz · 16-bit PCM. Renders a separate copy; live work keeps playing. Whole song uses saved sections A–D, with a 30-minute limit."
            )
        )
        val dialog =
            MaterialAlertDialogBuilder(this)
                .setTitle("Export audio")
                .setView(ScrollView(this).apply { addView(form) })
                .setNegativeButton("Cancel", null)
                .setPositiveButton("Prepare WAV", null)
                .create()
        dialog.setOnShowListener {
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener {
                try {
                    val duration = seconds.text.toString().toInt()
                    val effect = tail.text.toString().toInt()
                    if (duration !in 1..120 || effect !in 0..10)
                        throw IOException("Choose 1–120 music seconds and 0–10 tail seconds.")
                    val song = (mode.tag as Int) == 1
                    dialog.dismiss()
                    val busy = progressDialog("Export audio", "Rendering a separate copy…") {
                        engine.cancelExport()
                    }
                    engine.exportAudio(song, duration, effect) callback@{ file, failure ->
                        busy.dismiss()
                        if (isFinishing || isDestroyed) {
                            file?.delete()
                            return@callback
                        }
                        if (failure != null) {
                            if (failure !is java.util.concurrent.CancellationException)
                                error(failure)
                            return@callback
                        }
                        exportFile = file
                        startActivityForResult(
                            Intent(Intent.ACTION_CREATE_DOCUMENT)
                                .setType("audio/wav")
                                .addCategory(Intent.CATEGORY_OPENABLE)
                                .putExtra(
                                    Intent.EXTRA_TITLE,
                                    if (song) "SLOOP-song.wav" else "SLOOP-loop.wav",
                                ),
                            EXPORT,
                        )
                    }
                } catch (e: Exception) {
                    error(e)
                }
            }
        }
        dialog.show()
    }

    private fun error(e: Exception) {
        MaterialAlertDialogBuilder(this)
            .setTitle("File action")
            .setMessage(e.message ?: "The file action could not be completed.")
            .setPositiveButton("OK", null)
            .show()
    }

    override fun onResume() {
        super.onResume()
        foreground = true
        if (::engine.isInitialized) engine.setForeground(true)
    }

    override fun onPause() {
        foreground = false
        stopAudition()
        if (::panel.isInitialized) panel.releaseTouches()
        if (::engine.isInitialized) engine.setForeground(false)
        super.onPause()
    }

    override fun onDestroy() {
        if (::engine.isInitialized) engine.close()
        super.onDestroy()
    }
}
