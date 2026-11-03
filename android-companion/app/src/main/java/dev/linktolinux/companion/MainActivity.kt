package dev.linktolinux.companion

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.text.InputType
import android.widget.CheckBox
import android.widget.ProgressBar
import android.widget.Toast
import android.view.View
import android.view.inputmethod.InputMethodManager
import android.net.Uri
import android.provider.Settings
import android.widget.Button
import android.widget.EditText
import android.widget.Switch
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.app.Activity
import androidx.core.content.IntentCompat
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import org.json.JSONObject

class MainActivity : Activity() {
    private lateinit var status: TextView
    private lateinit var host: EditText
    private lateinit var port: EditText
    private lateinit var token: EditText
    private lateinit var connect: Button
    private lateinit var permissions: Button
    private lateinit var stopCapture: Button
    private lateinit var sendClip: Button
    private lateinit var progress: ProgressBar
    private lateinit var permissionStatus: TextView
    private lateinit var remember: CheckBox
    private var autoConnectChecked = false
    private lateinit var pairingDetails: LinearLayout
    private lateinit var detailsButton: Button
    private lateinit var cameraSwitch: Button
    private lateinit var cameraRotate: Button
    private lateinit var cameraStatus: TextView
    private lateinit var syncClipboard: Switch
    private lateinit var clipboardStatus: TextView
    private lateinit var permissionPanel: LinearLayout
    private var detailsExpanded = false
    private var pendingClipboardSend = false
    private lateinit var fileViews: FileTransferViews
    private val pendingFiles = PendingFileShares()
    private var sendingPendingFiles = false
    private lateinit var backgroundPanel: LinearLayout

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val prefs = getSharedPreferences("linktolinux", Context.MODE_PRIVATE)
        val paired = PairingStore(this).load()
        val ui = CompanionViews(this)
        window.statusBarColor = ui.background
        window.navigationBarColor = ui.background
        androidx.core.view.WindowCompat.getInsetsController(window, window.decorView).apply {
            isAppearanceLightStatusBars = true
            isAppearanceLightNavigationBars = true
        }
        val layout = ui.column().apply {
            setPadding(dp(18), dp(22), dp(18), dp(18))
            setBackgroundColor(ui.background)
        }
        layout.addView(ui.row().apply {
            addView(ImageView(this@MainActivity).apply {
                setImageResource(R.mipmap.ic_launcher)
                layoutParams = LinearLayout.LayoutParams(dp(38), dp(38)).apply { marginEnd = dp(12) }
            })
            addView(ui.column().apply {
                addView(ui.text("LinkToLinux", 23f, bold = true))
                addView(ui.text("Android companion", 13f, muted = true))
            })
            layoutParams = LinearLayout.LayoutParams(-1, -2).apply { bottomMargin = dp(18) }
        })
        val connection = ui.card("Desktop connection")
        status = ui.text(CompanionConnection.status, 15f, bold = true).apply {
            accessibilityLiveRegion = View.ACCESSIBILITY_LIVE_REGION_POLITE
        }
        connection.addView(status)
        progress = ProgressBar(this).apply { visibility = View.GONE }
        connection.addView(progress, LinearLayout.LayoutParams(dp(24), dp(24)).apply { bottomMargin = dp(8) })
        pairingDetails = ui.column()
        pairingDetails.addView(ui.text("Desktop address", 13f, muted = true))
        host = ui.field("Address shown in Linux", paired?.host ?: prefs.getString("host", "").orEmpty(), InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI)
        pairingDetails.addView(host)
        val credentials = ui.row()
        val portColumn = ui.column().apply { layoutParams = LinearLayout.LayoutParams(dp(88), -2).apply { marginEnd = dp(12) } }
        portColumn.addView(ui.text("Port", 13f, muted = true))
        port = ui.field("Port", (paired?.port ?: prefs.getInt("port", 1718)).toString(), InputType.TYPE_CLASS_NUMBER)
        portColumn.addView(port)
        val codeColumn = ui.column().apply { layoutParams = LinearLayout.LayoutParams(0, -2, 1f) }
        codeColumn.addView(ui.text("Pairing code", 13f, muted = true))
        token = ui.field("12-character code", paired?.code.orEmpty(), InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_CAP_CHARACTERS or InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD)
        codeColumn.addView(token)
        credentials.addView(portColumn); credentials.addView(codeColumn)
        pairingDetails.addView(credentials)
        remember = CheckBox(this).apply {
            setText(R.string.remember_desktop)
            textSize = 14f
            isChecked = prefs.getBoolean("remember", true)
            setTextColor(ui.ink)
            buttonTintList = android.content.res.ColorStateList.valueOf(ui.accent)
            setOnCheckedChangeListener { _, checked ->
                prefs.edit().putBoolean("remember", checked).apply()
                if (!checked) PairingStore(this@MainActivity).clear()
            }
        }
        pairingDetails.addView(remember)
        pairingDetails.addView(ui.text("Remember this desktop and reconnect after interruptions. Use private Wi-Fi; the local connection is not encrypted.", 12f, muted = true))
        connection.addView(pairingDetails)
        connect = ui.button(CompanionConnection.ui.connectLabel, primary = true) {
            if (CompanionConnection.connected || CompanionConnection.ui.showProgress) {
                pendingClipboardSend = false
                pendingFiles.clear()
                CompanionConnection.disconnect()
                prefs.edit().putBoolean("auto_connect", false).apply()
                stopService(Intent(this, CompanionService::class.java))
                stopService(Intent(this, CameraMicStreamer::class.java))
                renderState()
            } else startConnection()
        }
        detailsButton = ui.button("Connection details") { detailsExpanded = !detailsExpanded; renderState() }
        connection.addView(ui.pairedButtons(connect, detailsButton))
        layout.addView(connection)
        backgroundPanel = ui.card("Background connection")
        backgroundPanel.addView(ui.text("Keep sharing when you switch apps or close this screen. If your phone stops LinkToLinux, set its battery use to Unrestricted or No restrictions and allow background activity in your phone settings.", 13f, muted = true))
        backgroundPanel.addView(ui.button("Background settings") {
            try { startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)) }
            catch (error: android.content.ActivityNotFoundException) {
                try { startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:$packageName"))) }
                catch (failure: RuntimeException) { Toast.makeText(this, "Open Android Settings, then LinkToLinux app info and battery settings.", Toast.LENGTH_LONG).show() }
            }
        })
        layout.addView(backgroundPanel)

        fileViews = FileTransferViews(this, { chooseFiles() }, { pendingFiles.clear(); renderFiles() })
        layout.addView(fileViews.view)

        val camera = ui.card("Camera and microphone")
        cameraStatus = ui.text("", 14f)
        camera.addView(cameraStatus)
        cameraSwitch = ui.button("Switch camera") { changeCamera { CameraControls.switchCamera(this) } }
        cameraRotate = ui.button("Rotate video") { changeCamera { CameraControls.rotate(this) } }
        camera.addView(ui.pairedButtons(cameraSwitch, cameraRotate))
        camera.addView(ui.text("Start sharing from Linux. Keep this app open until capture starts.", 12f, muted = true).apply {
            layoutParams = LinearLayout.LayoutParams(-1, -2).apply { topMargin = dp(12) }
        })
        stopCapture = ui.button("Stop sharing", destructive = true) {
            if (!stopService(Intent(this, CameraMicStreamer::class.java))) CompanionConnection.captureChanged(false)
        }
        camera.addView(stopCapture)
        layout.addView(camera)

        val clipboard = ui.card("Clipboard")
        syncClipboard = Switch(this).apply {
            setText(R.string.automatic_clipboard_sync)
            textSize = 15f
            setTextColor(ui.ink)
            isChecked = ClipboardBridge.enabled(this@MainActivity)
            setPadding(0, dp(8), 0, dp(8))
            setOnCheckedChangeListener { _, checked -> ClipboardBridge.setEnabled(this@MainActivity, checked); renderState() }
        }
        clipboard.addView(syncClipboard)
        clipboardStatus = ui.text("", 12f, muted = true)
        clipboard.addView(clipboardStatus)
        clipboard.addView(ui.text("Desktop text arrives in the background. Send phone text while this app is open, or tap Send clipboard in its notification.", 12f, muted = true))
        sendClip = ui.button("Send clipboard now") { sendClipboard(force = true) }
        clipboard.addView(sendClip)
        layout.addView(clipboard)

        permissionPanel = ui.card("Access needed")
        permissionStatus = ui.text("", 13f, muted = true)
        permissionPanel.addView(permissionStatus)
        permissions = ui.button("Manage permissions") { requestPermissions() }
        permissionPanel.addView(permissions)
        layout.addView(permissionPanel)
        layout.addView(ui.button("Licenses") { showLicenses(ui) })
        setContentView(ScrollView(this).apply { isFillViewport = true; addView(layout) })
        pendingClipboardSend = savedInstanceState?.getBoolean("pending_clipboard_send")
            ?: (intent?.getBooleanExtra("send_clipboard", false) ?: false)
        intent?.removeExtra("send_clipboard")
        try { savedInstanceState?.getStringArrayList("pending_file_uris")?.let { pendingFiles.add(it) } }
        catch (failure: IllegalArgumentException) { Toast.makeText(this, failure.message, Toast.LENGTH_LONG).show() }
        if (savedInstanceState == null) acceptSharedFiles(intent)
        else clearShareIntent(intent)
        FileTransfers.onChanged = { runOnUiThread { renderFiles() } }
        CompanionConnection.onStatus = { runOnUiThread { renderState() } }
        CompanionConnection.onRequest = { message -> handleRequest(message) }
        renderState()
    }
    private fun changeCamera(action: () -> CameraControlState) {
        try { action(); renderState() }
        catch (error: Exception) { Toast.makeText(this, error.message ?: "Camera controls are unavailable", Toast.LENGTH_LONG).show() }
    }
    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()
    private fun startConnection() {
        host.error = null; port.error = null; token.error = null
        val endpoint = try { DesktopEndpoint.parse(host.text.toString(), port.text.toString(), token.text.toString()) }
        catch (error: ConnectionInputException) {
            val field = when (error.field) { InputField.HOST -> host; InputField.PORT -> port; InputField.CODE -> token }
            detailsExpanded = true
            pairingDetails.visibility = View.VISIBLE
            field.error = error.message
            field.requestFocus()
            status.text = error.message
            Toast.makeText(this, error.message, Toast.LENGTH_LONG).show()
            return
        }
        host.setText(endpoint.host); port.setText(endpoint.port.toString()); token.setText(endpoint.code)
        getSharedPreferences("linktolinux", Context.MODE_PRIVATE).edit().putString("host", endpoint.host).putInt("port", endpoint.port).apply()
        (getSystemService(INPUT_METHOD_SERVICE) as InputMethodManager).hideSoftInputFromWindow(connect.windowToken, 0)
        CompanionConnection.updateStatus("Connecting to desktop…", ConnectionPhase.CONNECTING)
        renderState()
        try {
            ContextCompat.startForegroundService(this, Intent(this, CompanionService::class.java)
                .putExtra("host", endpoint.host).putExtra("port", endpoint.port).putExtra("token", endpoint.code).putExtra("remember", remember.isChecked))
        } catch (error: RuntimeException) {
            CompanionConnection.updateStatus("Android blocked the connection service. Keep this app open, check permissions, then retry.", ConnectionPhase.ERROR)
        }
    }
    private fun granted(permission: String): Boolean = ContextCompat.checkSelfPermission(this, permission) == PackageManager.PERMISSION_GRANTED
    private fun permissionSnapshot(): PermissionSnapshot = PermissionSnapshot(
        granted(Manifest.permission.CAMERA), granted(Manifest.permission.RECORD_AUDIO),
        if (Build.VERSION.SDK_INT >= 33) granted(Manifest.permission.READ_MEDIA_IMAGES) ||
            (Build.VERSION.SDK_INT >= 34 && granted(Manifest.permission.READ_MEDIA_VISUAL_USER_SELECTED))
        else granted(Manifest.permission.READ_EXTERNAL_STORAGE),
        Build.VERSION.SDK_INT < 33 || granted(Manifest.permission.POST_NOTIFICATIONS))
    private fun missingPermissions(): List<String> {
        val state = permissionSnapshot()
        return buildList {
            if (!state.camera) add(Manifest.permission.CAMERA)
            if (!state.microphone) add(Manifest.permission.RECORD_AUDIO)
            if (!state.photos) {
                add(if (Build.VERSION.SDK_INT >= 33) Manifest.permission.READ_MEDIA_IMAGES else Manifest.permission.READ_EXTERNAL_STORAGE)
                if (Build.VERSION.SDK_INT >= 34) add(Manifest.permission.READ_MEDIA_VISUAL_USER_SELECTED)
            }
            if (!state.notifications && Build.VERSION.SDK_INT >= 33) add(Manifest.permission.POST_NOTIFICATIONS)
            if (Build.VERSION.SDK_INT <= 28 && !FileTransfers.canReceive(this@MainActivity)) add(Manifest.permission.WRITE_EXTERNAL_STORAGE)
        }
    }
    private fun renderState() {
        if (!::connect.isInitialized) return
        val state = CompanionConnection.ui
        status.text = state.message
        connect.text = state.connectLabel
        progress.visibility = if (state.showProgress) View.VISIBLE else View.GONE
        host.isEnabled = !state.showProgress && !CompanionConnection.connected
        port.isEnabled = host.isEnabled; token.isEnabled = host.isEnabled
        remember.isEnabled = !state.showProgress && !CompanionConnection.connected
        stopCapture.visibility = if (state.showStopCapture) View.VISIBLE else View.GONE
        pairingDetails.visibility = if (!CompanionConnection.connected || detailsExpanded) View.VISIBLE else View.GONE
        detailsButton.visibility = if (CompanionConnection.connected) View.VISIBLE else View.GONE
        detailsButton.setText(if (detailsExpanded) R.string.hide_details else R.string.connection_details)
        val camera = CameraControls.state(this)
        cameraSwitch.setText(if (camera.frontFacing) R.string.switch_back_camera else R.string.switch_front_camera)
        cameraSwitch.isEnabled = camera.canSwitch && !camera.busy && granted(Manifest.permission.CAMERA)
        cameraRotate.isEnabled = !camera.busy && granted(Manifest.permission.CAMERA)
        cameraStatus.text = cameraDescription(camera)
        clipboardStatus.setText(if (syncClipboard.isChecked) R.string.clipboard_sync_enabled else R.string.clipboard_sync_disabled)
        sendClip.isEnabled = state.canSendClipboard
        val access = permissionSnapshot()
        val missingFileAccess = Build.VERSION.SDK_INT <= 28 && !FileTransfers.canReceive(this)
        permissions.visibility = if (access.needsAttention || missingFileAccess) View.VISIBLE else View.GONE
        permissionPanel.visibility = permissions.visibility
        val power = getSystemService(POWER_SERVICE) as android.os.PowerManager
        val activity = getSystemService(ACTIVITY_SERVICE) as android.app.ActivityManager
        val restricted = Build.VERSION.SDK_INT >= 28 && activity.isBackgroundRestricted
        backgroundPanel.visibility = if (restricted || !power.isIgnoringBatteryOptimizations(packageName)) View.VISIBLE else View.GONE
        val missing = buildList {
            if (!access.camera) add("camera")
            if (!access.microphone) add("microphone")
            if (!access.photos) add("photos")
            if (!access.notifications) add("notifications")
            if (missingFileAccess) add("file downloads")
        }
        permissionStatus.text = if (missing.isEmpty()) "Permissions granted" else "Allow ${missing.joinToString(", ")} to use these features."
        dispatchClipboardAction()
        renderFiles()
    }
    private fun requestPermissions() {
        val missing = missingPermissions()
        if (missing.isEmpty()) { renderState(); return }
        val prefs = getSharedPreferences("permission_requests", Context.MODE_PRIVATE)
        val blocked = missing.any { prefs.getBoolean(it, false) && !ActivityCompat.shouldShowRequestPermissionRationale(this, it) }
        if (blocked) {
            startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:$packageName")))
        } else {
            prefs.edit().apply { missing.forEach { putBoolean(it, true) } }.apply()
            ActivityCompat.requestPermissions(this, missing.toTypedArray(), 101)
        }
    }
    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        renderState()
    }
    override fun onStop() {
        autoConnectChecked = false
        super.onStop()
    }
    override fun onResume() {
        super.onResume()
        renderState()
        if (!autoConnectChecked && ::remember.isInitialized) {
            autoConnectChecked = true
            if (!CompanionConnection.connected && !CompanionConnection.ui.showProgress &&
                getSharedPreferences("linktolinux", Context.MODE_PRIVATE).getBoolean("auto_connect", false) && PairingStore(this).load() != null) {
                startConnection()
            }
        }
    }
    private fun handleRequest(message: JSONObject) {
        val id = message.optString("id")
        val command = message.optString("command")
        val args = message.optJSONObject("args") ?: JSONObject()
        runOnUiThread {
            try {
                when (command) {
                    "clipboard_get", "clipboard_set" -> ClipboardBridge.request(this, message)
                    "capture_start" -> {
                        check(hasWindowFocus()) { "Open the companion on your phone before starting camera or microphone capture." }
                        ContextCompat.startForegroundService(this, Intent(this, CameraMicStreamer::class.java)
                            .putExtra("id", id).putExtra("mode", args.getString("mode")))
                    }
                    "capture_stop" -> {
                        val active = CameraMicStreamer.active
                        if (active == null) CompanionConnection.respond(id) else active.requestStop(id)
                    }
                    else -> CompanionConnection.respond(id, error = "Unsupported command")
                }
            } catch (error: Exception) { CompanionConnection.respond(id, error = error.message ?: "Phone request failed") }
        }
    }
    private fun showLicenses(ui: CompanionViews) {
        try {
            val text = listOf("LICENSE.txt", "third-party-notices.txt").joinToString("\n\n") { name ->
                assets.open(name).bufferedReader(Charsets.UTF_8).use { it.readText() }
            }
            val content = ui.text(text, 12f).apply { setTextIsSelectable(true); setPadding(dp(18), dp(12), dp(18), dp(12)) }
            android.app.AlertDialog.Builder(this).setTitle("Licenses")
                .setView(ScrollView(this).apply { addView(content) }).setPositiveButton("Close", null).show()
        } catch (error: java.io.IOException) { Toast.makeText(this, "License text is unavailable. See the project repository.", Toast.LENGTH_LONG).show() }
    }
    private fun chooseFiles() {
        startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
            type = "*/*"
            addCategory(Intent.CATEGORY_OPENABLE)
            putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION)
        }, 202)
    }
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != 202 || resultCode != RESULT_OK || data == null) return
        try {
            val uris = mutableListOf<Uri>()
            data.clipData?.let { clip ->
                require(clip.itemCount <= 16) { "Choose up to 16 files at a time" }
                for (index in 0 until clip.itemCount) clip.getItemAt(index).uri?.let { uris.add(it) }
            }
            data.data?.let { uris.add(it) }
            val selected = uris.distinct()
            pendingFiles.add(selected.map { it.toString() })
            if (data.flags and Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION != 0) {
                selected.forEach { uri ->
                    try { contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
                    catch (failure: SecurityException) { android.util.Log.w("FileSharing", "Document permission is temporary") }
                }
            }
            renderFiles()
        } catch (failure: Exception) { Toast.makeText(this, failure.message ?: "Could not select files", Toast.LENGTH_LONG).show() }
    }
    private fun clearShareIntent(source: Intent?) {
        if (source?.action == Intent.ACTION_SEND || source?.action == Intent.ACTION_SEND_MULTIPLE) {
            source.removeExtra(Intent.EXTRA_STREAM)
            source.clipData = null
            source.action = Intent.ACTION_MAIN
        }
    }
    private fun acceptSharedFiles(source: Intent?) {
        if (source == null || (source.action != Intent.ACTION_SEND && source.action != Intent.ACTION_SEND_MULTIPLE)) return
        try {
            val uris = mutableListOf<Uri>()
            if (source.action == Intent.ACTION_SEND) {
                IntentCompat.getParcelableExtra(source, Intent.EXTRA_STREAM, Uri::class.java)?.let { uris.add(it) }
            } else {
                IntentCompat.getParcelableArrayListExtra(source, Intent.EXTRA_STREAM, Uri::class.java)?.let {
                    require(it.size <= 16) { "Share up to 16 files at a time" }
                    uris.addAll(it)
                }
            }
            source.clipData?.let { clip ->
                require(clip.itemCount <= 16) { "Share up to 16 files at a time" }
                for (index in 0 until clip.itemCount) clip.getItemAt(index).uri?.let { uris.add(it) }
            }
            require(uris.isNotEmpty()) { "This share contains no file. Use Send files to choose one." }
            val selected = uris.distinct()
            pendingFiles.add(selected.map { it.toString() })
            if (source.flags and Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION != 0) {
                selected.forEach { uri ->
                    try { contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
                    catch (failure: SecurityException) { android.util.Log.w("FileSharing", "Shared document permission is temporary") }
                }
            }
        } catch (failure: Exception) { Toast.makeText(this, failure.message ?: "Could not share these files", Toast.LENGTH_LONG).show() }
        finally { clearShareIntent(source) }
    }
    private fun renderFiles() {
        if (!::fileViews.isInitialized) return
        val active = FileTransfers.items().count { it.status in setOf("queued", "waiting", "transferring") }
        if (!sendingPendingFiles && pendingFiles.snapshot().isNotEmpty() && active + pendingFiles.snapshot().size <= 16 && CompanionConnection.connected && FileTransfers.available) {
            sendingPendingFiles = true
            val selected = pendingFiles.take()
            try { FileTransfers.send(applicationContext, selected.map { Uri.parse(it) }) }
            catch (failure: Exception) {
                pendingFiles.add(selected)
                Toast.makeText(this, failure.message ?: "Could not queue files", Toast.LENGTH_LONG).show()
            } finally { sendingPendingFiles = false }
        }
        fileViews.render(CompanionConnection.connected, pendingFiles.snapshot().size, FileTransfers.items())
    }
    private fun cameraDescription(camera: CameraControlState): String {
        val lens = if (camera.frontFacing) "Front camera" else "Back camera"
        val orientation = if (camera.rotationDegrees % 180 == 0) "Landscape" else "Portrait"
        return if (camera.busy) "Switching camera" else "$lens, $orientation (${camera.rotationDegrees}°)"
    }
    private fun sendClipboard(force: Boolean = false) {
        if (ClipboardBridge.send(force)) {
            if (force) clipboardStatus.setText(R.string.clipboard_sent)
        } else if (force) {
            Toast.makeText(this, if (CompanionConnection.connected) "No text available to send" else getString(R.string.connection_required), Toast.LENGTH_SHORT).show()
        }
    }
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        pendingClipboardSend = pendingClipboardSend || intent.getBooleanExtra("send_clipboard", false)
        intent.removeExtra("send_clipboard")
        acceptSharedFiles(intent)
        dispatchClipboardAction()
        renderFiles()
    }
    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        ClipboardBridge.focused(hasFocus, sendOnFocus = !pendingClipboardSend)
        dispatchClipboardAction()
    }
    private fun dispatchClipboardAction() {
        if (pendingClipboardSend && hasWindowFocus() && CompanionConnection.connected) {
            pendingClipboardSend = false
            sendClipboard(force = true)
        }
    }
    override fun onSaveInstanceState(outState: Bundle) {
        outState.putBoolean("pending_clipboard_send", pendingClipboardSend)
        outState.putStringArrayList("pending_file_uris", ArrayList(pendingFiles.snapshot()))
        super.onSaveInstanceState(outState)
    }
    override fun onDestroy() {
        FileTransfers.onChanged = null
        CompanionConnection.onStatus = null
        CompanionConnection.onRequest = null
        ClipboardBridge.focused(false)
        super.onDestroy()
    }
}
