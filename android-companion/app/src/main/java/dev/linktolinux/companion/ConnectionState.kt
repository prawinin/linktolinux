package dev.linktolinux.companion

import java.net.URI

enum class InputField { HOST, PORT, CODE }
class ConnectionInputException(val field: InputField, message: String) : IllegalArgumentException(message)
data class DesktopEndpoint(val host: String, val port: Int, val code: String) {
    companion object {
        fun parse(address: String, port: String, code: String): DesktopEndpoint {
            val raw = address.trim()
            val authority = if (!raw.contains("://") && !raw.startsWith("[") && raw.count { it == ':' } > 1) "[$raw]" else raw
            val uri = try { URI(if (authority.contains("://")) authority else "tcp://$authority") }
                catch (error: java.net.URISyntaxException) { throw ConnectionInputException(InputField.HOST, "Enter the desktop IP address or hostname shown in Linux.") }
            val host = uri.host.orEmpty().removeSurrounding("[", "]")
            if (host.isEmpty() || host.length > 253 || uri.userInfo != null || uri.scheme !in listOf("tcp", "http", "https"))
                throw ConnectionInputException(InputField.HOST, "Enter the desktop IP address or hostname shown in Linux.")
            val number = if (uri.port != -1) uri.port else port.trim().toIntOrNull()
            if (number == null || number !in 1..65535) throw ConnectionInputException(InputField.PORT, "Enter a port between 1 and 65535, as shown in Linux.")
            val normalized = code.filterNot { it.isWhitespace() || it == '-' }.uppercase(java.util.Locale.ROOT)
            if (!normalized.matches(Regex("[A-F0-9]{12}"))) throw ConnectionInputException(InputField.CODE, "Enter all 12 characters of the pairing code shown in Linux.")
            return DesktopEndpoint(host, number, normalized)
        }
    }
}
enum class ConnectionPhase { DISCONNECTED, CONNECTING, CONNECTED, ERROR }
data class ConnectionUiState(val phase: ConnectionPhase, val message: String, val captureActive: Boolean = false) {
    val showProgress: Boolean get() = phase == ConnectionPhase.CONNECTING
    val connectLabel: String get() = when (phase) {
        ConnectionPhase.CONNECTING -> "Cancel connection"
        ConnectionPhase.CONNECTED -> "Disconnect"
        else -> "Connect to desktop"
    }
    val showStopCapture: Boolean get() = captureActive
    val canSendClipboard: Boolean get() = phase == ConnectionPhase.CONNECTED
}
data class PermissionSnapshot(val camera: Boolean, val microphone: Boolean, val photos: Boolean, val notifications: Boolean) {
    val needsAttention: Boolean get() = !camera || !microphone || !photos || !notifications
}
