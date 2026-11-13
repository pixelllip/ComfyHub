import 'dart:io';

import 'package:material_ui/material_ui.dart';
import 'package:flutter/services.dart';
import 'package:video_player_win/video_player_win.dart';

/// Windows 上的视频播放器（基于 Media Foundation）。
/// 其他平台不支持，调用方需要先用 [WinVideoView.supported] 判断。
class WinVideoView extends StatefulWidget {
  final String url;
  final bool autoPlay;
  final bool loop;

  /// 本机原件的绝对路径（桌面本地后端才有）。
  ///
  /// 有它就直接让 Media Foundation 读本地文件：走 HTTP 时 MF 用自己的网络源，
  /// 4K 大文件既容易掉帧，拖进度条还要重新发 Range 请求（2026-11-13 实测
  /// 4K HEVC 只有十几帧、拖动很卡）。拿不到路径时回落到 [url]。
  final String? localPath;

  /// 可选封面（预览图）：解码出第一帧之前先显示它，避免一大片黑。
  final String? posterUrl;

  const WinVideoView({
    super.key,
    required this.url,
    this.localPath,
    this.autoPlay = false,
    this.loop = false,
    this.posterUrl,
  });

  static bool get supported {
    try {
      return Platform.isWindows;
    } catch (_) {
      return false;
    }
  }

  @override
  State<WinVideoView> createState() => _WinVideoViewState();
}

class _WinVideoViewState extends State<WinVideoView> {
  WinVideoPlayerController? _controller;
  String? _error;
  bool _ready = false;

  /// 全屏期间内嵌这一路**不再渲染 Texture**：同一个 controller 同时接两个
  /// `WinVideoPlayer` 会抢纹理（其中一个黑屏）。画面仍旧是同一个解码器，
  /// 所以进出全屏不重建、不黑屏。
  bool _fullscreen = false;

  @override
  void initState() {
    super.initState();
    _open();
  }

  Future<void> _open() async {
    if (!WinVideoView.supported) {
      setState(() => _error = '当前平台不支持内嵌播放（仅 Windows）');
      return;
    }
    final path = widget.localPath;
    final controller = (path != null && path.isNotEmpty)
        ? WinVideoPlayerController.file(File(path))
        : WinVideoPlayerController.networkUrl(Uri.parse(widget.url));
    _controller = controller;
    try {
      await controller.initialize();
      if (!mounted) return;
      if (!controller.value.isInitialized) {
        setState(() => _error = '无法打开视频（可能缺少解码器）');
        return;
      }
      await controller.setLooping(widget.loop);
      setState(() => _ready = true);
      if (widget.autoPlay) await controller.play();
    } catch (e) {
      if (mounted) setState(() => _error = '播放器初始化失败: $e');
    }
  }

  @override
  void dispose() {
    _controller?.dispose();
    super.dispose();
  }

