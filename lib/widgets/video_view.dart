import 'dart:io' show File, Platform;

import 'package:material_ui/material_ui.dart';

import 'android_video_view.dart';
import 'media_kit_video_view.dart';

/// 内嵌视频播放的**平台分派**：Windows 用 better_player（libmpv），安卓/iOS 用官方 video_player。
///
/// 这个类只做一件事：**在哪个平台就用哪个实现**，并且给调用方**一个**
/// 判据 `supported`（而不是让它自己 `Platform.isWindows || Platform.isAndroid`
/// 地拼 —— 拼漏一个平台的表现是"明明能播却显示不支持"）。
class PlatformVideoView extends StatelessWidget {
  const PlatformVideoView({
    super.key,
    required this.url,
    this.localPath,
    this.file,
    this.posterUrl,
    this.autoPlay = false,
    this.loop = false,
  });

  final String url;

  /// 桌面本地后端的原件绝对路径；Windows 播放器优先用它。
  final String? localPath;

  /// Android 优先播本机缓存；Windows 侧走本地原件或网络 URL。
  final File? file;
  final String? posterUrl;
  final bool autoPlay;
  final bool loop;

  /// 当前平台有没有内嵌播放能力（没有就显示"用系统播放器打开"那条退路）。
  static bool get supported => Platform.isWindows || AndroidVideoView.supported;

  @override
  Widget build(BuildContext context) {
    if (Platform.isWindows) {
      return MediaKitVideoView(
        url: url,
        localPath: localPath,
        posterUrl: posterUrl,
        autoPlay: autoPlay,
        loop: loop,
      );
    }
    if (AndroidVideoView.supported) {
      return AndroidVideoView(
        url: url,
        file: file,
        posterUrl: posterUrl,
        autoPlay: autoPlay,
        loop: loop,
      );
    }
    return const SizedBox.shrink();
  }
}
