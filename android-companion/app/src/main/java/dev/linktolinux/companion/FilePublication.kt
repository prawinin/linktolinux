package dev.linktolinux.companion

import java.util.concurrent.atomic.AtomicInteger

internal class FilePublication {
    private val state = AtomicInteger(0)
    val committing: Boolean get() = state.get() == 1
    fun begin(): Boolean = state.compareAndSet(0, 1)
    fun cancel(): Boolean = state.compareAndSet(0, 2)
}
