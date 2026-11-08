package com.comfyhub.harness.tools

/**
 * 桌面上那份 skill 的只读视图（手机本地没有时去拿）。
 *
 * 为什么需要它：skill 正文的真源在**桌面**磁盘。手机本地投放口出厂只有随包那几份
 * 提示词 skill（见 [SkillSeeder]），用户在桌面上写的、或者桌面随包带的另外几份，
 * 手机上本来一个字都取不到 —— 系统提示里列着名字，`load_skill` 却只能回"没有"。
 *
 * 三条纪律（与出网工具同一套）：
 * 1. **拿不到 ≠ 没有**：所以 [body] 用三态而不是 `String?` —— "桌面根本没有这个 skill"
 *    与"桌面连不上"对用户是两句完全不同的话；
 * 2. **只读**：这里只发 GET，不注册、不改、不删（那些是桌面独有的事）；
 * 3. **不缓存正文**：桌面随时可能改，而一次工具调用本来就只取一次。
 */
interface RemoteSkills {
    /**
     * 桌面上的 skill 清单（只要名字 / 描述 / 版本）。
     *
     * `null` = **没连上**（调用方要如实说"只列了本机的"，不许当成"桌面上一个都没有"）。
     */
    suspend fun names(): List<RemoteSkill>?

    /** 取一份正文（frontmatter 已剥掉，与桌面 `load_skill` 返回的同形）。 */
    suspend fun body(name: String): RemoteSkillBody
}

/** 桌面侧一条 skill 的摘要（够列表展示即可）。 */
data class RemoteSkill(
    val name: String,
    val description: String,
    val version: String? = null,
)

/** [RemoteSkills.body] 的三态结论。 */
sealed interface RemoteSkillBody {
    /** 拿到了正文。 */
    data class Found(val body: String) : RemoteSkillBody

    /** 桌面明确说没有这一份（404 / `CONFIG_ERROR`）。 */
    data object Missing : RemoteSkillBody

    /** 没连上（网络 / 超时 / 502…）：**不能**说成"没有"。 */
    data object Unreachable : RemoteSkillBody
}
