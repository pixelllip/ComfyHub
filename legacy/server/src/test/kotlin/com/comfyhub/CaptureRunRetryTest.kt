package com.comfyhub

import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/**
 * 「这次运行没有产物」必须是**可重试**的（2026-09-17 实测踩到）。
 *
 * 场景：ComfyUI 的 `/history` 先出现那条记录、产物文件却还没落盘
 * （或后台轮询抢在 `captureRun` 前面把记录标成了 `empty`）。
 * 如果把它当终态，几秒后文件到了也不会再收 —— 用户看到的就是
 * 「AI 说生成了，画廊里却没有」，而且完全不知道为什么。
 */
class CaptureRunRetryTest {

    @Test
    fun `empty 可以重试（产物文件晚到也能补上）`() {
        assertTrue(CaptureRepo.canReclaim("empty", ageSeconds = 5, force = false))
    }

    @Test
    fun `error 可以重试`() {
        assertTrue(CaptureRepo.canReclaim("error", ageSeconds = 5, force = false))
    }

    @Test
    fun `success 不再重复收（幂等靠它）`() {
        assertFalse(CaptureRepo.canReclaim("success", ageSeconds = 9999, force = false))
    }

    @Test
    fun `正在处理的运行不能被抢`() {
        assertFalse(CaptureRepo.canReclaim("running", ageSeconds = 3, force = false))
    }

    @Test
    fun `卡死超过十分钟的 running 可以重新抢（后端被杀过）`() {
        assertTrue(CaptureRepo.canReclaim("running", ageSeconds = 601, force = false))
    }

    @Test
    fun `目录导入的 force 仍然能重导入`() {
        assertTrue(CaptureRepo.canReclaim("success", ageSeconds = 1, force = true))
    }
}
