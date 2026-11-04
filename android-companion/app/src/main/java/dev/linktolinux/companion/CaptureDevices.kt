package dev.linktolinux.companion

import android.annotation.SuppressLint
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.media.MediaFormat
import android.media.MediaRecorder
import android.util.Log
import android.view.Surface

internal object CaptureSelection {
    fun audioRates(allowFallback: Boolean): List<Int> =
        if (allowFallback) listOf(48000, 44100, 32000, 16000) else listOf(48000)

    fun sizes(selected: CameraOption, cameras: List<CameraOption>): List<CameraSize> {
        val opposite = cameras.firstOrNull { it.frontFacing != selected.frontFacing }
        return selected.sizes.filter { maxOf(it.width, it.height) <= 1280 && minOf(it.width, it.height) <= 720 }
            .sortedWith(compareByDescending<CameraSize> { opposite == null || it in opposite.sizes }
                .thenByDescending { it.width * it.height })
    }

    fun <C, T> firstWorking(candidates: Iterable<C>, open: (C) -> T, onFailure: (Exception) -> Unit): T {
        var last: Exception? = null
        for (candidate in candidates) {
            try { return open(candidate) }
            catch (error: SecurityException) { throw error }
            catch (error: Exception) { last = error; onFailure(error) }
        }
        throw IllegalStateException("No supported capture configuration is available", last)
    }
}

internal data class CameraEncoding(val name: String, val size: CameraSize, val fps: Int, val bitrate: Int)
internal data class ConfiguredCamera(val codec: MediaCodec, val surface: Surface, val size: CameraSize, val fps: Int)

internal object CaptureDevices {
    @SuppressLint("MissingPermission")
    fun audio(allowFallback: Boolean): AudioRecord = CaptureSelection.firstWorking(
        CaptureSelection.audioRates(allowFallback), { rate ->
            val minimum = AudioRecord.getMinBufferSize(rate, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
            check(minimum > 0) { "Microphone sample rate is unavailable" }
            val audio = AudioRecord(MediaRecorder.AudioSource.MIC, rate, AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT, maxOf(minimum, 8192))
            try {
                check(audio.state == AudioRecord.STATE_INITIALIZED) { "Microphone initialization failed" }
                audio.startRecording()
                check(audio.recordingState == AudioRecord.RECORDSTATE_RECORDING) { "Microphone capture was denied" }
                audio
            } catch (error: Exception) {
                audio.release()
                throw error
            }
        }, { Log.w("PhoneCapture", "Trying another microphone configuration: ${it.javaClass.simpleName}") })

    fun camera(selected: CameraOption, cameras: List<CameraOption>): ConfiguredCamera {
        val sizes = CaptureSelection.sizes(selected, cameras)
        val candidates = mutableListOf<CameraEncoding>()
        val encoders = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.filter {
            it.isEncoder && it.supportedTypes.any { type -> type.equals(MediaFormat.MIMETYPE_VIDEO_AVC, true) }
        }
        for (size in sizes) for (fps in selected.frameRates.sortedBy { kotlin.math.abs(it - 24) }) for (info in encoders) {
            try {
                val capabilities = info.getCapabilitiesForType(MediaFormat.MIMETYPE_VIDEO_AVC)
                val video = capabilities.videoCapabilities ?: continue
                if (MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface !in capabilities.colorFormats ||
                    !video.areSizeAndRateSupported(size.width, size.height, fps.toDouble())) continue
                val bitrate = video.bitrateRange.clamp(minOf(2_000_000, maxOf(256_000, size.width * size.height * 3)))
                candidates.add(CameraEncoding(info.name, size, fps, bitrate))
            } catch (error: IllegalArgumentException) {
                Log.w("PhoneCapture", "Encoder capabilities unavailable: ${error.javaClass.simpleName}")
            }
        }
        return CaptureSelection.firstWorking(candidates.take(48), { candidate ->
            val codec = MediaCodec.createByCodecName(candidate.name)
            var surface: Surface? = null
            try {
                val format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, candidate.size.width, candidate.size.height).apply {
                    setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
                    setInteger(MediaFormat.KEY_BIT_RATE, candidate.bitrate)
                    setInteger(MediaFormat.KEY_FRAME_RATE, candidate.fps)
                    setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)
                }
                codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
                surface = codec.createInputSurface()
                codec.start()
                ConfiguredCamera(codec, surface, candidate.size, candidate.fps)
            } catch (error: Exception) {
                surface?.release()
                codec.release()
                throw error
            }
        }, { Log.w("PhoneCapture", "Trying another camera encoder configuration: ${it.javaClass.simpleName}") })
    }
}
