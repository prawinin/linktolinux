package dev.linktolinux.companion

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.BatteryManager
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import org.json.JSONObject

object CompanionConnection {
    @Volatile var ui = ConnectionUiState(ConnectionPhase.DISCONNECTED, "Disconnected")
        private set
    val connected: Boolean get() = ui.phase == ConnectionPhase.CONNECTED
    val status: String get() = ui.message
    @Volatile var onStatus: ((String) -> Unit)? = null
    @Volatile var onConnectionStatus: ((ConnectionUiState) -> Unit)? = null
    @Volatile var onRequest: ((JSONObject) -> Unit)? = null
    @Volatile private var session: DesktopSession? = null

    val serverFeatures: Set<String> get() = session?.serverFeatures ?: emptySet()
    fun send(message: JSONObject): Boolean = session?.send(message) ?: false
    fun respond(id: String, result: JSONObject = JSONObject(), error: String? = null) {
        val message = JSONObject().put("type", "response").put("id", id)
        if (error == null) message.put("result", result) else message.put("error", error.take(500))
        send(message)
    }
    @Synchronized fun updateStatus(value: String, phase: ConnectionPhase = ui.phase) {
        ui = ui.copy(phase = phase, message = value)
        onStatus?.invoke(value)
        onConnectionStatus?.invoke(ui)
    }
    @Synchronized fun captureChanged(active: Boolean) {
        ui = ui.copy(captureActive = active)
        onStatus?.invoke(status)
    }
    @Synchronized fun disconnect(reason: String = "Disconnected") {
        FileTransfers.disconnected()
        val old = session
        session = null
        try { old?.close() } catch (error: java.io.IOException) { Log.w("CompanionConnection", "Socket close failed: ${error.javaClass.simpleName}") }
        updateStatus(reason, ConnectionPhase.DISCONNECTED)
    }
    fun run(context: Context, endpoint: DesktopEndpoint, remember: Boolean, isActive: () -> Boolean, gallery: GalleryRequests): FailureKind? {
        val prefs = context.getSharedPreferences("linktolinux", Context.MODE_PRIVATE)
        val deviceId = prefs.getString("device_id", null) ?: java.util.UUID.randomUUID().toString().also {
            prefs.edit().putString("device_id", it).apply()
        }
        val client = DesktopSession(endpoint, deviceId, "${Build.MANUFACTURER} ${Build.MODEL}".trim())
        synchronized(this) {
            if (!isActive()) { client.close(); return null }
            session = client
        }
        var failure: FailureKind? = null
        try {
            client.run(onConnected = {
                synchronized(this) {
                    if (session !== client || !isActive()) { client.close() }
                    else {
                        var saved = true
                        if (remember) try { PairingStore(context).save(endpoint) }
                        catch (error: Exception) { saved = false; Log.w("PairingStore", "Could not save pairing: ${error.javaClass.simpleName}") }
                        ClipboardBridge.connected()
                        FileTransfers.connected(context, endpoint, client.serverFeatures.contains("files-v1"))
                        updateStatus(if (saved) "Connected to ${endpoint.host}:${endpoint.port}" else "Connected. Could not remember pairing on this phone.", ConnectionPhase.CONNECTED)
                    }
                }
            }, onMessage = { message ->
                if (FileTransfers.handle(message) || gallery.handle(message, client::send)) return@run
                if (message.optString("type") == "request") {
                    if (ClipboardBridge.handles(message.optString("command"))) {
                        ClipboardBridge.request(context, message)
                    } else if (message.optString("command") == "capture_stop") {
                        android.os.Handler(context.mainLooper).post {
                            val capture = CameraMicStreamer.active
                            if (capture == null) respond(message.optString("id")) else capture.requestStop(message.optString("id"))
                        }
                    } else {
                        val handler = onRequest
                        if (handler == null) respond(message.optString("id"), error = "Open the companion on your phone to continue.") else handler(message)
                    }
                }
            }, heartbeat = {
                val battery = context.getSystemService(Context.BATTERY_SERVICE) as BatteryManager
                JSONObject().put("type", "ping").put("battery", battery.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY)).put("charging", battery.isCharging)
            })
        } catch (error: SessionFailure) {
            failure = error.kind
            synchronized(this) {
                if (session === client) {
                    if (error.kind == FailureKind.PAIRING) PairingStore(context).clear()
                    updateStatus(error.message ?: "Connection failed. Retry from the phone.", ConnectionPhase.ERROR)
                }
            }
        } finally {
            synchronized(this) {
                if (session === client) {
                    session = null
                    FileTransfers.disconnected()
                    if (connected) updateStatus("Disconnected", ConnectionPhase.DISCONNECTED)
                    context.stopService(Intent(context, CameraMicStreamer::class.java))
                }
            }
        }
        return failure
    }
}

