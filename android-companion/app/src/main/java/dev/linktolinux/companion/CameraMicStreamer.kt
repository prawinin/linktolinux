package dev.linktolinux.companion

import android.Manifest
import android.annotation.SuppressLint
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.media.MediaRecorder
import android.os.Bundle
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.util.Base64
import android.util.Log
import android.view.Surface
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import org.json.JSONObject
import java.util.concurrent.atomic.AtomicBoolean

class CameraMicStreamer : Service() {
    @Volatile private var running = false
    private var camera: CameraDevice? = null
    private var session: CameraCaptureSession? = null
    private var encoder: MediaCodec? = null
    private var surface: Surface? = null
    private var recorder: AudioRecord? = null
    private var cameraThread: HandlerThread? = null
    private var videoThread: Thread? = null
    private var audioThread: Thread? = null
    private var requestId = ""
    private val acknowledged = AtomicBoolean(false)
    private val failed = AtomicBoolean(false)
    @Volatile private var videoReady = false
    @Volatile private var audioReady = false
    private var mode = "studio"
    @Volatile var cameraBusy = false
        private set
    val videoActive: Boolean get() = running && mode != "mic"
    private var captureSize: CameraSize? = null
    private var cameraHandler: Handler? = null
    @Volatile private var cameraGeneration = 0
    private var closingCamera: CameraDevice? = null
    private var pendingCamera: CameraOption? = null
    @Volatile private var encodedRotation = 0
    @Volatile private var awaitingKeyFrame = false

