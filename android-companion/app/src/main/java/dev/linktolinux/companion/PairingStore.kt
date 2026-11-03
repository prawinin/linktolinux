package dev.linktolinux.companion

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import android.util.Log
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

class PairingStore(context: Context) {
    private val prefs = context.getSharedPreferences("linktolinux", Context.MODE_PRIVATE)
    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        return (store.getKey("desktop-pairing", null) as? SecretKey) ?: KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
            init(KeyGenParameterSpec.Builder("desktop-pairing", KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build())
            generateKey()
        }
    }
    fun load(): DesktopEndpoint? {
        val encoded = prefs.getString("paired_code", null) ?: return null
        return try {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            val iv = Base64.decode(prefs.getString("paired_iv", ""), Base64.NO_WRAP)
            cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, iv))
            DesktopEndpoint.parse(prefs.getString("paired_host", "").orEmpty(), prefs.getInt("paired_port", 0).toString(),
                String(cipher.doFinal(Base64.decode(encoded, Base64.NO_WRAP)), Charsets.UTF_8))
        } catch (error: Exception) {
            Log.w("PairingStore", "Saved pairing unavailable: ${error.javaClass.simpleName}")
            clear()
            null
        }
    }
    fun save(endpoint: DesktopEndpoint) {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val encrypted = cipher.doFinal(endpoint.code.toByteArray(Charsets.UTF_8))
        prefs.edit().putString("paired_host", endpoint.host).putInt("paired_port", endpoint.port)
            .putString("paired_code", Base64.encodeToString(encrypted, Base64.NO_WRAP))
            .putString("paired_iv", Base64.encodeToString(cipher.iv, Base64.NO_WRAP)).putBoolean("auto_connect", true).apply()
    }
    fun clear() { prefs.edit().remove("paired_code").remove("paired_iv").remove("paired_host").remove("paired_port").putBoolean("auto_connect", false).apply() }
}
