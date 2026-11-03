package dev.linktolinux.companion

import android.content.Context
import android.content.res.ColorStateList
import android.graphics.Color
import android.text.TextUtils
import android.view.Gravity
import android.view.View
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.TextView
import android.widget.Toast
import java.text.NumberFormat

class FileTransferViews(private val context: Context, pick: () -> Unit, clearPending: () -> Unit) {
    private val ui = CompanionViews(context)
    val view: LinearLayout = ui.card("Files")
    private val send: Button = ui.button("Send files", primary = true, action = pick)
    private val summary = ui.text("", 13f, muted = true)
    private val pending = ui.text("", 13f)
    private val discard = ui.button("Discard selected files", action = clearPending)
    private val history = ui.column()
    private val rows = mutableMapOf<String, TransferRow>()
    init {
        view.addView(summary)
        view.addView(send)
        view.addView(pending)
        view.addView(discard)
        view.addView(history)
        view.addView(ui.text("Received files are saved to Downloads/LinkToLinux. You can also share files to LinkToLinux from another app.", 12f, muted = true).apply {
            layoutParams = LinearLayout.LayoutParams(-1, -2).apply { topMargin = ui.dp(12) }
        })
    }
    fun render(connected: Boolean, pendingCount: Int, items: List<FileTransferItem>) {
        send.isEnabled = connected && FileTransfers.available
        summary.text = when {
            !connected -> "Connect to your desktop to send files."
            !FileTransfers.available -> "Update LinkToLinux on your desktop to enable file sharing."
            else -> "Choose up to 16 files, 8 GiB per file."
        }
        pending.text = if (connected && FileTransfers.available && pendingCount > 0)
            context.resources.getQuantityString(R.plurals.waiting_file_count, pendingCount, pendingCount)
            else context.resources.getQuantityString(R.plurals.pending_file_count, pendingCount, pendingCount)
        pending.visibility = if (pendingCount > 0) View.VISIBLE else View.GONE
        discard.visibility = pending.visibility
        val visible = items.sortedWith(compareByDescending<FileTransferItem> { active(it) }.thenByDescending { it.startedAt }).take(20)
        val retained = visible.map { it.id }.toSet()
        rows.keys.filter { it !in retained }.forEach { rows.remove(it) }
        history.removeAllViews()
        if (visible.isEmpty()) {
            history.addView(ui.text("No transfers yet", 13f, muted = true).apply {
                layoutParams = LinearLayout.LayoutParams(-1, -2).apply { topMargin = ui.dp(16) }
            })
        } else visible.forEach { item ->
            val row = rows.getOrPut(item.id) { TransferRow() }
            row.update(item)
            history.addView(row.view)
        }
    }
    private fun active(item: FileTransferItem): Boolean = item.status in setOf("queued", "waiting", "transferring")
    private fun bytes(value: Long): String {
        val units = listOf("B", "KiB", "MiB", "GiB")
        var amount = value.toDouble()
        var unit = 0
        while (amount >= 1024 && unit < units.lastIndex) { amount /= 1024; unit++ }
        return NumberFormat.getNumberInstance().apply { maximumFractionDigits = if (unit == 0) 0 else 1 }.format(amount) + " " + units[unit]
    }
    private inner class TransferRow {
        val view = ui.column().apply {
            setPadding(0, ui.dp(14), 0, ui.dp(6))
            layoutParams = LinearLayout.LayoutParams(-1, -2)
        }
        private val title = ui.text("", 14f, bold = true).apply { maxLines = 1; ellipsize = TextUtils.TruncateAt.MIDDLE }
        private val detail = ui.text("", 12f, muted = true)
        private val error = ui.text("", 12f).apply { setTextColor(Color.rgb(159, 48, 48)) }
        private val progress = ProgressBar(context, null, android.R.attr.progressBarStyleHorizontal).apply {
            max = 1000
            progressTintList = ColorStateList.valueOf(ui.accent)
            indeterminateTintList = ColorStateList.valueOf(ui.accent)
            layoutParams = LinearLayout.LayoutParams(-1, ui.dp(5)).apply { bottomMargin = ui.dp(4) }
        }
        private var current: FileTransferItem? = null
        private val action = ui.button("Cancel") {
            current?.let { item ->
                try {
                    if (active(item)) FileTransfers.cancel(item.id) else FileTransfers.openReceived(context, item.id)
                } catch (failure: Exception) {
                    Toast.makeText(context, failure.message ?: "File action is unavailable", Toast.LENGTH_LONG).show()
                }
            }
        }
        init {
            val heading = ui.row().apply { gravity = Gravity.CENTER_VERTICAL }
            val description = ui.column().apply { layoutParams = LinearLayout.LayoutParams(0, -2, 1f) }
            description.addView(title); description.addView(detail)
            heading.addView(description)
            action.layoutParams = LinearLayout.LayoutParams(ui.dp(76), -2).apply { marginStart = ui.dp(8) }
            heading.addView(action)
            view.addView(heading); view.addView(progress); view.addView(error)
        }
        fun update(item: FileTransferItem) {
            current = item
            title.text = item.name
            title.contentDescription = item.name
            val label = when (item.status) {
                "queued" -> "Queued"
                "waiting" -> "Waiting for desktop"
                "transferring" -> if (item.direction == "send") "Sending" else "Receiving"
                "complete" -> if (item.direction == "send") "Sent" else "Received"
                "failed" -> "Failed"
                else -> "Cancelled"
            }
            detail.text = item.size?.let { context.getString(R.string.file_transfer_progress_known, label, bytes(item.transferred), bytes(it)) }
                ?: context.getString(R.string.file_transfer_progress_unknown, label, bytes(item.transferred))
            error.text = item.error.orEmpty()
            error.visibility = if (item.error.isNullOrEmpty()) View.GONE else View.VISIBLE
            progress.visibility = if (active(item)) View.VISIBLE else View.GONE
            progress.isIndeterminate = item.size == null
            progress.progress = if (item.size == null || item.size == 0L) 0 else ((item.transferred.toDouble() / item.size * 1000).toInt()).coerceIn(0, 1000)
            action.setText(if (active(item)) R.string.cancel_file_transfer else R.string.open_received_file)
            action.contentDescription = (if (active(item)) "Cancel " else "Open ") + item.name
            action.visibility = if (active(item) || (item.direction == "receive" && item.status == "complete")) View.VISIBLE else View.GONE
        }
    }
}
