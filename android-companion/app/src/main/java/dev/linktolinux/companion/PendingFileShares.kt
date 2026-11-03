package dev.linktolinux.companion

import java.net.URI

class PendingFileShares {
    private var values = emptyList<String>()
    fun add(uris: List<String>) {
        require(uris.size <= 16) { "Choose up to 16 files at a time" }
        require(uris.all { value ->
            value.length <= 8192 && value.none { it.code < 32 || it.code == 127 } &&
                try { URI(value).let { it.scheme == "content" && !it.rawAuthority.isNullOrBlank() } }
                catch (error: java.net.URISyntaxException) { false }
        }) { "Share files from a document provider" }
        val combined = (values + uris).distinct()
        require(combined.size <= 16) { "Choose up to 16 files at a time" }
        values = combined
    }
    fun snapshot(): List<String> = values.toList()
    fun take(): List<String> = snapshot().also { values = emptyList() }
    fun clear() { values = emptyList() }
}
