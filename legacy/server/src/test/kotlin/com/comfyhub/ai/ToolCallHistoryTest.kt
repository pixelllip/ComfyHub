package com.comfyhub.ai

import com.comfyhub.ai.protocol.ChatTurn
import com.comfyhub.ai.protocol.ToolCallRef
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * 用户 bug 2026-09-21：**一次被「停止」打断的提问，把整条会话打成 400**。
 *
 * 现场（查库实据，会话 `367f46a2`）：
 *  - seq 6 的助手消息 parts 是 `text / tool_call / tool_result`（配齐的）；
 *  - seq 8 的助手消息 parts 只有 `text / tool_call` —— 那次 Run 在提问中途被点了停止，
 *    `tool_result` 从来没落库；
 *  - 之后这条会话**怎么发都是 400**：
 *    `An assistant message with 'tool_calls' must be followed by tool messages
 *     responding to each 'tool_call_id'`，界面上显示成
 *    "（上游网关不接受工具参数，本次已退回纯文本模式。）" + `HTTP 400 配置有误`。
 *
 * [ToolCallHistory.repairOrphans] 就是解这一条的；这里钉四件事：
 * 缺的补上、有的不动、只补缺的那一个、空 id 不补。
 */
class ToolCallHistoryTest {

    private fun call(id: String, name: String = "ask_user_question") =
        ToolCallRef(id = id, name = name, arguments = "{}")

    @Test
    fun `缺结果的工具调用要补一条如实的占位结果`() {
        val turns = listOf(
            ChatTurn("user", "你问我一道选择题"),
            ChatTurn("assistant", "行，出一道：", toolCalls = listOf(call("call_a"))),
            // 后面接的就是下一条用户消息 —— 中间少了 call_a 的结果（被停止打断的那次）
            ChatTurn("user", "你问我一道选择题"),
        )
        val fixed = ToolCallHistory.repairOrphans(turns)

        assertEquals(4, fixed.size)
        assertEquals(listOf("user", "assistant", "tool", "user"), fixed.map { it.role })
        val tool = fixed[2]
        assertEquals("call_a", tool.toolCallId, "占位结果必须挂在对得上的 tool_call_id 上")
        assertTrue(tool.content.contains("没有返回结果"), tool.content)
        assertTrue(tool.content.contains("ask_user_question"), "要写清是哪个工具没返回：${tool.content}")
        assertTrue(tool.content.contains("重新调用一次"), "要告诉模型下一步怎么办：${tool.content}")
    }

    @Test
    fun `已经有结果的调用一个字都不动`() {
        val turns = listOf(
            ChatTurn("assistant", "查一下", toolCalls = listOf(call("call_1", "read_file"))),
            ChatTurn("tool", "# 标题\n正文", toolCallId = "call_1"),
            ChatTurn("assistant", "看完了。"),
        )
        assertEquals(turns, ToolCallHistory.repairOrphans(turns))
    }

    @Test
    fun `同一条助手轮里只缺一个时 只补缺的那一个`() {
        val turns = listOf(
            ChatTurn(
                "assistant",
                "两个都查",
                toolCalls = listOf(call("call_1", "read_file"), call("call_2", "list_dir")),
            ),
            ChatTurn("tool", "文件内容", toolCallId = "call_1"),
        )
        val fixed = ToolCallHistory.repairOrphans(turns)

        // 占位插在这条助手轮**所有工具结果之前**（顺序对上游不敏感，配齐才是硬要求）
        assertEquals(listOf("assistant", "tool", "tool"), fixed.map { it.role })
        assertEquals(
            setOf("call_1", "call_2"),
            fixed.filter { it.role == "tool" }.map { it.toolCallId }.toSet(),
        )
        assertEquals("文件内容", fixed[2].content, "真实结果不能被占位顶掉")
        assertTrue(fixed[1].content.contains("没有返回结果"))
    }

    @Test
    fun `空调用号不补（补了也配不上 反而多一条无主结果）`() {
        val turns = listOf(ChatTurn("assistant", "嗯", toolCalls = listOf(call(""))))
        assertEquals(turns, ToolCallHistory.repairOrphans(turns))
    }
}
