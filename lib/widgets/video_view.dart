import 'dart:io' show File;

import 'package:material_ui/material_ui.dart';

import 'android_video_view.dart';
import 'win_video_view.dart';

/// 内嵌视频播放的**平台分派**：Windows 一份实现、安卓/iOS 另一份。
///
/// 两份实现各有各的问题（Windows 那份依赖打过补丁的插件，见 AGENTS §9），
/// 所以这个类只做一件事：**在哪个平台就用哪个实现**，并且给调用方**一个**
/// 判据 `supported`（而不是让它自己 `Platform.isWindows || Platform.isAndroid`
/// 地拼 —— 拼漏一个平台的表现是"明明能播却显示不支持"）。
class PlatformVideoView extends StatelessWidget {
  const PlatformVideoView({
    super.key,
    required this.url,
    this.file,
    this.posterUrl,
    this.autoPlay = false,
    this.loop = false,
  });

  final String url;

  /// Android 优先播本机缓存；Windows 侧仍走 Media Foundation 的 URL。
  final File? file;
  final String? posterUrl;
  final bool autoPlay;
  final bool loop;

  /// 当前平台有没有内嵌播放能力（没有就显示"用系统播放器打开"那条退路）。
  static bool get supported =>
      WinVideoView.supported || AndroidVideoView.supported;

  @override
  Widget build(BuildContext context) {
    if (WinVideoView.supported) {
      return WinVideoView(
        url: url,
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
