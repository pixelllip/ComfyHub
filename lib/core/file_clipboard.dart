import 'dart:io' show Platform;

import 'package:flutter/services.dart';

/// 剪贴板里的一幅**位图**（截图工具 / 浏览器里复制图片）。
///
/// `format` 是原生给的原样格式：`png`（剪贴板本来就带编码好的 PNG，保真）
/// 或 `bmp`（原生用 `CF_DIB` / `CF_BITMAP` 包出来的 BMP，需要解码后转 PNG 再上传）。
class ClipboardImage {
  const ClipboardImage({required this.bytes, required this.format});

  final Uint8List bytes;
  final String format;

  bool get isPng => format == 'png';
}

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

  /// 通道**是不是真的接上了**。
  ///
  /// ⚠ 这一条是 2026-10-18 补出来的，起因是我自己埋的一个坑：这个类里所有失败都是
  /// **静默降级**的，于是"原生通道根本没注册 / 没编进产物"与"剪贴板里确实没有文件"
  /// 表现**完全一样** —— 用户报"Ctrl+V 粘不上附件"时，连该查哪一半都不知道。
  ///
  /// 判据只有一条：**方法调用成功过一次**就算接上了；报 `MissingPluginException`
  /// 就是没接上。别拿 `supported` 当判据 —— 那只是"平台对不对"。
  static bool? _channelAlive;

  /// 通道见过没有（`null` = 还没问过）。界面据此给一次提示，别再让它静默下去。
  static bool? get channelAlive => _channelAlive;

  /// 最近一次失败的原因（给日志用；没有就是 `null`）。
  static String? lastError;

  /// 统一入口：把"接上了没有"记下来，其余照旧降级。
  static Future<T?> _invoke<T>(String method, [Object? args]) async {
    try {
      final result = await _channel.invokeMethod<T>(method, args);
      _channelAlive = true;
      return result;
    } on MissingPluginException catch (e) {
      // 原生那一半没接上（老产物 / 没编进 exe）—— 这是**能力缺失**，不是"没有文件"
      _channelAlive = false;
      lastError = 'MissingPluginException($method): $e';
      return null;
    } catch (e) {
      _channelAlive ??= false;
      lastError = '$method: $e';
      return null;
    }
  }

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
    return await _invoke<bool>('hasFiles') ?? false;
  }

  /// 取出剪贴板里的文件路径。拿不到就回空列表（调用方要按"空了"处理）。
  static Future<List<String>> readFiles() async {
    if (!supported) return const [];
    final raw = await _invoke<List<Object?>>('readFiles');
    return (raw ?? const <Object?>[]).whereType<String>().toList(growable: false);
  }

  /// 剪贴板里**有没有位图**（截图 / 浏览器复制的图）。
  ///
  /// 与 [hasFiles] 一样只是一条提示：真正的判据永远是 [readImage] 拿没拿到东西。
  static Future<bool> hasImage() async {
    if (!supported) return false;
    return await _invoke<bool>('hasImage') ?? false;
  }

  /// 取出剪贴板里的位图。没有图 / 读失败都回 `null`（调用方按"没有"处理）。
  ///
  /// ⚠ 拿不到图与通道没接上**必须能区分**：前者要退回"粘文本"，后者要提示一次
  /// （[channelAlive]）。这也是所有失败都走 [_invoke] 的原因。
  static Future<ClipboardImage?> readImage() async {
    if (!supported) return null;
    final raw = await _invoke<Map<Object?, Object?>>('readImage');
    if (raw == null) return null;
    final bytes = raw['bytes'];
    if (bytes is! Uint8List || bytes.isEmpty) return null;
    final format = raw['format'];
    return ClipboardImage(
      bytes: bytes,
      format: format is String && format.isNotEmpty ? format : 'png',
    );
  }

  /// 把一批文件放进剪贴板（资源管理器里 Ctrl+V 能粘出文件）。失败回 `false`。
  ///
  /// 失败必须**如实**告诉用户（回 `false`），不能假装成功 —— 与 `_openFolder`
  /// 那条既有纪律一致（"不能假装打开了"）。
  static Future<bool> writeFiles(List<String> paths) async {
    if (!supported || paths.isEmpty) return false;
    return await _invoke<bool>('writeFiles', paths) ?? false;
  }
}
