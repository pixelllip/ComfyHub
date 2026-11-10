import 'dart:io' show File, Platform;

import 'package:flutter/material.dart';
import 'package:video_player/video_player.dart';

/// 安卓 / iOS 上的视频播放器（官方 `video_player`）。
///
/// 为什么不复用 `WinVideoView`：那个走 Media Foundation，**只有 Windows 有实现**
/// （而且我们给它的插件打过补丁，见 AGENTS §9）。所以按平台分成两个实现，
/// 由 [PlatformVideoView] 分派 —— 两条路各自的解码器/纹理细节完全不同，
/// 合并进一个类只会让两边的问题互相牵连。
///
/// 三条纪律与 Windows 那侧一致：
///  · **失败要如实**：解不开就显示原因 + "用系统播放器打开"那条退路，不显示黑屏；
///  · **封面先上**：解码第一帧之前先铺 `posterUrl`，避免一大片黑；
///  · **`dispose` 必须放掉控制器**：安卓上漏掉它 = 播放器/音频焦点泄漏
///    （表现是"退出详情页之后声音还在"）。
class AndroidVideoView extends StatefulWidget {
  const AndroidVideoView({
    super.key,
    required this.url,
    this.file,
    this.posterUrl,
    this.autoPlay = false,
    this.loop = false,
  });

  final String url;

  /// 本机缓存原件；有它就不走网络（手机离线 / 远程访问都靠它）。
  final File? file;

  /// 可选封面（后端 `/api/media/{id}/poster` 抽的首帧）。
  final String? posterUrl;
  final bool autoPlay;
  final bool loop;

  /// 这个实现能不能在当前平台跑（调用方要先判它，别拿异常当判据）。
  static bool get supported {
    try {
      return Platform.isAndroid || Platform.isIOS;
    } catch (_) {
      return false;
    }
  }

  @override
  State<AndroidVideoView> createState() => _AndroidVideoViewState();
}

class _AndroidVideoViewState extends State<AndroidVideoView> {
  VideoPlayerController? _controller;
  String? _error;
  bool _ready = false;

  @override
  void initState() {
    super.initState();
    _open();
  }

  @override
  void didUpdateWidget(covariant AndroidVideoView oldWidget) {
    super.didUpdateWidget(oldWidget);
    // 缓存下载完成会把 file 从 null 变成有值：这时要从网络源切到本地源，
    // 不能继续停在已经打开的网络控制器上。
    if (oldWidget.url != widget.url ||
        oldWidget.file?.path != widget.file?.path) {
      _controller?.dispose();
      _controller = null;
      _ready = false;
      _error = null;
      _open();
    }
  }

  Future<void> _open() async {
    if (!AndroidVideoView.supported) {
      setState(() => _error = '当前平台不支持内嵌播放（安卓 / iOS 才有这个实现）');
      return;
    }
    final controller = widget.file != null
        ? VideoPlayerController.file(widget.file!)
        : VideoPlayerController.networkUrl(Uri.parse(widget.url));
    _controller = controller;
    try {
      await controller.initialize();
      if (!mounted) return;
      if (!controller.value.isInitialized) {
        setState(() => _error = '无法打开视频（可能缺少解码器或网络不通）');
        return;
      }
      await controller.setLooping(widget.loop);
      if (widget.autoPlay) await controller.play();
      if (!mounted) return;
      setState(() => _ready = true);
    } catch (e) {
      if (!mounted) return;
      // 原始异常原样带出来：Android 上"没有这个文件/编码不支持/明文被拦"是三种不同的处置
      setState(() => _error = '无法打开视频，$e');
    }
  }

  @override
  void dispose() {
    _controller?.dispose();
    super.dispose();
  }

  void _toggle() {
    final c = _controller;
    if (c == null || !_ready) return;
    setState(() {
      if (c.value.isPlaying) {
        c.pause();
      } else {
        c.play();
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    final error = _error;
    if (error != null) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(16),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              const Icon(Icons.movie_outlined, size: 42, color: Colors.white54),
              const SizedBox(height: 10),
              SelectableText(
                error,
                textAlign: TextAlign.center,
                style: const TextStyle(color: Colors.white70),
              ),
              const SizedBox(height: 6),
              const Text(
                '可以用右上角菜单里的「用系统播放器打开」',
                style: TextStyle(color: Colors.white38, fontSize: 12),
              ),
            ],
          ),
        ),
      );
    }

    final c = _controller;
    if (!_ready || c == null) {
      return _PosterOrSpinner(url: widget.posterUrl);
    }

    final duration = c.value.duration;
    final position = c.value.position;
    return GestureDetector(
      onTap: _toggle,
      behavior: HitTestBehavior.opaque,
      child: Stack(
        alignment: Alignment.center,
        children: [
          AspectRatio(
            aspectRatio: c.value.aspectRatio == 0
                ? 16 / 9
                : c.value.aspectRatio,
            child: VideoPlayer(c),
          ),
          // 暂停时提供可见的"继续播放"，否则会被误认为卡住
          if (!c.value.isPlaying)
            Container(
              decoration: const BoxDecoration(
                color: Colors.black38,
                shape: BoxShape.circle,
              ),
              padding: const EdgeInsets.all(14),
              child: const Icon(
                Icons.play_arrow,
                size: 40,
                color: Colors.white,
              ),
            ),
          Positioned(
            left: 0,
            right: 0,
            bottom: 0,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                VideoProgressIndicator(
                  c,
                  allowScrubbing: true,
                  padding: const EdgeInsets.symmetric(vertical: 6),
                ),
                Padding(
                  padding: const EdgeInsets.only(bottom: 6),
                  child: Text(
                    '${_fmt(position)} / ${_fmt(duration)}',
                    style: const TextStyle(color: Colors.white70, fontSize: 12),
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  /// `mm:ss`（超过一小时给 `h:mm:ss`）—— 与 Windows 那侧的控制条口径一致。
  static String _fmt(Duration d) {
    final h = d.inHours;
    final m = d.inMinutes.remainder(60).toString().padLeft(2, '0');
    final s = d.inSeconds.remainder(60).toString().padLeft(2, '0');
    return h > 0 ? '$h:$m:$s' : '$m:$s';
  }
}

/// 解码出第一帧之前铺封面 + 转圈（没有封面就只转圈，不假装已经能播）。
class _PosterOrSpinner extends StatelessWidget {
  final String? url;
  const _PosterOrSpinner({this.url});

  @override
  Widget build(BuildContext context) {
    if (url == null || url!.isEmpty) {
      return const Center(child: CircularProgressIndicator());
    }
    return Stack(
      fit: StackFit.expand,
      children: [
        Image.network(
          url!,
          fit: BoxFit.contain,
          errorBuilder: (_, _, _) => const SizedBox.shrink(),
        ),
        const Center(child: CircularProgressIndicator()),
      ],
    );
  }
}
