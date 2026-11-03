package dev.linktolinux.companion

data class ClipboardPolicy(val enabled: Boolean, val connected: Boolean, val focused: Boolean) {
    val canAutoSend: Boolean get() = enabled && connected && focused
    val canManualSend: Boolean get() = connected && focused
    fun canReceive(automatic: Boolean): Boolean = connected && (!automatic || enabled)
}

class ClipboardTracker {
    private var lastText: String? = null
    @Synchronized fun shouldSend(text: String, force: Boolean): Boolean = force || text != lastText
    @Synchronized fun remember(text: String) { lastText = text }
    @Synchronized fun clear() { lastText = null }
}
