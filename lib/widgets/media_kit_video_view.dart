import 'package:material_ui/material_ui.dart';
import 'package:media_kit/media_kit.dart';
import 'package:media_kit_video/media_kit_video.dart';

/// Windows 的播放实现：**media_kit**（libmpv，GPU 渲染 + 硬解）。
///
/// 4K 高码率文件把 demuxer 缓冲开大（128MB），来回拖动时不必反复读盘；
/// libplacebo 的 tone mapping 只作用于视频画面，不影响 Flutter UI。
class MediaKitVideoView extends StatefulWidget {
  const MediaKitVideoView({
    super.key,
    required this.url,
    this.localPath,
    this.posterUrl,
    this.autoPlay = false,
    this.loop = false,
  });

  final String url;

  /// 本机原件的绝对路径：有它就直接让 libmpv 读文件，不走 HTTP。
  final String? localPath;
  final String? posterUrl;
  final bool autoPlay;
  final bool loop;

  @override
  State<MediaKitVideoView> createState() => _MediaKitVideoViewState();
}

class _MediaKitVideoViewState extends State<MediaKitVideoView> {
  late final Player _player;
  late final VideoController _controller;
  String? _error;

  @override
  void initState() {
    super.initState();
    MediaKit.ensureInitialized();
    _player = Player(
      configuration: const PlayerConfiguration(logLevel: MPVLogLevel.error),
    );
    _controller = VideoController(_player);
    _open();
  }

  Future<void> _open() async {
    final path = widget.localPath;
    final source = (path != null && path.isNotEmpty) ? path : widget.url;
    try {
      await _player.open(Media(source), play: widget.autoPlay);
      final native = _player.platform;
      if (native is NativePlayer) {
        await native.setProperty('hr-seek', 'yes');
      }
      if (widget.loop) {
        await _player.setPlaylistMode(PlaylistMode.loop);
      }
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
    }
  }

  @override
  void dispose() {
    _player.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (_error != null) {
      return ColoredBox(
        color: Colors.black,
        child: Center(
          child: Padding(
            padding: const EdgeInsets.all(24),
            child: Text(
              '视频播放失败：$_error',
              textAlign: TextAlign.center,
              style: const TextStyle(color: Colors.white70),
            ),
          ),
        ),
      );
    }
    return Video(controller: _controller, fit: BoxFit.contain);
  }
}
