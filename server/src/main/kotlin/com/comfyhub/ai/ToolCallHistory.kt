package com.comfyhub.ai

import com.comfyhub.ai.protocol.ChatTurn

/**
 * 拼历史时的一个**协议硬约束**：每个 `tool_call` 后面必须跟一条 `tool_call_id`
 * 对得上的工具结果。
 *
 * 用户 bug 2026-09-21 现场（查库实据，会话 `367f46a2`）：
 *  - seq 6 的助手消息 parts 是 `text / tool_call / tool_result`（配齐的）；
 *  - seq 8 的助手消息 parts 只有 `text / tool_call` —— 那次 Run 在提问中途被点了「停止」，
 *    `tool_result` 从来没落库；
 *  - 之后这条会话**怎么发都是 400**：
 *    `An assistant message with 'tool_calls' must be followed by tool messages
 *     responding to each 'tool_call_id'`，
 *    界面上显示成"（上游网关不接受工具参数，本次已退回纯文本模式。）" +
 *    `HTTP 400 配置有误` —— 用户看到的是"参数也不对"，其实是历史里有半截工具轮。
 *
 * 这里做的是**修历史**，不是改协议：给"整条历史里都没有结果"的调用补一条**如实说明**的
 * 占位工具结果。有真实结果的一个字都不动。
 */
object ToolCallHistory {

    /**
     * 给**没有结果的工具调用**补占位结果；返回一份新列表（不改入参）。
     *
     * 占位插在**这条助手轮的所有工具结果之前**：OpenAI 协议只要求 `tool_call_id` 能配上，
     * 对同一条 `tool_calls` 里多个结果的先后顺序不敏感；而"把占位放最后"在某些网关实现里
     * 会被判成"结果落在下一条消息之后"，所以统一放前面更稳。
     */
    fun repairOrphans(turns: List<ChatTurn>): List<ChatTurn> {
        val answered = turns.filter { it.role == "tool" }
            .mapNotNull { it.toolCallId?.takeIf { id -> id.isNotEmpty() } }
            .toSet()
        val out = mutableListOf<ChatTurn>()
        turns.forEach { turn ->
            val missing = turn.toolCalls.filter { it.id.isNotEmpty() && it.id !in answered }
            if (missing.isEmpty()) {
                out += turn
                return@forEach
            }
            out += turn
            missing.forEach { call ->
                out += ChatTurn(
                    "tool",
                    "（工具「${call.name.ifEmpty { "未知" }}」没有返回结果：这次调用在跑完前被中断" +
                        "（用户点了停止、连接断开，或 App 退出）。需要的话**重新调用一次**，" +
                        "不要把这次当成已完成。）",
                    toolCallId = call.id,
                )
            }
        }
        return out
    }
}
