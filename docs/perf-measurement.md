# 帧耗时测量：可能影响流畅度的组件改动

> 这是 `AGENTS.md` 第 3 节「开发与测试」里性能那一条的细则。全局红线见 [`../AGENTS.md`](../AGENTS.md)。

**2026-11-13 定下的硬约束**：改动落在下面任何一类，就必须在真跑起来的应用上量一次帧耗时，不许以"看起来不卡"收工 —— 新建或改写 widget、改列表与重建范围（懒构建 / `itemExtent` / `RepaintBoundary`）、改图片解码与缓存、改动画与滚动联动、给逐字回包或进度这类高频刷新加一次 `setState`。量化工具是 **Dart MCP Server**。

## 接线顺序

`dtd`（`listConnectedApps` / `connect` 找到正在跑的应用）→ `vm_service`（`command: callMethod`：`setVMTimelineFlags` 开 `Dart` / `Embedder`，`clearVMTimeline` → 操作一遍 → `getVMTimeline` 取帧事件）→ `widget_inspector`（`get_widget_tree` 看重建范围）→ 改完 `hot_reload`（改到全局常量 / `main` 才用 `hot_restart`）→ `get_runtime_errors` 确认没有新异常。

## 怎么量

- **一次只量一段真实操作**：滚动到底、流式回包、放大一张大图各量各的；改前改后量同一段，别拿首帧或空载当结论。
- **判据是长帧，不是平均帧率**：一帧超过刷新预算（60Hz 16.7ms、120Hz 8.3ms）就是卡顿（jank）；"平均 60 fps、每秒一个 40ms 长帧"照样算没做完。
- 量出来的数字写进当次结论给用户，**别把某台机器的帧耗时当规格抄进文档**。

## 解药

先按 [`ui-conventions.md`](ui-conventions.md) §3 用：懒构建 / `SliverFixedExtentList` / `RepaintBoundary` / `cacheWidth` / 重建闸门 / 流式通知攒到下一帧。

## 没有 Dart MCP Server 时

**不算跳过**：用 `flutter run -d windows --profile` 在 DevTools 的 Performance 页量同一段，结论里写明走的哪条路径。

## 回归防线

`test/scroll_perf_test.dart` 与 `test/ai_chat_perf_test.dart` 一条都不许删。
