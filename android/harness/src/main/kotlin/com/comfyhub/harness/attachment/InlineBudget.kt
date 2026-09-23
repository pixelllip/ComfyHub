package com.comfyhub.harness.attachment

/**
 * 附件内联预算 —— 移植自桌面 `com.comfyhub.ai.InlineBudget` / `AttachmentPolicy`。
 *
 * 一次对话可能累积好几张历史图片，而请求体不能无限大：超过预算的那些**不发**，
 * 但调用方必须把它们变成一条**明确的说明**（"这张图没随本次请求发送"），
 * 而不是静默消失 —— 要防的就是"看起来发出去了、其实没有"。
 *
 * 这是"贪心 + 保序"：从最新到最旧由调用方决定顺序，装不下就跳过、继续试后面的小图。
 *
 * ⚠ 桌面把这两个常量通过 run-kit 的 `inlineBudget` 下发（`perImageBytes` / `totalBytes`），
 * 手机**优先用下发值**，这里的数字只是"下发拿不到时的保守回落"（离线首次启动）。
 */
object InlineBudget {
    /** 单张图片内联的原始字节上限（与桌面 `AttachmentPolicy.MAX_INLINE_BYTES` 同值）。 */
    const val MAX_INLINE_BYTES: Long = 8L * 1024 * 1024

    /** 单次请求内联附件的原始字节总预算（base64 后约 ×1.33）。 */
    const val MAX_TOTAL_BYTES: Long = 20L * 1024 * 1024

    /**
     * 判定每个附件到底发不发（`sizes` 与结果的顺序一一对应）。
     *
     * [maxTotal] / [maxSingle] 默认取本文件的常量；联网后应当传 run-kit 下发的值 ——
     * **下发的值优先**，否则两端会出现"桌面发得出去、手机发不出去"这种莫名差异。
     */
    fun plan(
        sizes: List<Long>,
        maxTotal: Long = MAX_TOTAL_BYTES,
        maxSingle: Long = MAX_INLINE_BYTES,
    ): List<Boolean> {
        var total = 0L
        return sizes.map { size ->
            val ok = size in 1..maxSingle && total + size <= maxTotal
            if (ok) total += size
            ok
        }
    }

    /** run-kit 下发的预算；拿不到时用常量（见类注释）。 */
    data class Limits(val perImageBytes: Long, val totalBytes: Long) {
        fun plan(sizes: List<Long>): List<Boolean> =
            plan(sizes, maxTotal = totalBytes, maxSingle = perImageBytes)

        companion object {
            val FALLBACK = Limits(MAX_INLINE_BYTES, MAX_TOTAL_BYTES)
        }
    }

    /**
     * 没发出去的附件要附在正文后面的一句话（**不吞内容**：让用户和模型都知道少了什么）。
     *
     * 桌面 `HarnessRunner.turnsFromHistory` 用的是同一条纪律（AGENTS §10）：
     * 既不能静默丢弃，也不能因为历史里有张图就把请求打成 400。
     */
    fun skippedNote(names: List<String>): String =
        if (names.isEmpty()) "" else "（以下附件未随本次请求发送：${names.joinToString("、")}）"
}