class CompanionService : Service() {
    private var worker: Thread? = null
    private val main = android.os.Handler(android.os.Looper.getMainLooper())
    private fun notification(state: ConnectionUiState): android.app.Notification {
        val open = android.app.PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), android.app.PendingIntent.FLAG_IMMUTABLE)
        val clipboard = android.app.PendingIntent.getActivity(this, 2,
            Intent(this, MainActivity::class.java).putExtra("send_clipboard", true)
                .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP),
            android.app.PendingIntent.FLAG_IMMUTABLE or android.app.PendingIntent.FLAG_UPDATE_CURRENT)
        val disconnect = android.app.PendingIntent.getService(this, 3,
            Intent(this, CompanionService::class.java).setAction(DISCONNECT), android.app.PendingIntent.FLAG_IMMUTABLE)
        val text = if (state.phase == ConnectionPhase.CONNECTED) "Your phone is connected. ${state.message}" else state.message
        return NotificationCompat.Builder(this, "connection")
            .setContentTitle(if (state.phase == ConnectionPhase.CONNECTED) "Connected to desktop" else "LinkToLinux")
            .setContentText(text).setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setSmallIcon(R.drawable.ic_notification).setContentIntent(open)
            .setOnlyAlertOnce(true).setOngoing(true)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .apply {
                if (state.phase == ConnectionPhase.CONNECTED) addAction(R.drawable.ic_notification, "Send clipboard", clipboard)
            }
            .addAction(R.drawable.ic_notification, "Disconnect", disconnect)
            .build()
    }
    private lateinit var gallery: GalleryRequests
    private var persistent = false
    @Volatile private var running = true
    private val restartMode: Int get() = if (persistent) START_STICKY else START_NOT_STICKY

    override fun onCreate() {
        super.onCreate()
        gallery = GalleryRequests(applicationContext)
        ClipboardBridge.attach(this)
        val manager = getSystemService(NOTIFICATION_SERVICE) as NotificationManager
        if (Build.VERSION.SDK_INT >= 26) {
            manager.createNotificationChannel(NotificationChannel("connection", "Phone connection", NotificationManager.IMPORTANCE_LOW))
        }
        CompanionConnection.onConnectionStatus = { state ->
            main.post {
                if (running) manager.notify(1001, notification(state))
            }
        }
        val notification = notification(ConnectionUiState(ConnectionPhase.CONNECTING, "Connecting to desktop…"))
        try {
            val type = if (Build.VERSION.SDK_INT >= 29) android.content.pm.ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE else 0
            androidx.core.app.ServiceCompat.startForeground(this, 1001, notification, type)
        } catch (error: RuntimeException) {
            running = false
            CompanionConnection.updateStatus("Android could not start the connection service. Open the app and check background access.", ConnectionPhase.ERROR)
            stopSelf()
        }
    }
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == DISCONNECT) {
            running = false
            CompanionConnection.disconnect()
            getSharedPreferences("linktolinux", MODE_PRIVATE).edit().putBoolean("auto_connect", false).apply()
            stopSelf()
            return START_NOT_STICKY
        }
        if (intent?.action == FileTransfers.CANCEL_ACTION) {
            intent.getStringExtra("id")?.let(FileTransfers::cancel)
            if (worker?.isAlive == true) return restartMode
            stopSelf(startId)
            return START_NOT_STICKY
        }
        if (worker?.isAlive == true) return restartMode
        if (!running) { stopSelf(); return START_NOT_STICKY }
        val endpoint = try {
            if (intent == null) {
                val prefs = getSharedPreferences("linktolinux", MODE_PRIVATE)
                ConnectionRecovery.restore(prefs.getBoolean("auto_connect", false), PairingStore(this).load())
            } else DesktopEndpoint.parse(intent.getStringExtra("host").orEmpty(),
                intent.getIntExtra("port", 0).toString(), intent.getStringExtra("token").orEmpty())
        } catch (error: ConnectionInputException) {
            CompanionConnection.updateStatus(error.message ?: "Check the desktop pairing details.", ConnectionPhase.ERROR)
            null
        }
        if (endpoint == null) { stopSelf(); return START_NOT_STICKY }
        persistent = intent?.getBooleanExtra("remember", true) ?: true
        CompanionConnection.updateStatus("Connecting to desktop…", ConnectionPhase.CONNECTING)
        worker = Thread({
            try {
                ConnectionRecovery.run({ running }, connect = {
                    CompanionConnection.run(this, endpoint, persistent, { running }, gallery)
                }, onRetry = { delay ->
                    CompanionConnection.updateStatus("Desktop unavailable. Reconnecting in ${delay / 1000}s…", ConnectionPhase.CONNECTING)
                })
            } finally { stopSelf() }
        }, "companion-reader").apply { start() }
        return restartMode
    }
    override fun onDestroy() {
        running = false
        CompanionConnection.onConnectionStatus = null
        main.removeCallbacksAndMessages(null)
        worker?.interrupt()
        stopService(Intent(this, CameraMicStreamer::class.java))
        gallery.close()
        ClipboardBridge.detach()
        val state = CompanionConnection.ui
        CompanionConnection.disconnect(if (state.phase == ConnectionPhase.CONNECTED) "Disconnected" else state.message)
        if (state.phase == ConnectionPhase.ERROR) CompanionConnection.updateStatus(state.message, ConnectionPhase.ERROR)
        stopForeground(STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }
    override fun onBind(intent: Intent?): IBinder? = null
    companion object { private const val DISCONNECT = "dev.linktolinux.companion.DISCONNECT" }
}
