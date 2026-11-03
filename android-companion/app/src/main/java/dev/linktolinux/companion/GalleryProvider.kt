package dev.linktolinux.companion

import android.content.ContentUris
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import android.util.Base64
import android.util.Size
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayOutputStream

object GalleryProvider {
    private const val MAX_PHOTO_BYTES = 20 * 1024 * 1024
    fun list(context: Context, offset: Int): JSONObject {
        require(offset in 0..100000) { "Invalid photo offset" }
        val photos = JSONArray()
        val projection = arrayOf(MediaStore.Images.Media._ID, MediaStore.Images.Media.DISPLAY_NAME,
            MediaStore.Images.Media.SIZE, MediaStore.Images.Media.DATE_MODIFIED)
        context.contentResolver.query(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, projection, null, null,
            "${MediaStore.Images.Media.DATE_MODIFIED} DESC, ${MediaStore.Images.Media._ID} DESC")?.use { cursor ->
            cursor.moveToPosition(offset - 1)
            while (photos.length() < 60 && cursor.moveToNext()) {
                photos.put(JSONObject().put("id", cursor.getLong(0).toString()).put("name", cursor.getString(1))
                    .put("size", cursor.getLong(2)).put("modified", cursor.getLong(3) * 1000))
            }
            return JSONObject().put("photos", photos).put("hasMore", offset + photos.length() < cursor.count)
        }
        return JSONObject().put("photos", photos).put("hasMore", false)
    }
    private fun uri(id: String): Uri {
        val value = id.toLongOrNull()
        require(value != null && value > 0) { "Invalid photo identity" }
        return ContentUris.withAppendedId(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, value)
    }
    fun thumbnail(context: Context, id: String): JSONObject {
        val uri = uri(id)
        val bitmap = if (Build.VERSION.SDK_INT >= 29) {
            context.contentResolver.loadThumbnail(uri, Size(320, 240), null)
        } else {
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            context.contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, bounds) }
            val options = BitmapFactory.Options().apply {
                inSampleSize = (maxOf(bounds.outWidth, bounds.outHeight) / 320).coerceAtLeast(1)
            }
            context.contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, options) }
                ?: throw IllegalStateException("Cannot read photo thumbnail")
        }
        try {
            val out = ByteArrayOutputStream()
            bitmap.compress(Bitmap.CompressFormat.JPEG, 75, out)
            return JSONObject().put("dataUrl", "data:image/jpeg;base64," + Base64.encodeToString(out.toByteArray(), Base64.NO_WRAP))
        } finally { bitmap.recycle() }
    }
    fun photo(context: Context, id: String): JSONObject {
        val uri = uri(id)
        var name = "photo.jpg"
        context.contentResolver.query(uri, arrayOf(MediaStore.Images.Media.DISPLAY_NAME, MediaStore.Images.Media.SIZE), null, null, null)?.use {
            check(it.moveToFirst()) { "Photo is no longer available" }
            name = it.getString(0)
            require(it.getLong(1) <= MAX_PHOTO_BYTES) { "Photo exceeds the 20 MiB transfer limit" }
        }
        val bytes = context.contentResolver.openInputStream(uri)?.use { input ->
            val out = ByteArrayOutputStream()
            val buffer = ByteArray(64 * 1024)
            while (true) {
                val count = input.read(buffer)
                if (count < 0) break
                require(out.size() + count <= MAX_PHOTO_BYTES) { "Photo exceeds the 20 MiB transfer limit" }
                out.write(buffer, 0, count)
            }
            out.toByteArray()
        } ?: throw IllegalStateException("Photo is not readable")
        return JSONObject().put("name", name).put("data", Base64.encodeToString(bytes, Base64.NO_WRAP))
    }
}