    internal fun supportsCamera(option: CameraOption): Boolean = captureSize in option.sizes
    internal fun requestKeyFrame() {
        if (!videoActive) return
        awaitingKeyFrame = true
        cameraHandler?.post {
            try { encoder?.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) }) }
            catch (error: IllegalStateException) { if (running) fail(error.message ?: "Cannot request a camera key frame") }
        }
    }
    internal fun switchCamera(option: CameraOption) {
        check(!cameraBusy) { "Camera is changing. Try again when it is ready." }
        check(supportsCamera(option)) { "These cameras cannot share a video size. Stop capture before switching cameras." }
        val handler = cameraHandler ?: throw IllegalStateException("Camera is not ready")
        cameraBusy = true
        CameraControls.changed()
        handler.post {
            if (!running) return@post
            pendingCamera = option
            cameraGeneration += 1
            val previous = camera
            camera = null
            session = null
            if (previous == null) {
                pendingCamera = null
                openCamera(option, handler)
            } else { closingCamera = previous; previous.close() }
            val generation = cameraGeneration
            handler.postDelayed({
                if (running && cameraBusy && generation == cameraGeneration) fail("Camera switch timed out. Stop capture and retry.")
            }, 8000)
        }
    }
    private var stopRequestId: String? = null

    fun requestStop(id: String) {
        stopRequestId = id
        stopSelf()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == "stop") { stopSelf(); return START_NOT_STICKY }
        val id = intent?.getStringExtra("id").orEmpty()
        if (running) {
            CompanionConnection.respond(id, error = "Capture is already running. Stop it before changing mode.")
            return START_NOT_STICKY
        }
        requestId = id
        mode = intent?.getStringExtra("mode") ?: "studio"
        try {
            require(mode in setOf("webcam", "mic", "studio")) { "Invalid capture mode" }
            if (mode != "mic") checkPermission(Manifest.permission.CAMERA)
            if (mode != "webcam") checkPermission(Manifest.permission.RECORD_AUDIO)
            val manager = getSystemService(NOTIFICATION_SERVICE) as NotificationManager
            if (Build.VERSION.SDK_INT >= 26) {
                manager.createNotificationChannel(NotificationChannel("capture", "Camera and microphone", NotificationManager.IMPORTANCE_LOW))
            }
            val stop = android.app.PendingIntent.getService(this, 1, Intent(this, CameraMicStreamer::class.java).setAction("stop"), android.app.PendingIntent.FLAG_IMMUTABLE)
            val notification = NotificationCompat.Builder(this, "capture")
                .setContentTitle("LinkToLinux capture active").setContentText("Your phone is sharing $mode with Linux.")
                .setSmallIcon(R.drawable.ic_notification).setOngoing(true)
                .addAction(android.R.drawable.ic_media_pause, "Stop capture", stop).build()
            var types = 0
            if (Build.VERSION.SDK_INT >= 30) {
                if (mode != "mic") types = types or ServiceInfo.FOREGROUND_SERVICE_TYPE_CAMERA
                if (mode != "webcam") types = types or ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
            }
            if (Build.VERSION.SDK_INT >= 29) startForeground(1002, notification, types) else startForeground(1002, notification)
            running = true
            CompanionConnection.captureChanged(true)
            if (mode != "webcam") startAudio()
            if (mode != "mic") startCamera()
        } catch (error: Exception) { fail(error.message ?: "Cannot start phone capture") }
        return START_NOT_STICKY
    }
    override fun onCreate() { super.onCreate(); active = this }
    private fun checkPermission(permission: String) {
        check(ContextCompat.checkSelfPermission(this, permission) == PackageManager.PERMISSION_GRANTED) {
            "Grant camera and microphone permissions in the companion, then retry."
        }
    }
    @Synchronized private fun ready() {
        if ((mode == "mic" || videoReady) && (mode == "webcam" || audioReady) && acknowledged.compareAndSet(false, true)) {
            CompanionConnection.respond(requestId, JSONObject().put("mode", mode))
        }
    }
    private fun fail(message: String) {
        if (!failed.compareAndSet(false, true)) return
        if (acknowledged.compareAndSet(false, true)) CompanionConnection.respond(requestId, error = message)
        else CompanionConnection.send(JSONObject().put("type", "capture_error").put("error", message))
        Handler(mainLooper).post { stopSelf() }
    }
    @SuppressLint("MissingPermission")
    private fun startAudio() {
        val bufferSize = AudioRecord.getMinBufferSize(48000, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        check(bufferSize > 0) { "48 kHz microphone recording is unavailable" }
        val audio = AudioRecord(MediaRecorder.AudioSource.MIC, 48000, AudioFormat.CHANNEL_IN_MONO,
            AudioFormat.ENCODING_PCM_16BIT, maxOf(bufferSize, 8192))
        recorder = audio
        check(audio.state == AudioRecord.STATE_INITIALIZED) { "Microphone initialization failed" }
        audio.startRecording()
        check(audio.recordingState == AudioRecord.RECORDSTATE_RECORDING) { "Microphone capture was denied" }
        audioReady = true
        ready()
        audioThread = Thread({
            try {
                val buffer = ByteArray(4096)
                while (running) {
                    val count = audio.read(buffer, 0, buffer.size)
                    check(count >= 0 || !running) { "Microphone read failed: $count" }
                    if (count > 0 && !CompanionConnection.send(JSONObject().put("type", "audio")
                            .put("codec", "pcm_s16le").put("sampleRate", 48000).put("channels", 1)
                            .put("data", Base64.encodeToString(buffer, 0, count, Base64.NO_WRAP)))) break
                }
            } catch (error: Exception) { if (running) fail(error.message ?: "Microphone capture failed") }
        }, "phone-audio").apply { start() }
    }
    @SuppressLint("MissingPermission")
    private fun startCamera() {
        val options = CameraControls.options(this)
        val selected = CameraControls.selected(this, options)
        val size = CameraSelection.size(selected, options)
        captureSize = size
        encodedRotation = CameraControls.rotation(this)
        CameraControls.saveLens(this, selected.frontFacing)
        cameraBusy = true
        CameraControls.changed()
        val format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, size.width, size.height).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE, 2_000_000)
            setInteger(MediaFormat.KEY_FRAME_RATE, 24)
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)
        }
        val codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
        encoder = codec
        codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        val target = codec.createInputSurface()
        surface = target
        codec.start()
        val thread = HandlerThread("phone-camera").apply { start() }
        cameraThread = thread
        val handler = Handler(thread.looper)
        cameraHandler = handler
        openCamera(selected, handler)
        videoThread = Thread({
            try {
                val info = MediaCodec.BufferInfo()
                var config = ByteArray(0)
                while (running) {
                    val index = codec.dequeueOutputBuffer(info, 10000)
                    if (index >= 0) {
                        val output = codec.getOutputBuffer(index)
                        if (output != null && info.size > 0) {
                            output.position(info.offset)
                            output.limit(info.offset + info.size)
                            val bytes = ByteArray(info.size)
                            output.get(bytes)
                            val isConfig = info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0
                            if (isConfig) config = bytes
                            val keyFrame = info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0
                            if (awaitingKeyFrame && !isConfig && !keyFrame) {
                                codec.releaseOutputBuffer(index, false)
                                continue
                            }
                            if (keyFrame) { encodedRotation = CameraControls.rotation(this); awaitingKeyFrame = false }
                            val data = if (!isConfig && keyFrame) config + bytes else bytes
                            if (!CompanionConnection.send(JSONObject().put("type", "video").put("codec", "h264")
                                    .put("width", size.width).put("height", size.height).put("rotation", encodedRotation)
                                    .put("data", Base64.encodeToString(data, Base64.NO_WRAP)))) break
                            if (!isConfig) { videoReady = true; ready() }
                        }
                        codec.releaseOutputBuffer(index, false)
                    }
                }
            } catch (error: Exception) { if (running) fail(error.message ?: "Camera encoding failed") }
        }, "phone-video").apply { start() }
    }
    @SuppressLint("MissingPermission")
    private fun openCamera(option: CameraOption, handler: Handler) {
        val manager = getSystemService(CAMERA_SERVICE) as CameraManager
        val target = surface ?: throw IllegalStateException("Camera encoder surface is unavailable")
        val generation = cameraGeneration
        manager.openCamera(option.id, object : CameraDevice.StateCallback() {
            override fun onOpened(device: CameraDevice) {
                if (!running || generation != cameraGeneration) { device.close(); return }
                camera = device
                try {
                    device.createCaptureSession(listOf(target), object : CameraCaptureSession.StateCallback() {
                        override fun onConfigured(capture: CameraCaptureSession) {
                            if (!running || generation != cameraGeneration || camera !== device) { capture.close(); return }
                            session = capture
                            try {
                                val request = device.createCaptureRequest(CameraDevice.TEMPLATE_RECORD).apply {
                                    addTarget(target)
                                    set(android.hardware.camera2.CaptureRequest.CONTROL_MODE, android.hardware.camera2.CaptureRequest.CONTROL_MODE_AUTO)
                                }
                                capture.setRepeatingRequest(request.build(), null, handler)
                                CameraControls.saveLens(this@CameraMicStreamer, option.frontFacing)
                                cameraBusy = false
                                requestKeyFrame()
                                CameraControls.changed()
                            } catch (error: Exception) { fail(error.message ?: "Camera request failed") }
                        }
                        override fun onConfigureFailed(capture: CameraCaptureSession) {
                            capture.close()
                            if (running && generation == cameraGeneration) fail("Camera session configuration failed")
                        }
                    }, handler)
                } catch (error: Exception) { if (running && generation == cameraGeneration) fail(error.message ?: "Camera session failed") }
            }
            override fun onClosed(device: CameraDevice) {
                if (!running || closingCamera !== device) return
                closingCamera = null
                val next = pendingCamera ?: return
                pendingCamera = null
                try { openCamera(next, handler) }
                catch (error: Exception) { fail(error.message ?: "Cannot switch phone camera") }
            }
            override fun onDisconnected(device: CameraDevice) {
                device.close()
                if (running && generation == cameraGeneration) fail("Phone camera disconnected")
            }
            override fun onError(device: CameraDevice, error: Int) {
                device.close()
                if (running && generation == cameraGeneration) fail("Camera unavailable (error $error). Close other camera apps.")
            }
        }, handler)
    }
    override fun onDestroy() {
        running = false
        cameraGeneration += 1
        pendingCamera = null
        cameraBusy = false
        CameraControls.changed()
        CompanionConnection.captureChanged(false)
        if (active === this) active = null
        session?.close()
        camera?.close()
        closingCamera?.close()
        try { recorder?.stop() } catch (error: Exception) { Log.w("PhoneCapture", "Audio stop failed", error) }
        audioThread?.join(500)
        recorder?.release()
        videoThread?.join(500)
        try { encoder?.stop() } catch (error: Exception) { Log.w("PhoneCapture", "Encoder stop failed", error) }
        encoder?.release()
        surface?.release()
        cameraThread?.quitSafely()
        CompanionConnection.send(JSONObject().put("type", "capture_stopped"))
        stopRequestId?.let { CompanionConnection.respond(it) }
        super.onDestroy()
    }
    override fun onBind(intent: Intent?): IBinder? = null
    companion object {
        @Volatile var active: CameraMicStreamer? = null
            private set
    }
}