  /// 全屏播放：**复用同一个 controller**，只把画面换到全屏路由里渲染。
  ///
  /// 以前新开一路播放器（重新 initialize + seek），进全屏会先黑一下等解码；
  /// 而同一个 controller 同时接两个 `WinVideoPlayer` 又会抢纹理。所以这里在
  /// 全屏期间让内嵌那一路只画黑底（不建 Texture），全屏页直接接管同一路画面。
  Future<void> _openFullscreen() async {
    final controller = _controller;
    final v = controller?.value;
    if (controller == null || v == null || !v.isInitialized || _fullscreen) {
      return;
    }

    setState(() => _fullscreen = true);
    try {
      await Navigator.of(context, rootNavigator: true).push<void>(
        // 进出全屏都**立即切换**，不做路由过渡：同一块画面只是换了尺寸，
        // 淡入/位移只会让人以为视频卡了一下（2026-11-13 用户要求）。
        PageRouteBuilder<void>(
          fullscreenDialog: true,
          transitionDuration: Duration.zero,
          reverseTransitionDuration: Duration.zero,
          pageBuilder: (_, _, _) => _FullscreenVideo(controller: controller),
        ),
      );
    } finally {
      if (mounted) setState(() => _fullscreen = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    if (_error != null) {
      return _FallbackMessage(message: _error!, url: widget.url);
    }
    final controller = _controller;
    if (controller == null || !_ready) {
      return _PosterOrSpinner(url: widget.posterUrl);
    }

    // 全屏路由正拿着同一个 controller 渲染：这里不能再建 Texture（会抢纹理）。
    if (_fullscreen) {
      return const ColoredBox(color: Colors.black);
    }

    final ratio = _ratioOf(controller);

    return Column(
      children: [
        Expanded(
          child: ColoredBox(
            color: Colors.black,
            child: Stack(
              children: [
                // 视频按**长边铺满**：可用区域偏宽就拉满宽度，偏窄就拉满高度，
                // 保持原始比例、不裁切。以前的固定 16:9 会让竖屏视频两侧留一大块黑。
                Positioned.fill(
                  child: LayoutBuilder(
                    builder: (context, box) {
                      var w = box.maxWidth;
                      var h = w / ratio;
                      if (h > box.maxHeight) {
                        h = box.maxHeight;
                        w = h * ratio;
                      }
                      return Center(
                        child: SizedBox(
                          width: w,
                          height: h,
                          child: WinVideoPlayer(controller),
                        ),
                      );
                    },
                  ),
                ),
              ],
            ),
          ),
        ),
        // 全屏入口只留控制条上这一个。以前右上角还有一个悬浮按钮，
        // 同一页面出现两个"全屏"图标会被理解为两个功能。
        _Controls(
          controller: controller,
          onFullscreen: _openFullscreen,
          dark: false,
        ),
      ],
    );
  }

  static double _ratioOf(WinVideoPlayerController c) {
    final r = c.value.aspectRatio;
    if (r.isFinite && r > 0) return r;
    final size = c.value.size;
    if (size.width > 0 && size.height > 0) return size.width / size.height;
    return 16 / 9;
  }
}

/// 解码出第一帧之前显示封面；没有封面就转圈。
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

/// 全屏播放页：直接渲染内嵌那一路的 controller，不重新初始化。
class _FullscreenVideo extends StatefulWidget {
  final WinVideoPlayerController controller;

  const _FullscreenVideo({required this.controller});

  @override
  State<_FullscreenVideo> createState() => _FullscreenVideoState();
}

class _FullscreenVideoState extends State<_FullscreenVideo> {
  /// 键盘焦点锚点：没有它，按 Esc 时全屏页里可能没有任何东西持有焦点，
  /// `Shortcuts` 收不到按键（这就是"无法按 Esc 退出"的成因）。
  final _focus = FocusNode(debugLabel: 'fullscreen-video');

  @override
  void initState() {
    super.initState();
    // 自动接管焦点，Esc 才有效（点击控件后焦点会移过去，退出按钮依然在）。
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _focus.requestFocus();
    });
  }

  void _exit() => Navigator.of(context).pop();

