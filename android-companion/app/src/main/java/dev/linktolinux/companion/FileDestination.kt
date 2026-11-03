package dev.linktolinux.companion

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import androidx.core.content.FileProvider
import java.io.Closeable
import java.io.File
import java.io.OutputStream

internal class FileDestination(private val context: Context, name: String, mime: String) : Closeable {
    private var published = false
    private var pendingUri: Uri? = null
    private var temporary: File? = null
    private var finalFile: File? = null
    val output: OutputStream
    init {
        if (Build.VERSION.SDK_INT >= 29) {
            val values = ContentValues().apply {
                put(MediaStore.Downloads.DISPLAY_NAME, name)
                put(MediaStore.Downloads.MIME_TYPE, mime)
                put(MediaStore.Downloads.RELATIVE_PATH, "${Environment.DIRECTORY_DOWNLOADS}/LinkToLinux")
                put(MediaStore.Downloads.IS_PENDING, 1)
            }
            val uri = context.contentResolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values) ?: throw java.io.IOException("Cannot create download")
            pendingUri = uri
            try { output = context.contentResolver.openOutputStream(uri, "w") ?: throw java.io.IOException("Cannot write download") }
            catch (error: Exception) { context.contentResolver.delete(uri, null, null); throw error }
        } else {
            @Suppress("DEPRECATION") val directory = File(Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS), "LinkToLinux")
            require(directory.isDirectory || directory.mkdirs()) { "Downloads is unavailable" }
            val temp = File.createTempFile(".transfer-", ".part", context.cacheDir)
            temporary = temp
            finalFile = File(directory, name)
            output = try { temp.outputStream() }
            catch (error: Exception) { temp.delete(); throw error }
        }
    }
    fun publish(cancelled: () -> Boolean, beginCommit: () -> Unit): Uri {
        check(!cancelled()) { "Transfer cancelled" }
        output.close()
        val uri = pendingUri
        if (Build.VERSION.SDK_INT >= 29 && uri != null) {
            beginCommit()
            require(context.contentResolver.update(uri, ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) }, null, null) == 1) { "Cannot publish download" }
            published = true
            return uri
        }
        val temp = requireNotNull(temporary)
        val target = requireNotNull(finalFile)
        val stem = target.name.substringBeforeLast('.', target.name)
        val extension = target.name.substringAfterLast('.', "").let { if (it.isEmpty()) "" else ".$it" }
        for (index in 0..10000) {
            val candidate = if (index == 0) target else File(target.parentFile, "$stem ($index)$extension")
            val descriptor = try {
                android.system.Os.open(candidate.path, android.system.OsConstants.O_WRONLY or android.system.OsConstants.O_CREAT or android.system.OsConstants.O_EXCL or android.system.OsConstants.O_NOFOLLOW, 0x180)
            } catch (error: android.system.ErrnoException) {
                if (error.errno == android.system.OsConstants.EEXIST) continue
                throw java.io.IOException("Cannot create download", error)
            }
            val owned = try { android.system.Os.fstat(descriptor) }
            catch (error: Exception) { android.system.Os.close(descriptor); throw error }
            try {
                java.io.FileOutputStream(descriptor).use { destination ->
                    temp.inputStream().use { source ->
                        val buffer = ByteArray(65536)
                        while (true) {
                            check(!cancelled()) { "Transfer cancelled" }
                            val count = source.read(buffer)
                            if (count < 0) break
                            destination.write(buffer, 0, count)
                        }
                    }
                    destination.fd.sync()
                }
                check(!cancelled()) { "Transfer cancelled" }
                val current = android.system.Os.lstat(candidate.path)
                require(current.st_ino == owned.st_ino && current.st_dev == owned.st_dev) { "Download destination changed" }
                val result = FileProvider.getUriForFile(context, "${context.packageName}.files", candidate)
                beginCommit()
                published = true
                return result
            } catch (error: Exception) {
                try {
                    val current = android.system.Os.lstat(candidate.path)
                    if (current.st_ino == owned.st_ino && current.st_dev == owned.st_dev) candidate.delete()
                } catch (cleanup: android.system.ErrnoException) {
                    if (cleanup.errno != android.system.OsConstants.ENOENT) error.addSuppressed(cleanup)
                }
                throw error
            }
        }
        throw java.io.IOException("Too many files with this name")
    }
    override fun close() {
        try { output.close() } finally {
            if (!published) pendingUri?.let { context.contentResolver.delete(it, null, null) }
            temporary?.delete()
        }
    }
}
