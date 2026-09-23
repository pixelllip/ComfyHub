package com.comfyhub.harness.bridge

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * 凭据存储（**Android Keystore**）—— 桌面那份 `CredentialService`（DPAPI + powershell）的对应物。
 *
 * ⚠ 桌面的实现**不可复用**：`CredentialService.kt:190` 是 `if (!isWindows()) error(...)`，
 * 而且它走 `powershell.exe` 调 .NET。手机上必须另写一份，这正是 M6 的活。
 *
 * 设计上**与桌面同构**（`docs/mobile-harness-design.md` §8）：
 *  - `ai_providers.credential_ref` 存的只是**名字**，值由平台自己的密钥库保管；
 *  - 于是 `ai_providers` 可以整体下行同步（里面没有密钥），
 *    但**用户要在手机上单独填一次 Key** —— 这是 B 方案的必然代价，界面要说清楚。
 *
 * 实现要点：
 *  - 密钥由 **AndroidKeyStore** 生成并保管（`AES/GCM/NoPadding`，**密钥不出硬件/系统密钥库**）；
 *  - 密文 + IV 一起 base64 存在 SharedPreferences（`iv:ciphertext`）；
 *  - 删到 `remove` 时密文删掉即可（Keystore 里的那把钥匙留着，换个 ref 复用）。
 */
class KeystoreCredentials(context: Context) {

    private val prefs = context.applicationContext
        .getSharedPreferences("comfyhub_credentials", Context.MODE_PRIVATE)

    private fun secretKey(): SecretKey {
        val ks = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }
        (ks.getEntry(KEY_ALIAS, null) as? KeyStore.SecretKeyEntry)?.let { return it.secretKey }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                // 刻意**不要求**用户认证：手机 harness 要在后台（前台服务里）跑，
                // 每次调用都弹指纹是走不通的
                .build()
        )
        return generator.generateKey()
    }

    fun put(ref: String, secret: String) {
        val cipher = Cipher.getInstance(TRANSFORMATION).apply { init(Cipher.ENCRYPT_MODE, secretKey()) }
        val cipherText = cipher.doFinal(secret.toByteArray(Charsets.UTF_8))
        val packed = Base64.encodeToString(cipher.iv, Base64.NO_WRAP) + ":" +
            Base64.encodeToString(cipherText, Base64.NO_WRAP)
        prefs.edit().putString(ref, packed).apply()
    }

    /** 取回明文；没有配置 / 解密失败都返回 null（**不许抛**：调用方按"没密钥"处理）。 */
    fun get(ref: String): String? {
        val packed = prefs.getString(ref, null) ?: return null
        val parts = packed.split(":")
        if (parts.size != 2) return null
        return runCatching {
            val iv = Base64.decode(parts[0], Base64.NO_WRAP)
            val data = Base64.decode(parts[1], Base64.NO_WRAP)
            val cipher = Cipher.getInstance(TRANSFORMATION).apply {
                init(Cipher.DECRYPT_MODE, secretKey(), GCMParameterSpec(128, iv))
            }
            String(cipher.doFinal(data), Charsets.UTF_8)
        }.getOrNull()
    }

    fun has(ref: String): Boolean = prefs.contains(ref)

    fun remove(ref: String) {
        prefs.edit().remove(ref).apply()
    }

    /** 已配置的 ref 名字（界面显示"手机上填过哪些 Key"用；**不含任何值**）。 */
    fun refs(): List<String> = prefs.all.keys.sorted()

    private companion object {
        const val ANDROID_KEYSTORE = "AndroidKeyStore"
        const val KEY_ALIAS = "comfyhub.harness.credentials"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
    }
}