  @override
  void dispose() {
    _focus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final controller = widget.controller;
    final ratio = _WinVideoViewState._ratioOf(controller);
    return Scaffold(
      backgroundColor: Colors.black,
      body: Shortcuts(
        shortcuts: const {
          SingleActivator(LogicalKeyboardKey.escape): DismissIntent(),
        },
        child: Actions(
          actions: {
            DismissIntent: CallbackAction<DismissIntent>(
              onInvoke: (_) {
                _exit();
                return null;
              },
            ),
          },
          child: Focus(
            focusNode: _focus,
            child: Stack(
              children: [
                Positioned.fill(
                  child: LayoutBuilder(
                    builder: (context, box) {
                      var w = box.maxWidth;
                      var h = w / ratio;
                      if (h > box.maxHeight) {
                        h = box.maxHeight;
                        w = h * ratio;
                      }
                      return Center(
                        child: SizedBox(
                          width: w,
                          height: h,
                          child: WinVideoPlayer(controller),
                        ),
                      );
                    },
                  ),
                ),
                // 退出全屏和控制条的「全屏」在**同一个位置**（右下角）。
                Positioned(
                  left: 0,
                  right: 0,
                  bottom: 0,
                  child: ColoredBox(
                    color: Colors.black54,
                    child: _Controls(
                      controller: controller,
                      onFullscreen: _exit,
                      dark: true,
                      fullscreenIcon: Icons.fullscreen_exit,
                      fullscreenTooltip: '退出全屏（Esc）',
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _Controls extends StatefulWidget {
  final WinVideoPlayerController controller;

  /// 控制条上的全屏 / 退出全屏按钮；传 null 表示不显示。
  final VoidCallback? onFullscreen;

  /// 全屏页用它把图标换成「退出全屏」，保证进出全屏的按钮落在同一个位置。
  final IconData fullscreenIcon;
  final String fullscreenTooltip;

  /// 黑底场景（全屏页）：文字与图标用白色。
  final bool dark;

  const _Controls({
    required this.controller,
    this.onFullscreen,
    this.fullscreenIcon = Icons.fullscreen,
    this.fullscreenTooltip = '全屏（Esc 退出）',
    this.dark = false,
  });

  @override
  State<_Controls> createState() => _ControlsState();
}

class _ControlsState extends State<_Controls> {
  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_onTick);
  }

  void _onTick() {
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    widget.controller.removeListener(_onTick);
    super.dispose();
  }

  String _fmt(Duration d) {
    final m = d.inMinutes.remainder(60).toString().padLeft(2, '0');
    final s = d.inSeconds.remainder(60).toString().padLeft(2, '0');
    return d.inHours > 0 ? '${d.inHours}:$m:$s' : '$m:$s';
  }

  @override
  Widget build(BuildContext context) {
    final v = widget.controller.value;
    final total = v.duration.inMilliseconds;
    final pos = v.position.inMilliseconds.clamp(0, total == 0 ? 1 : total);
    // 全屏页是黑底，字与图标必须跟着变白，否则时间显示看不见
    final fg = widget.dark ? Colors.white : null;
    final timeStyle = TextStyle(fontSize: 12, color: fg);

    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      child: Row(
        children: [
          IconButton(
            icon: Icon(
              v.isPlaying ? Icons.pause_circle_filled : Icons.play_circle_fill,
              size: 34,
              color: fg,
            ),
            onPressed: () async {
              if (v.isPlaying) {
                await widget.controller.pause();
              } else {
                await widget.controller.play();
              }
              setState(() {});
            },
          ),
          Text(_fmt(v.position), style: timeStyle),
          Expanded(
            child: Slider(
              value: pos.toDouble(),
              max: (total == 0 ? 1 : total).toDouble(),
              onChanged: total == 0
                  ? null
                  : (value) => widget.controller.seekTo(
                      Duration(milliseconds: value.toInt()),
                    ),
            ),
          ),
          Text(_fmt(v.duration), style: timeStyle),
          const SizedBox(width: 8),
          Icon(
            v.volume == 0 ? Icons.volume_off : Icons.volume_up,
            size: 18,
            color: fg ?? Theme.of(context).colorScheme.outline,
          ),
          SizedBox(
            width: 90,
            child: Slider(
              value: v.volume.clamp(0.0, 1.0),
              onChanged: (value) => widget.controller.setVolume(value),
            ),
          ),
          if (widget.onFullscreen != null) ...[
            const SizedBox(width: 4),
            IconButton(
              tooltip: widget.fullscreenTooltip,
              icon: Icon(widget.fullscreenIcon, size: 22, color: fg),
              onPressed: widget.onFullscreen,
            ),
          ],
        ],
      ),
    );
  }
}

class _FallbackMessage extends StatelessWidget {
  final String message;
  final String url;

  const _FallbackMessage({required this.message, required this.url});

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Icon(Icons.videocam_off_outlined, size: 48),
            const SizedBox(height: 12),
            Text(message, textAlign: TextAlign.center),
          ],
        ),
      ),
    );
  }
}
