import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:video_player_win/video_player_win.dart';

/// Windows 上的视频播放器（基于 Media Foundation）。
/// 其他平台不支持，调用方需要先用 [WinVideoView.supported] 判断。
class WinVideoView extends StatefulWidget {
  final String url;
  final bool autoPlay;
  final bool loop;

  /// 可选封面（预览图）：解码出第一帧之前先显示它，避免一大片黑。
  final String? posterUrl;

  const WinVideoView({
    super.key,
    required this.url,
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
    final controller = WinVideoPlayerController.networkUrl(Uri.parse(widget.url));
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

  /// 全屏播放：新开一个全屏路由再放一个播放器。
  ///
  /// 不复用同一个 controller —— `video_player_win` 的同一个 controller 接到两个
  /// `WinVideoPlayer` 上会互相抢纹理，实测会出现其中一个黑屏。新开一路更稳，
  /// 代价只是多解码一次（本地文件，可接受）。
  ///
  /// 进去之前先**暂停**内嵌这一路，退出时按全屏页回传的位置续播。
  /// 以前只传 `startAt` 却不暂停/不回写，两路同时输出同一份音频，
  /// 回来还停在旧进度 —— 看上去就是"点全屏后从头播"（bug 清单第 1 条）。
  Future<void> _openFullscreen() async {
    final controller = _controller;
    final v = controller?.value;
    if (controller == null || v == null || !v.isInitialized) return;

    final startAt = v.position;
    final wasPlaying = v.isPlaying;
    if (wasPlaying) await controller.pause();
    if (!mounted) return;

    final exitAt = await Navigator.of(context).push<Duration>(
      MaterialPageRoute<Duration>(
        fullscreenDialog: true,
        builder: (_) => _FullscreenVideo(
          url: widget.url,
          posterUrl: widget.posterUrl,
          startAt: startAt,
        ),
      ),
    );

    // 全屏页里可能拖到了别处：回到内嵌这一路时接着最新进度，而不是倒回去
    if (!mounted) return;
    if (exitAt != null && exitAt > startAt) {
      await controller.seekTo(exitAt);
    }
    if (wasPlaying && mounted) await controller.play();
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
        // 同一个页面出现两个"全屏"图标，用户会以为是两个功能（bug 清单第 1 条）。
        _Controls(controller: controller, onFullscreen: _openFullscreen, dark: false),
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

/// 全屏播放页：黑底 + 沉浸式铺满，带一套同样的控制条。
class _FullscreenVideo extends StatefulWidget {
  final String url;
  final String? posterUrl;
  final Duration startAt;

  const _FullscreenVideo({required this.url, this.posterUrl, required this.startAt});

  @override
  State<_FullscreenVideo> createState() => _FullscreenVideoState();
}

class _FullscreenVideoState extends State<_FullscreenVideo> {
  WinVideoPlayerController? _controller;
  String? _error;
  bool _ready = false;

  /// 键盘焦点锚点：没有它，按 Esc 时全屏页里可能没有任何东西持有焦点，
  /// `Shortcuts` 收不到按键（这就是"无法按 Esc 退出"的成因）。
  final _focus = FocusNode(debugLabel: 'fullscreen-video');

  @override
  void initState() {
    super.initState();
    _open();
  }

  Future<void> _open() async {
    final controller = WinVideoPlayerController.networkUrl(Uri.parse(widget.url));
    _controller = controller;
    try {
      await controller.initialize();
      if (!mounted) return;
      if (!controller.value.isInitialized) {
        setState(() => _error = '无法打开视频（可能缺少解码器）');
        return;
      }
      // 接着刚才的位置继续看
      if (widget.startAt > Duration.zero) {
        await controller.seekTo(widget.startAt);
      }
      setState(() => _ready = true);
      await controller.play();
      // 自动接管焦点，Esc 才有效（点击控件后焦点会移过去，退出按钮依然在）
      _focus.requestFocus();
    } catch (e) {
      if (mounted) setState(() => _error = '全屏播放失败: $e');
    }
  }

  /// 退出全屏：把当前进度交还给内嵌那一路，避免"退出后从头开始"。
  void _exit() {
    Navigator.of(context).pop<Duration>(_controller?.value.position ?? Duration.zero);
  }

  @override
  void dispose() {
    _focus.dispose();
    _controller?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
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
                if (_error != null)
                  Center(
                    child: Text(_error!, style: const TextStyle(color: Colors.white70)),
                  )
                else if (!_ready)
                  _PosterOrSpinner(url: widget.posterUrl)
                else
                  Positioned.fill(
                    child: LayoutBuilder(
                      builder: (context, box) {
                        final ratio = _WinVideoViewState._ratioOf(_controller!);
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
                            child: WinVideoPlayer(_controller!),
                          ),
                        );
                      },
                    ),
                  ),
                // 顶部：退出全屏
                Positioned(
                  left: 8,
                  top: 8,
                  child: _OverlayButton(
                    icon: Icons.fullscreen_exit,
                    tooltip: '退出全屏（Esc）',
                    onTap: _exit,
                  ),
                ),
                if (_ready)
                  Positioned(
                    left: 0,
                    right: 0,
                    bottom: 0,
                    child: ColoredBox(
                      color: Colors.black54,
                      child: _Controls(
                        controller: _controller!,
                        onFullscreen: null,
                        dark: true,
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

/// 播放器上悬浮的圆形按钮（全屏 / 退出全屏）。
class _OverlayButton extends StatelessWidget {
  final IconData icon;
  final String tooltip;
  final VoidCallback onTap;

  const _OverlayButton({required this.icon, required this.tooltip, required this.onTap});

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: tooltip,
      child: Material(
        color: Colors.black45,
        shape: const CircleBorder(),
        child: InkWell(
          customBorder: const CircleBorder(),
          onTap: onTap,
          child: Padding(
            padding: const EdgeInsets.all(8),
            child: Icon(icon, color: Colors.white, size: 22),
          ),
        ),
      ),
    );
  }
}

class _Controls extends StatefulWidget {
  final WinVideoPlayerController controller;

  /// 控制条上的全屏按钮；传 null 表示已经有别的全屏入口（全屏页自己不再显示）。
  final VoidCallback? onFullscreen;

  /// 黑底场景（全屏页）：文字与图标用白色。
  final bool dark;

  const _Controls({required this.controller, this.onFullscreen, this.dark = false});

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
            icon: Icon(v.isPlaying ? Icons.pause_circle_filled : Icons.play_circle_fill,
                size: 34, color: fg),
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
                  : (value) => widget.controller.seekTo(Duration(milliseconds: value.toInt())),
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
              tooltip: '全屏（Esc 退出）',
              icon: Icon(Icons.fullscreen, size: 22, color: fg),
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
