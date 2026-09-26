package com.comfyhub

import kotlinx.serialization.json.JsonObject

/**
 * **提交前的模型预检**（用户 2026-09-22 拍板：要的模型加载不了就**当场拒**，别白烧一轮显卡）。
 *
 * 为什么要有它：会话 `0bedb3f5` 里 AI 一共提交了 4 次，其中一次 78 秒跑完却 `mediaCount: 0`、
 * 一次 0.01 秒空跑（自定义节点不兼容）、还有一次带着 8 根"补上去的线"硬提交 ——
 * 这些代价本来都能在**提交之前**看出来。
 *
 * 判据只有一条，而且复用 [WorkflowFit]（**不另写一份**）：
 *  - [FitStatus.ON_DISK_ONLY] → **拒**：文件就在硬盘上、只是当前实例枚举不到，
 *    正确动作是重启 / 修挂载，不是换个名字或者重试；
 *  - [FitStatus.MISSING] → **拒**：当场说清"缺哪个、要装什么"，
 *    比让 ComfyUI 回一句 `file not found` 有用得多。
 *
 * ## 清单不可信时不拦（"宁可漏报也不误拦"）
 *
 * [ComfyModels.Source.NONE]（ComfyUI 没跑、也没扫到任何 models 目录）时**一个都不拦** ——
 * 那时的 missing 只是"我没看见"，不是"它不存在"。误拦的代价是明明能跑的工作流被 AI 拒掉，
 * 那比白跑一次更糟。
 *
 * ## 它只回答"能不能提交"
 *
 * 不替模型换模型、不写文件、不改图。被拒时给的是**动作**（重启 / 修挂载 / 装哪个 /
 * 或者把用不上的分支摘掉再提交），不是一个 `force` 开关 ——
 * 后者会让"AI 自作主张换掉用户的模型"换个名字重新出现。
 */
object ModelPreflight {

    /** 判定结果：null = 放行；非 null = 拒，返回的就是给模型的说明。 */
    fun check(graph: JsonObject, inventory: ComfyModels.Inventory): String? {
        if (inventory.source == ComfyModels.Source.NONE) return null
        if (graph.isEmpty()) return null

        val report = WorkflowFit.fit(graph, inventory)
        val blocked = report.items.filter {
            it.status == FitStatus.ON_DISK_ONLY || it.status == FitStatus.MISSING
        }
        if (blocked.isEmpty()) return null

        val diskOnly = blocked.filter { it.status == FitStatus.ON_DISK_ONLY }
        val missing = blocked.filter { it.status == FitStatus.MISSING }

        return buildString {
            append("**没有提交**：这份工作流要的 ").append(blocked.size)
            append(" 个模型当前这个 ComfyUI 实例加载不了 —— 硬提交只会白跑一次")
            append("（ComfyUI 只会回一句文件不存在）。\n")

            if (diskOnly.isNotEmpty()) {
                append("\n【硬盘上就有，只是这个实例看不到】").append(diskOnly.size).append(" 个：\n")
                diskOnly.forEach { item ->
                    append("  · ").append(item.requirement.overrideKey)
                        .append(" 要 ").append(item.requirement.fileName)
                    item.candidates.firstOrNull()?.model?.path?.let { append("（在 ").append(it).append("）") }
                    append('\n')
                }
                append("  → 让用户**重启 ComfyUI**（ComfyHub 起实例时会自动挂上 ComfyUI Desktop ")
                append("的模型根与 input/output），或用 `comfy_env_report` 看三个根差在哪。")
                append("**不要下载**（文件本来就在硬盘上），也不要改 overrides。\n")
            }

            if (missing.isNotEmpty()) {
                append("\n【本机确实没有】").append(missing.size).append(" 个：\n")
                missing.forEach { item ->
                    append("  · ").append(item.requirement.overrideKey)
                        .append(" 要 ").append(item.requirement.fileName)
                    if (item.nearMisses.isNotEmpty()) {
                        append("（沾边但不是同一个东西的：").append(item.nearMisses.joinToString("、")).append("）")
                    }
                    append('\n')
                }
                append("  → 先 `civitai_search` 查它（拿体积与下载链接），再用 `ask_user_question` ")
                append("让用户选：装它 / 换一条只用本机已有模型的工作流 / 放弃。\n")
                append("  → 如果这个加载器其实**不在执行链上**（被 bypass、或摘节点后悬着的旁支），")
                append("先用 `comfy_prune_workflow(analyze=true)` 看清楚，再把它摘掉 —— 这样这份图就能提交了。\n")
            }
        }
    }
}
