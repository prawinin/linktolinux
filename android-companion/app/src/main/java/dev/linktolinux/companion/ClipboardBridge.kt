package dev.linktolinux.companion

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Handler
import android.os.Looper
import org.json.JSONObject

object ClipboardBridge {
    private val main = Handler(Looper.getMainLooper())
    private val tracker = ClipboardTracker()
    private var clipboard: ClipboardManager? = null
    private var application: Context? = null
    private var focused = false
    private val listener = ClipboardManager.OnPrimaryClipChangedListener { send(force = false) }

    fun enabled(context: Context): Boolean = context.getSharedPreferences("linktolinux", Context.MODE_PRIVATE).getBoolean("clipboard_sync", true)
    fun setEnabled(context: Context, enabled: Boolean) {
        context.getSharedPreferences("linktolinux", Context.MODE_PRIVATE).edit().putBoolean("clipboard_sync", enabled).apply()
        if (enabled) send(force = false)
    }
    fun attach(context: Context) {
        check(Looper.myLooper() == Looper.getMainLooper()) { "Clipboard bridge requires the main thread" }
        if (clipboard != null) return
        application = context.applicationContext
        clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        clipboard?.addPrimaryClipChangedListener(listener)
    }
    fun detach() {
        clipboard?.removePrimaryClipChangedListener(listener)
        clipboard = null
        application = null
        tracker.clear()
    }
    fun focused(value: Boolean, sendOnFocus: Boolean = true) {
        focused = value
        if (value && sendOnFocus) send(force = false)
    }
    fun connected() { main.post { send(force = false) } }
    private fun policy(): ClipboardPolicy = ClipboardPolicy(application?.let { enabled(it) } ?: true, CompanionConnection.connected, focused)
    fun handles(command: String): Boolean = command == "clipboard_get" || command == "clipboard_set"
    fun request(context: Context, message: JSONObject) {
        main.post {
            val id = message.optString("id")
            try {
                attach(context)
                val args = message.optJSONObject("args") ?: JSONObject()
                when (message.optString("command")) {
                    "clipboard_set" -> {
                        check(policy().canReceive(args.optBoolean("automatic", false))) { "Automatic clipboard sync is disabled on the phone." }
                        val text = args.getString("text")
                        require(text.length <= 1024 * 1024) { "Clipboard text exceeds 1 MiB" }
                        clipboard?.setPrimaryClip(ClipData.newPlainText("Linux", text))
                        tracker.remember(text)
                        CompanionConnection.respond(id)
                    }
                    "clipboard_get" -> {
                        check(policy().canManualSend) { "Open the companion on your phone to send clipboard text." }
                        CompanionConnection.respond(id, JSONObject().put("text", readText().orEmpty()))
                    }
                    else -> CompanionConnection.respond(id, error = "Unsupported clipboard command")
                }
            } catch (error: Exception) {
                CompanionConnection.respond(id, error = error.message ?: "Clipboard access failed. Open the companion and retry.")
            }
        }
    }
    private fun readText(): String? {
        val context = application ?: return null
        val clip = clipboard?.primaryClip ?: return null
        if (clip.itemCount == 0) return null
        val text = clip.getItemAt(0).coerceToText(context)?.toString() ?: return null
        require(text.length <= 1024 * 1024) { "Clipboard text exceeds 1 MiB" }
        return text
    }
    fun send(force: Boolean): Boolean {
        val state = policy()
        if (!(if (force) state.canManualSend else state.canAutoSend)) return false
        return try {
            val text = readText() ?: return false
            if (!tracker.shouldSend(text, force)) return false
            if (!CompanionConnection.send(JSONObject().put("type", "clipboard").put("text", text).put("automatic", !force))) return false
            tracker.remember(text)
            true
        } catch (error: Exception) {
            android.util.Log.w("ClipboardBridge", "Clipboard access failed: ${error.javaClass.simpleName}")
            false
        }
    }
}
