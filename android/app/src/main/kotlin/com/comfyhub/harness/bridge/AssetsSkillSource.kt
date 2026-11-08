package com.comfyhub.harness.bridge

import android.content.Context
import com.comfyhub.harness.tools.SkillAssetSource
import java.io.InputStream

/**
 * 从 APK 的 `assets/` 里取内置 skill（`:harness` 是纯 JVM 模块，碰不到 Android API，
 * 所以这一层胶水留在 app 模块）。
 *
 * 资产来自仓库根的 `skills/` 目录（`app/build.gradle.kts` 里把它加进了 assets 源），
 * so APK 里的路径是 `builtin/<name>/SKILL.md`，与磁盘上的 `skills/builtin/<name>/SKILL.md`
 * 一一对应 —— 改 skill 只改仓库根那一份，不需要往 git 里再拷贝一遍。
 */
class AssetsSkillSource(private val context: Context) : SkillAssetSource {

    override fun list(assetPath: String): List<String> =
        runCatching { context.assets.list(assetPath)?.toList() }
            .getOrNull()
            .orEmpty()

    /**
     * 打开一个资产文件。
     *
     * 路径是**目录**时 `AssetManager.open` 会抛 `FileNotFoundException`（Is a directory），
     * 这里统一收成 null —— 调用方据此判断"这是目录，继续往下走"。
     */
    override fun open(assetPath: String): InputStream? =
        runCatching { context.assets.open(assetPath) }.getOrNull()
}
