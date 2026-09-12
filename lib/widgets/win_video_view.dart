import 'dart:io';

import 'package:flutter/material.dart';
import 'package:video_player_win/video_player_win.dart';

/// Windows 上的视频播放器（基于 Media Foundation）。
/// 其他平台不支持，调用方需要先用 [WinVideoView.supported] 判断。
class WinVideoView extends StatefulWidget {
  final String url;
  final bool autoPlay;
  final bool loop;

  const WinVideoView({
    super.key,
    required this.url,
    this.autoPlay = false,
    this.loop = false,
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

  @override
  Widget build(BuildContext context) {
    if (_error != null) {
      return _FallbackMessage(message: _error!, url: widget.url);
    }
    final controller = _controller;
    if (controller == null || !_ready) {
      return const Center(child: CircularProgressIndicator());
    }

    return Column(
      children: [
        Expanded(
          child: Center(
            child: AspectRatio(
              aspectRatio: controller.value.aspectRatio == 0
                  ? 16 / 9
                  : controller.value.aspectRatio,
              child: WinVideoPlayer(controller),
            ),
          ),
        ),
        _Controls(controller: controller),
      ],
    );
  }
}

class _Controls extends StatefulWidget {
  final WinVideoPlayerController controller;

  const _Controls({required this.controller});

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

    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      child: Row(
        children: [
          IconButton(
            icon: Icon(v.isPlaying ? Icons.pause_circle_filled : Icons.play_circle_fill, size: 34),
            onPressed: () async {
              if (v.isPlaying) {
                await widget.controller.pause();
              } else {
                await widget.controller.play();
              }
              setState(() {});
            },
          ),
          Text(_fmt(v.position), style: const TextStyle(fontSize: 12)),
          Expanded(
            child: Slider(
              value: pos.toDouble(),
              max: (total == 0 ? 1 : total).toDouble(),
              onChanged: total == 0
                  ? null
                  : (value) => widget.controller.seekTo(Duration(milliseconds: value.toInt())),
            ),
          ),
          Text(_fmt(v.duration), style: const TextStyle(fontSize: 12)),
          const SizedBox(width: 8),
          Icon(
            v.volume == 0 ? Icons.volume_off : Icons.volume_up,
            size: 18,
            color: Theme.of(context).colorScheme.outline,
          ),
          SizedBox(
            width: 90,
            child: Slider(
              value: v.volume.clamp(0.0, 1.0),
              onChanged: (value) => widget.controller.setVolume(value),
            ),
          ),
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
