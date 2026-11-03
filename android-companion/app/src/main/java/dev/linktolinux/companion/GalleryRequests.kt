package dev.linktolinux.companion

import android.content.Context
import org.json.JSONObject
import java.io.Closeable
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

class GalleryRequests(context: Context) : Closeable {
    private val application = context.applicationContext
    private val executor = ThreadPoolExecutor(1, 2, 30, TimeUnit.SECONDS, ArrayBlockingQueue(16))
    fun handle(message: JSONObject, reply: (JSONObject) -> Boolean): Boolean {
        if (message.optString("type") != "request") return false
        val command = message.optString("command")
        if (!command.startsWith("gallery_")) return false
        val id = message.optString("id")
        val args = message.optJSONObject("args") ?: JSONObject()
        fun respond(result: JSONObject = JSONObject(), error: String? = null) {
            val response = JSONObject().put("type", "response").put("id", id)
            if (error == null) response.put("result", result) else response.put("error", error.take(500))
            reply(response)
        }
        try {
            executor.execute {
                try {
                    val result = when (command) {
                        "gallery_list" -> GalleryProvider.list(application, args.optInt("offset", 0))
                        "gallery_thumbnail" -> GalleryProvider.thumbnail(application, args.getString("photoId"))
                        "gallery_photo" -> GalleryProvider.photo(application, args.getString("photoId"))
                        else -> throw IllegalArgumentException("Unknown photo command")
                    }
                    respond(result)
                } catch (error: SecurityException) {
                    respond(error = "Photo access denied. Open Manage permissions on the phone and allow photos.")
                } catch (error: Exception) {
                    respond(error = error.message ?: "Photo request failed")
                }
            }
        } catch (error: RejectedExecutionException) { respond(error = "Phone is busy. Try again shortly.") }
        return true
    }
    override fun close() { executor.shutdownNow() }
}
