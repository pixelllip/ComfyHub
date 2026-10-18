#ifndef RUNNER_CLIPBOARD_CHANNEL_H_
#define RUNNER_CLIPBOARD_CHANNEL_H_

#include <flutter/binary_messenger.h>

// 注册 `comfyhub/clipboard` 方法通道。
//
// # 为什么这件事必须落到原生
//
// Flutter 内置的 `Clipboard.setData(ClipboardData(text: ...))` 在 Windows 上**只写
// `CF_UNICODETEXT`**（见引擎的 `platform_handler.cc`：`SetString()` 先
// `::EmptyClipboard()` 再 `::SetClipboardData(CF_UNICODETEXT, ...)`）。而
// **"复制文件本身"（能在资源管理器里 Ctrl+V 粘出文件）只有一个格式能满足：`CF_HDROP`**
// —— 内置 API 写不了，任何"先写 CF_HDROP 再调 Flutter setData"的写法还会被那次
// `EmptyClipboard()` 清掉。
//
// 同一个通道也用来**读**：`Ctrl+V` 想粘附件时，必须先知道剪贴板里有没有文件
// （读了才知道该不该拦截这次粘贴，否则会把普通文本粘贴弄坏）。
//
// 方法（全部同步返回）：
//   * `hasFiles`   → bool
//   * `readFiles`  → List<String>
//   * `writeFiles` → bool（参数是 `List<String>` 路径）
void RegisterClipboardChannel(flutter::BinaryMessenger* messenger);

#endif  // RUNNER_CLIPBOARD_CHANNEL_H_
