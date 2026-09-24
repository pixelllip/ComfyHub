package com.comfyhub.harness

import org.junit.Assert.assertTrue
import org.junit.Test
import java.nio.file.Files
import java.nio.file.Path

/**
 * **真机才看得见的那条**：Android 的明文（http://）策略。
 *
 * 2026-09-24 真机验收实测：装到手机上以后，"模型清单拉得到、**上游一条也发不出去**"——
 * 手机 harness 的 `HttpURLConnection` 被系统拦下：
 * `IOException: Cleartext HTTP traffic to 127.0.0.1 not permitted`。
 * 而 Flutter 那一侧（`dart:io` 自己实现 socket、不走 Java 网络栈）**是通的**，
 * 所以表现成最难猜的那种半通状态：界面上一切正常，上行队列每一条都记一次失败，
 * **单测全绿**、桌面侧也一切正常。
 *
 * 这条用例把"manifest 必须挂上明文策略"钉死 —— 它扫的是**真文件**，
 * 所以哪天有人"顺手清理"掉那份配置，这里会红，而不是等到下一次真机验收。
 *
 * ⚠ 它只做**存在性**检查（不解析 XML）：真出问题时的表现是"整条出网被拦"，
 * 存在性已经足够抓住那种改动。
 */
class AndroidCleartextPolicyTest {

    /** 从测试工作目录往上找到 `android/`（与本模块其它基于路径的用例同一套做法）。 */
    private fun androidRoot(): Path? {
        var dir: Path? = Path.of("").toAbsolutePath()
        while (dir != null) {
            val app = dir.resolve("android/app")
            if (Files.isDirectory(app)) return dir.resolve("android")
            dir = dir.parent
        }
        return null
    }

    @Test
    fun `manifest 必须挂上明文策略，否则真机上一条消息都发不出去`() {
        val android = androidRoot()
        assertTrue("没找到 android/ 目录，用例本身失效了", android != null)

        val manifest = android!!.resolve("app/src/main/AndroidManifest.xml")
        assertTrue("manifest 不见了：$manifest", Files.isRegularFile(manifest))
        val manifestText = String(Files.readAllBytes(manifest), Charsets.UTF_8)

        assertTrue(
            "AndroidManifest 少了 android:networkSecurityConfig（真机上 http 上游会被系统拦下）",
            manifestText.contains("android:networkSecurityConfig=\"@xml/network_security_config\""),
        )
        assertTrue(
            "AndroidManifest 少了 INTERNET 权限（没有它连 socket 都建不起来）",
            manifestText.contains("android.permission.INTERNET"),
        )

        val config = android.resolve("app/src/main/res/xml/network_security_config.xml")
        assertTrue("明文策略文件不见了：$config", Files.isRegularFile(config))
        val configText = String(Files.readAllBytes(config), Charsets.UTF_8)
        assertTrue(
            "明文策略里必须允许 cleartext（手机 harness 直连的是用户自己的 http 网关）",
            configText.contains("cleartextTrafficPermitted=\"true\""),
        )
        // 回环显式声明：真机验收就是拿 127.0.0.1 + adb reverse 验的
        assertTrue("策略里要有 127.0.0.1", configText.contains("127.0.0.1"))
    }
}
