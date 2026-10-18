import 'dart:io' show Platform;

import 'package:flutter/services.dart';

/// 剪贴板里的**文件**（Windows 的 `CF_HDROP`）——读与写都只有这一份实现。
///
/// # 为什么不用 Flutter 自带的 `Clipboard`
///
/// 它在 Windows 上**只写 `CF_UNICODETEXT`**，而"能在资源管理器里 Ctrl+V 粘出文件"
/// 只有一个格式能满足 —— `CF_HDROP`。内置 API 写不了。
///
/// ⚠⚠ **两者不能混用**：引擎写文本之前会先 `::EmptyClipboard()`，会把刚写好的
/// 文件列表一起清掉 —— 而且**一个错都不报**。所以"复制文件"之后**不要**再调
/// `Clipboard.setData`（原生那一半已经额外写了一份纯文本兜底）。
///
/// 原生实现：`windows/runner/clipboard_channel.cpp`（通道名 `comfyhub/clipboard`）。
class FileClipboard {
  FileClipboard._();

  static const MethodChannel _channel = MethodChannel('comfyhub/clipboard');

  /// 只有 Windows 有这份实现。
  ///
  /// 手机上"复制文件本身"是另一件事（产物是远端 HTTP、本地没有那个文件，
  /// 而且没有 FileProvider），见 `docs/open-issues-2026-10-18.md` §5.3。
  static bool get supported => Platform.isWindows;

  /// 剪贴板里**有没有文件**。
  ///
  /// 这是 `Ctrl+V` 拦截的**唯一判据**：没有文件就放行，让输入框自己粘文本 ——
  /// 无条件拦截会把普通文本粘贴弄坏。
  static Future<bool> hasFiles() async {
    if (!supported) return false;
    try {
      return await _channel.invokeMethod<bool>('hasFiles') ?? false;
    } catch (_) {
      // 通道不可用（老产物 / 非 Windows）时当"没有文件"，绝不因此中断粘贴
      return false;
    }
  }

  /// 取出剪贴板里的文件路径。拿不到就回空列表（调用方要按"空了"处理）。
  static Future<List<String>> readFiles() async {
    if (!supported) return const [];
    try {
      final raw = await _channel.invokeMethod<List<Object?>>('readFiles');
      return (raw ?? const <Object?>[]).whereType<String>().toList(growable: false);
    } catch (_) {
      return const [];
    }
  }

  /// 把一批文件放进剪贴板（资源管理器里 Ctrl+V 能粘出文件）。失败回 `false`。
  ///
  /// 失败必须**如实**告诉用户（回 `false`），不能假装成功 —— 与 `_openFolder`
  /// 那条既有纪律一致（"不能假装打开了"）。
  static Future<bool> writeFiles(List<String> paths) async {
    if (!supported || paths.isEmpty) return false;
    try {
      return await _channel.invokeMethod<bool>('writeFiles', paths) ?? false;
    } catch (_) {
      return false;
    }
  }
}
