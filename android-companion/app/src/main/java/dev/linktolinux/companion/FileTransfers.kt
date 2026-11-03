package dev.linktolinux.companion

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.OpenableColumns
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import org.json.JSONObject
import java.io.Closeable
import java.util.UUID
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

data class FileTransferItem(val id: String, val name: String, val direction: String, val size: Long?, val transferred: Long, val status: String, val error: String?, val savedUri: String?, val startedAt: Long, val finishedAt: Long?)

object FileTransfers {
    const val CANCEL_ACTION = "dev.linktolinux.companion.CANCEL_FILE"
    @Volatile var onChanged: (() -> Unit)? = null
    @Volatile private var endpoint: DesktopEndpoint? = null
    @Volatile private var supported = false
    private var application: android.app.Application? = null
    private var generation = 0L
    private val lock = Any()
    private val entries = LinkedHashMap<String, FileTransferItem>()
    private val jobs = HashMap<String, Job>()
    private val sendWorker = Executors.newSingleThreadExecutor()
    private val receiveWorker = Executors.newSingleThreadExecutor()
    private val timer = Executors.newSingleThreadScheduledExecutor()
    private val terminal = setOf("complete", "failed", "cancelled")
    private class Job(val generation: Long, val source: Uri? = null) {
        @Volatile var cancelled = false
        @Volatile var wire: TimedFileSocket? = null
        @Volatile var stream: Closeable? = null
        val accepted = java.util.concurrent.CompletableFuture<String>()
        val metadataCancellation = android.os.CancellationSignal()
        val publication = FilePublication()
        var lastUpdate = 0L
    }
    val available: Boolean get() = endpoint != null && supported
    fun connected(context: Context, endpoint: DesktopEndpoint, supported: Boolean) {
        disconnected()
        synchronized(lock) { this.application = context.applicationContext as android.app.Application; this.endpoint = endpoint; this.supported = supported; generation++ }
        changed()
    }
    fun disconnected() {
        val ids = synchronized(lock) { endpoint = null; supported = false; generation++; jobs.keys.toList() }
        ids.forEach { finish(it, "cancelled", "Desktop disconnected") }
        changed()
    }
    fun items(): List<FileTransferItem> = synchronized(lock) { entries.values.toList().asReversed() }
    fun canReceive(context: Context): Boolean = Build.VERSION.SDK_INT >= 29 || ContextCompat.checkSelfPermission(context, Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
    fun send(context: Context, uris: List<Uri>) {
        require(available) { "Connect to an updated desktop to send files" }
        require(uris.isNotEmpty() && uris.size <= 16) { "Select between 1 and 16 files" }
        require(uris.all { it.scheme == "content" && !it.authority.isNullOrBlank() }) { "Select files using the document picker" }
        val app = context.applicationContext
        synchronized(lock) {
            require(available && jobs.size + uris.size <= 16) { "Transfer queue is full or desktop disconnected" }
            for (uri in uris) {
                val id = UUID.randomUUID().toString()
                entries[id] = FileTransferItem(id, "Preparing file", "send", null, 0, "queued", null, null, System.currentTimeMillis(), null)
                val job = Job(generation, uri); jobs[id] = job
                sendWorker.execute { prepareAndSend(app, id, job, uri) }
            }
            trim()
        }
        changed()
    }
    private fun prepareAndSend(app: Context, id: String, job: Job, uri: Uri) {
        try {
            synchronized(lock) { check(!job.cancelled && job.generation == generation && jobs[id] === job) }
            val metadata = ProviderTasks.call(cancelled = { job.metadataCancellation.cancel() }, isCancelled = { job.cancelled }) {
                var name: String? = null
                var size: Long? = null
                app.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null, job.metadataCancellation)?.use { cursor ->
                    if (cursor.moveToFirst()) {
                        val ni = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                        val si = cursor.getColumnIndex(OpenableColumns.SIZE)
                        if (ni >= 0 && !cursor.isNull(ni)) name = cursor.getString(ni)
                        if (si >= 0 && !cursor.isNull(si)) {
                            require(cursor.getType(si) == android.database.Cursor.FIELD_TYPE_INTEGER) { "Invalid file size" }
                            size = cursor.getLong(si).takeIf { it >= 0 }
                        }
                    }
                }
                name to size
            }
            val (name, size) = metadata
            val validName = FileTransferPolicy.name(name ?: "Shared file")
            require(size == null || size <= FileTransferPolicy.MAX_SIZE) { "File exceeds 8 GiB" }
            // Resolve MIME locally; getType() can block on a provider without cancellation support.
            val extension = validName.substringAfterLast('.', "").lowercase(java.util.Locale.ROOT)
            val mime = FileTransferPolicy.mime(android.webkit.MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension).orEmpty())
            synchronized(lock) {
                check(!job.cancelled && job.generation == generation && jobs[id] === job)
                entries[id] = entries.getValue(id).copy(name = validName, size = size, status = "waiting")
            }
            changed()
            check(CompanionConnection.send(JSONObject().put("type", "file_offer").put("id", id).put("name", validName).put("size", size ?: JSONObject.NULL).put("mime", mime)))
            transfer(id, job, job.accepted.get(60, TimeUnit.SECONDS), mime)
        } catch (error: Exception) {
            finish(id, if (job.cancelled) "cancelled" else "failed", if (job.cancelled) null else "Cannot prepare or send this file. Check file access and desktop connection.")
        }
    }

    fun handle(message: JSONObject): Boolean {
        val type = message.optString("type")
        if (type !in setOf("file_offer", "file_accept", "file_reject", "file_cancel", "file_result")) return false
        val id = try { FileTransferPolicy.id(message.getString("id")) } catch (error: Exception) { return true }
        try {
            when (type) {
                "file_offer" -> {
                    val name = FileTransferPolicy.name(message.getString("name")); val size = FileTransferPolicy.size(message)
                    val mime = FileTransferPolicy.mime(message.optString("mime")); val token = FileTransferPolicy.token(message.getString("token"))
                    synchronized(lock) {
                        require(available && jobs.size < 16 && !entries.containsKey(id)) { "Transfer unavailable or duplicate" }
                        require(canReceive(requireNotNull(application))) { "Allow storage permission to receive files" }
                        val job = Job(generation); jobs[id] = job
                        entries[id] = FileTransferItem(id, name, "receive", size, 0, "queued", null, null, System.currentTimeMillis(), null); trim()
                        receiveWorker.execute { transfer(id, job, token, mime) }
                    }
                    changed()
                }
                "file_accept" -> {
                    val token = FileTransferPolicy.token(message.getString("token"))
                    synchronized(lock) {
                        val job = jobs[id] ?: return true
                        if (entries[id]?.status != "waiting" || job.source == null) return true
                        entries[id] = entries.getValue(id).copy(status = "queued")
                        job.accepted.complete(token)
                    }
                    changed()
                }
                "file_reject" -> finish(id, "failed", "Desktop rejected the transfer")
                "file_cancel" -> finish(id, "cancelled", "Cancelled by desktop")
                "file_result" -> if (message.optString("status") in setOf("failed", "cancelled")) finish(id, message.getString("status"), "Desktop ended the transfer")
            }
        } catch (error: Exception) {
            CompanionConnection.send(JSONObject().put("type", "file_reject").put("id", id).put("error", "Invalid or unavailable file transfer"))
        }
        return true
    }
    private fun transfer(id: String, job: Job, token: String, mime: String) {
        try {
            val item: FileTransferItem; val target: DesktopEndpoint; val app: Context
            synchronized(lock) {
                check(!job.cancelled && job.generation == generation && jobs[id] === job) { "Transfer cancelled" }
                item = entries.getValue(id); target = requireNotNull(endpoint); app = requireNotNull(application)
                entries[id] = item.copy(status = "transferring")
            }
            changed(); notify(id)
            TimedFileSocket.connect(target.host, target.port).use { wire ->
                job.wire = wire; check(!job.cancelled) { "Transfer cancelled" }
                wire.json(JSONObject().put("type", "file_stream").put("id", id).put("token", token))
                val ready = wire.json(); require(ready.optString("type") == "file_ready" && ready.optString("id") == id) { "Desktop refused file stream" }
                wire.timeoutMs = 30000
                val source = job.source
                if (source != null) {
                    val descriptor = ProviderTasks.call(cancelled = { job.metadataCancellation.cancel() }, isCancelled = { job.cancelled }, discard = { descriptor: android.os.ParcelFileDescriptor -> descriptor.close() }) {
                        val opened = app.contentResolver.openFileDescriptor(source, "r", job.metadataCancellation) ?: throw java.io.IOException("Cannot read selected file")
                        if (job.cancelled || job.metadataCancellation.isCanceled) { opened.close(); throw java.io.IOException("File read cancelled") }
                        opened
                    }
                    DocumentSource(descriptor) { job.cancelled }.use { input ->
                        job.stream = input; check(!job.cancelled)
                        val receipt = FileWire.send(wire, input, item.size) { progress(id, job, it) }
                        synchronized(lock) { if (jobs[id] === job) entries[id] = entries.getValue(id).copy(transferred = receipt.size) }
                    }
                    finish(id, "complete", null)
                } else {
                    FileDestination(app, item.name, mime).use { destination ->
                        job.stream = destination.output; check(!job.cancelled)
                        val receipt = FileWire.receive(wire, destination.output, item.size) { progress(id, job, it) }
                        synchronized(lock) { check(!job.cancelled && jobs[id] === job && job.generation == generation) }
                        val uri = destination.publish(cancelled = { job.cancelled }, beginCommit = {
                            synchronized(lock) {
                                check(!job.cancelled && jobs[id] === job && job.generation == generation && job.publication.begin())
                            }
                        })
                        synchronized(lock) {
                            check(!job.cancelled && jobs[id] === job)
                            entries[id] = entries.getValue(id).copy(savedUri = uri.toString(), transferred = receipt.size)
                        }
                        try { wire.json(JSONObject().put("type", "file_complete").put("size", receipt.size).put("sha256", receipt.sha256)) }
                        finally { finish(id, "complete", null) }
                    }
                }
            }
        } catch (error: Exception) { finish(id, if (job.cancelled) "cancelled" else "failed", if (job.cancelled) null else "Transfer failed. Check Wi-Fi, file access and free storage.", ownFailure = true) }
        finally { job.stream = null; job.wire = null }
    }
    private fun progress(id: String, job: Job, total: Long) {
        val now = System.nanoTime()
        synchronized(lock) {
            check(!job.cancelled && jobs[id] === job) { "Transfer cancelled" }
            if (now - job.lastUpdate < 250000000) return
            job.lastUpdate = now; entries[id] = entries.getValue(id).copy(transferred = total)
        }
        changed(); notify(id)
    }
    fun cancel(id: String) {
        if (finish(id, "cancelled", null)) CompanionConnection.send(JSONObject().put("type", "file_cancel").put("id", id))
    }
    private fun finish(id: String, status: String, error: String?, ownFailure: Boolean = false): Boolean {
        val job = synchronized(lock) {
            val job = jobs[id] ?: return false
            if (status != "complete" && job.publication.committing && !ownFailure) return false
            if (status == "cancelled" && !job.publication.cancel()) return false
            jobs.remove(id)
            job.cancelled = status != "complete"; job.accepted.completeExceptionally(java.io.IOException("Transfer ended"))
            val item = entries.getValue(id)
            entries[id] = item.copy(status = status, error = error, transferred = if (status == "complete") item.size ?: item.transferred else item.transferred, finishedAt = System.currentTimeMillis())
            trim(); job
        }
        try { job.wire?.close() } catch (_: java.io.IOException) { /* Closing a cancelled stream can already have closed its socket. */ }
        // Provider streams may block in close; the network/control caller must stay responsive.
        if (status != "complete") timer.execute {
            job.metadataCancellation.cancel()
            try { job.stream?.close() } catch (_: java.io.IOException) { /* Cancellation is already terminal. */ }
        }
        changed(); notify(id)
        return true
    }
    private fun trim() { while (entries.size > 50) { val id = entries.values.firstOrNull { it.status in terminal }?.id ?: break; entries.remove(id) } }
    private fun changed() { onChanged?.invoke() }
    fun openReceived(context: Context, id: String) {
        val item = synchronized(lock) { entries[id] } ?: return
        require(item.direction == "receive" && item.status == "complete" && item.savedUri != null) { "File is not available" }
        context.startActivity(openIntent(context, Uri.parse(item.savedUri)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }
    private fun openIntent(context: Context, uri: Uri): Intent = Intent(Intent.ACTION_VIEW).setDataAndType(uri, FileTransferPolicy.mime(context.contentResolver.getType(uri).orEmpty())).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    private fun notify(id: String) {
        val app = application ?: return; val item = synchronized(lock) { entries[id] } ?: return
        val manager = app.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (Build.VERSION.SDK_INT >= 26) manager.createNotificationChannel(NotificationChannel("files", "File transfers", NotificationManager.IMPORTANCE_LOW))
        val builder = NotificationCompat.Builder(app, "files").setSmallIcon(R.drawable.ic_notification).setContentTitle(item.name).setOnlyAlertOnce(true)
            .setContentText("${if (item.direction == "send") "Send" else "Receive"}: ${item.status}")
        if (item.status !in terminal) {
            val cancel = PendingIntent.getService(app, id.hashCode(), Intent(app, CompanionService::class.java).setAction(CANCEL_ACTION).setData(Uri.parse("linktolinux://file/$id")).putExtra("id", id), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
            builder.setOngoing(true).addAction(R.drawable.ic_notification, "Cancel", cancel).setProgress(100, if (item.size != null && item.size > 0) ((item.transferred * 100) / item.size).toInt() else 0, item.size == null)
        } else if (item.status == "complete" && item.direction == "receive" && item.savedUri != null) {
            val open = PendingIntent.getActivity(app, id.hashCode(), openIntent(app, Uri.parse(item.savedUri)), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
            builder.setContentIntent(open).addAction(R.drawable.ic_notification, "Open", open).setAutoCancel(true)
        }
        try { manager.notify(id, id.hashCode(), builder.build()) } catch (_: SecurityException) { /* Notification permission is optional for transfer operation. */ }
    }
}
