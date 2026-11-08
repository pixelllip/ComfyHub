package com.comfyhub.harness.tools

import java.io.File
import java.io.InputStream

/**
 * 随包内置 skill 的来源（APK 的 `assets/` 里；单测用目录实现）。
 *
 * 刻意只留这两个动作：**列一层**、**打开一个文件**。手机那边没有"读字节数 / 改时间"
 * 这类需要（assets 是只读的，种一次就够），接口越小越不容易在两端实现出分歧。
 */
interface SkillAssetSource {
    /** `assetPath` 下一层的名字（文件与目录都在里面）。不存在就是空表。 */
    fun list(assetPath: String): List<String>

    /** 打开一个**文件**；路径是目录、或者不存在时返回 null。 */
    fun open(assetPath: String): InputStream?
}

/**
 * 把**随包内置的提示词 skill** 种进手机的本地投放口。
 *
 * 为什么要有（2026-11-08 用户："手机需要一点必要的提示词生成 skill 吧？"）：
 * skill 正文的真源是**桌面**磁盘（`<storage>\ai\skills` 与 `<项目根>\skills\builtin`），
 * 而手机 `load_skill` 读的是本机 `<filesDir>/ai/skills` —— 那本来是给"用户自己拷进来"用的投放口，
 * 出厂是空的。于是手机上：系统提示（桌面渲染的 run-kit）**列着 skill 名**，
 * 模型真去 `load_skill` 却一个都取不到，`/技能` 也就只剩一个名字。
 *
 * # 四条纪律
 *
 * 1. **只种一次**：`<target>/.seeded` 记版本号，种过就不再动 —— 用户删掉某份 skill
 *    之后重启 App，它**不会**自己长回来（那是用户的选择，不是缺件）。
 * 2. **绝不覆盖**：目标文件已存在就跳过（用户可能改过正文）。种失败（`failed > 0`）
 *    **不写标记**，下次启动再试 —— 静默留个空投放口只会让人以为"本来就该是空的"。
 * 3. **只种提示词那几份**（[SEEDED_SKILLS]）：出图 / 工作流类的 skill 要 ComfyUI 在场上，
 *    手机本地既不能跑也不该假装能跑。
 * 4. **种不进去不拦启动**：App 少几份 skill 也能用（`list_skills` 会如实说本机没有）。
 */
object SkillSeeder {

    /** 种子的版本：改了清单或格式就 +1，下次启动会重新走一遍（仍然不覆盖已存在的文件）。 */
    const val SEED_VERSION = 1

    /** 种进手机的那几份（与 `skills/builtin/` 里的目录同名；改名前先确认资源里有它）。 */
    val SEEDED_SKILLS: List<String> = listOf(
        // 把参考图 + 角色卡 + 画风卡扩写成一段可直接出图的提示词（手机上的主力场景）
        "r2i-prompt",
        // 写 / 改角色卡与画风卡
        "card-writer",
        // 两个模型的提示词规则：换模型写提示词时用得上，正文自带，不依赖 references/
        "krea-2",
        "moss-image",
    )

    private const val MARKER = ".seeded"
    private const val ROOT = "builtin"

    /** 一次种的结果（给人看的事实，不参与判断）。 */
    data class SeedResult(val created: Int, val skipped: Int, val failed: Int) {
        val ran: Boolean get() = created > 0 || skipped > 0 || failed > 0
    }

    /**
     * 把 [names] 这些 skill 从 [source] 的 `builtin/<name>` 种到 [targetDir]。
     *
     * 已经种过（标记是当前 [SEED_VERSION]）就直接返回全 0 —— 调用方不用自己判。
     */
    fun seed(
        source: SkillAssetSource,
        targetDir: File,
        names: List<String> = SEEDED_SKILLS,
    ): SeedResult {
        val marker = File(targetDir, MARKER)
        if (marker.isFile && marker.readText(Charsets.UTF_8).trim() == SEED_VERSION.toString()) {
            return SeedResult(0, 0, 0)
        }
        var created = 0
        var skipped = 0
        var failed = 0
        targetDir.mkdirs()
        for (name in names) {
            // 平铺写法（`builtin/<name>.md`）也认：与仓库 `skills/builtin/` 的两种布局一致
            val flat = source.open("$ROOT/$name.md")
            if (flat != null) {
                val dest = File(targetDir, "$name.md")
                if (dest.isFile) {
                    skipped++
                } else {
                    flat.use { input -> dest.outputStream().use { input.copyTo(it) } }
                    created++
                }
                continue
            }
            val bundle = copyDir(source, "$ROOT/$name", File(targetDir, name))
            if (bundle == null) {
                failed++ // 资源里根本没有这份（多半是改名了没同步清单）
            } else {
                created += bundle.first
                skipped += bundle.second
            }
        }
        // 有失败就**不写标记**：下次启动再试一次，免得手机上永远只有半个投放口
        if (failed == 0) {
            marker.writeText("$SEED_VERSION\n", Charsets.UTF_8)
        }
        return SeedResult(created, skipped, failed)
    }

    /** 递归拷一层目录；资产里没有这个目录（`list` 为空）就返回 null。 */
    private fun copyDir(
        source: SkillAssetSource,
        assetPath: String,
        destDir: File,
    ): Pair<Int, Int>? {
        val children = source.list(assetPath)
        if (children.isEmpty()) return null
        var created = 0
        var skipped = 0
        for (child in children) {
            val childAsset = "$assetPath/$child"
            val dest = File(destDir, child)
            val input = source.open(childAsset)
            if (input != null) {
                if (dest.isFile) {
                    skipped++ // 用户可能改过这份：一个字节都不许覆盖
                    continue
                }
                dest.parentFile?.mkdirs()
                input.use { ins -> dest.outputStream().use { ins.copyTo(it) } }
                created++
            } else {
                val sub = copyDir(source, childAsset, dest) ?: continue
                created += sub.first
                skipped += sub.second
            }
        }
        return created to skipped
    }
}
