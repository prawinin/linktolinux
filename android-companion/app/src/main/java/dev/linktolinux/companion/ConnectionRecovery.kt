package dev.linktolinux.companion

object ConnectionRecovery {
    fun restore(autoConnect: Boolean, saved: DesktopEndpoint?): DesktopEndpoint? = if (autoConnect) saved else null

    fun run(isActive: () -> Boolean, connect: () -> FailureKind?, onRetry: (Long) -> Unit,
            wait: (Long) -> Unit = { Thread.sleep(it) }) {
        var delay = 1000L
        while (isActive()) {
            val failure = connect() ?: return
            if (!isActive() || failure == FailureKind.PAIRING || failure == FailureKind.PROTOCOL) return
            onRetry(delay)
            try { wait(delay) }
            catch (error: InterruptedException) { Thread.currentThread().interrupt(); return }
            delay = (delay * 2).coerceAtMost(30000L)
        }
    }
}
