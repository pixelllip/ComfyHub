import 'dart:io' show Platform;

import 'package:flutter/services.dart';

/// 把缓存里的产物**单独保存到手机相册 / 媒体库**。
///
/// 为什么走原生：Android 10+ 要用 `MediaStore` 才能进系统相册（直接写
/// `/sdcard/Pictures` 在分区存储下不可见），而 Flutter 侧没有现成依赖；
/// 这里只开一个很窄的 MethodChannel，不引新的第三方包。
///
/// 与「导出产物」不同：那是桌面端的**整库导出**（写桌面磁盘）；这是手机上
/// 对**单条**产物的保存，不经过桌面、不复制工作流。
class MediaSaver {
  MediaSaver._();

  static const MethodChannel _channel = MethodChannel('comfyhub/media');

  /// 当前平台有没有"保存到相册"能力。
  static bool get supported {
    try {
      return Platform.isAndroid;
    } catch (_) {
      return false;
    }
  }

  /// 保存并返回系统给的 content URI（失败抛异常，调用方如实显示）。
  ///
  /// [path] 必须是本机已有文件（通常是 [MediaCache] 的缓存原件）；
  /// [mime] 决定进相册的图片 / 视频 / 音频集合。
  static Future<String> saveToGallery({
    required String path,
    required String mime,
    required String name,
  }) async {
    if (!supported) throw UnsupportedError('当前平台不支持保存到手机相册');
    final uri = await _channel.invokeMethod<String>('saveToGallery', {
      'path': path,
      'mime': mime,
      'name': name,
    });
    if (uri == null || uri.isEmpty) throw StateError('系统没有返回保存位置');
    return uri;
  }
}
