package app.photosync

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import org.json.JSONObject
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Android Keystore-backed vault for the Syncthing API key and status token.
 * The key never leaves AndroidKeyStore and the ciphertext lives in
 * noBackupFilesDir, so it is not copied off the device by backup.
 */
object SecretsVault {
    private const val ALIAS = "app.photosync.secrets.v1"
    private val aad = ALIAS.toByteArray(Charsets.UTF_8)

    private fun file(context: Context) = File(context.noBackupFilesDir, "secrets.enc")

    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        val spec = KeyGenParameterSpec.Builder(
            ALIAS,
            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT
        )
            .setKeySize(256)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .build()
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
            .apply { init(spec) }
            .generateKey()
    }

    @Synchronized
    fun load(context: Context): JSONObject {
        val stored = file(context)
        if (!stored.isFile) return JSONObject()
        val bytes = stored.readBytes()
        require(bytes.size >= 30 && bytes[0] == 1.toByte()) { "vault is corrupt" }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, bytes.copyOfRange(1, 13)))
        cipher.updateAAD(aad)
        val plain = String(cipher.doFinal(bytes.copyOfRange(13, bytes.size)), Charsets.UTF_8)
        return JSONObject(plain)
    }

    @Synchronized
    fun put(context: Context, key: String, value: String) {
        val current = load(context)
        current.put(key, value)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key())
        cipher.updateAAD(aad)
        check(cipher.iv.size == 12)
        val bytes = byteArrayOf(1) + cipher.iv + cipher.doFinal(current.toString().toByteArray(Charsets.UTF_8))
        val tmp = File(context.noBackupFilesDir, "secrets.enc.tmp")
        try {
            tmp.outputStream().use { it.write(bytes); it.fd.sync() }
            check(tmp.renameTo(file(context)))
        } finally {
            tmp.delete()
        }
    }

    @Synchronized
    fun clear(context: Context) {
        val stored = file(context)
        if (stored.exists() && !stored.delete()) {
            KeyStore.getInstance("AndroidKeyStore").apply { load(null); deleteEntry(ALIAS) }
            error("Could not remove stored credentials")
        }
    }
}
