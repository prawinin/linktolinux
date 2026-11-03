package dev.linktolinux.companion

import android.content.Context
import android.hardware.camera2.CameraAccessException
import android.util.Log
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraManager
import android.media.MediaCodec

data class CameraControlState(val frontFacing: Boolean, val rotationDegrees: Int, val canSwitch: Boolean, val videoActive: Boolean, val busy: Boolean = false)
internal data class CameraSize(val width: Int, val height: Int)
internal data class CameraOption(val id: String, val frontFacing: Boolean, val sizes: Set<CameraSize>)

internal object CameraSelection {
    fun validRotation(value: Int): Int = if (value in setOf(0, 90, 180, 270)) value else 0
    fun nextRotation(value: Int): Int = (validRotation(value) + 90) % 360
    fun select(cameras: List<CameraOption>, front: Boolean): CameraOption =
        cameras.firstOrNull { it.frontFacing == front } ?: cameras.firstOrNull()
        ?: throw IllegalStateException("No camera is available")
    fun size(selected: CameraOption, cameras: List<CameraOption>): CameraSize {
        val opposite = cameras.firstOrNull { it.frontFacing != selected.frontFacing }
        val supported = selected.sizes.filter { it.width <= 1280 && it.height <= 720 }
        val common = supported.filter { opposite == null || it in opposite.sizes }
        return (common.ifEmpty { supported }).maxByOrNull { it.width * it.height }
            ?: throw IllegalStateException("Camera has no supported video size at or below 720p")
    }
}

object CameraControls {
    private fun preferences(context: Context) = context.getSharedPreferences("camera-controls", Context.MODE_PRIVATE)
    internal fun rotation(context: Context): Int = CameraSelection.validRotation(preferences(context).getInt("rotation", 0))
    internal fun options(context: Context): List<CameraOption> {
        val manager = context.getSystemService(Context.CAMERA_SERVICE) as CameraManager
        return manager.cameraIdList.mapNotNull { id ->
            val info = manager.getCameraCharacteristics(id)
            val facing = info.get(CameraCharacteristics.LENS_FACING)
            if (facing != CameraCharacteristics.LENS_FACING_BACK && facing != CameraCharacteristics.LENS_FACING_FRONT) return@mapNotNull null
            val sizes = info.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)?.getOutputSizes(MediaCodec::class.java)
                ?.map { CameraSize(it.width, it.height) }?.toSet().orEmpty()
            if (sizes.isEmpty()) null else CameraOption(id, facing == CameraCharacteristics.LENS_FACING_FRONT, sizes)
        }
    }
    internal fun selected(context: Context, cameras: List<CameraOption>): CameraOption =
        CameraSelection.select(cameras, preferences(context).getBoolean("front", false))
    internal fun saveLens(context: Context, front: Boolean) { preferences(context).edit().putBoolean("front", front).apply() }
    internal fun changed() { CompanionConnection.updateStatus(CompanionConnection.status) }
    fun state(context: Context): CameraControlState {
        val cameras = try { options(context) }
        catch (error: CameraAccessException) { Log.w("PhoneCapture", "Camera availability check failed", error); emptyList() }
        catch (error: SecurityException) { Log.w("PhoneCapture", "Camera permission is unavailable", error); emptyList() }
        val selected = cameras.takeIf { it.isNotEmpty() }?.let { selected(context, it) }
        val active = CameraMicStreamer.active
        val opposite = cameras.firstOrNull { it.frontFacing != selected?.frontFacing }
        return CameraControlState(selected?.frontFacing ?: false, rotation(context),
            opposite != null && (active?.videoActive != true || active.supportsCamera(opposite)),
            active?.videoActive == true, active?.cameraBusy == true)
    }
    fun switchCamera(context: Context): CameraControlState {
        val cameras = options(context)
        val selected = selected(context, cameras)
        val next = cameras.firstOrNull { it.frontFacing != selected.frontFacing }
            ?: throw IllegalStateException("This phone has no alternate camera")
        val active = CameraMicStreamer.active
        check(active?.cameraBusy != true) { "Camera is changing. Try again when it is ready." }
        if (active?.videoActive == true) active.switchCamera(next)
        else { saveLens(context, next.frontFacing); changed() }
        return state(context)
    }
    fun rotate(context: Context): CameraControlState {
        val active = CameraMicStreamer.active
        check(active?.cameraBusy != true) { "Camera is changing. Try again when it is ready." }
        preferences(context).edit().putInt("rotation", CameraSelection.nextRotation(rotation(context))).apply()
        active?.requestKeyFrame()
        changed()
        return state(context)
    }
}
