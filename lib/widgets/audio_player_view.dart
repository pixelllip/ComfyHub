import 'package:audioplayers/audioplayers.dart';
import 'package:flutter/material.dart';

/// 音频播放器（audioplayers，Windows / Android / iOS 都支持）。
class AudioPlayerView extends StatefulWidget {
  final String url;
  final String title;

  const AudioPlayerView({super.key, required this.url, this.title = ''});

  @override
  State<AudioPlayerView> createState() => _AudioPlayerViewState();
}

class _AudioPlayerViewState extends State<AudioPlayerView> {
  final _player = AudioPlayer();
  Duration _position = Duration.zero;
  Duration _duration = Duration.zero;
  bool _playing = false;
  String? _error;
  bool _dragging = false;
  double _dragValue = 0;

  @override
  void initState() {
    super.initState();
    _player.onPositionChanged.listen((p) {
      if (!_dragging && mounted) setState(() => _position = p);
    });
    _player.onDurationChanged.listen((d) {
      if (mounted) setState(() => _duration = d);
    });
    _player.onPlayerStateChanged.listen((s) {
      if (mounted) setState(() => _playing = s == PlayerState.playing);
    });
    _player.onPlayerComplete.listen((_) {
      if (mounted) {
        setState(() {
          _playing = false;
          _position = Duration.zero;
        });
      }
    });
  }

  @override
  void dispose() {
    _player.dispose();
    super.dispose();
  }

  Future<void> _toggle() async {
    try {
      if (_playing) {
        await _player.pause();
      } else if (_position > Duration.zero && _position < _duration) {
        await _player.resume();
      } else {
        await _player.play(UrlSource(widget.url));
      }
    } catch (e) {
      setState(() => _error = '$e');
    }
  }

  String _fmt(Duration d) {
    final m = d.inMinutes.remainder(60).toString().padLeft(2, '0');
    final s = d.inSeconds.remainder(60).toString().padLeft(2, '0');
    return d.inHours > 0 ? '${d.inHours}:$m:$s' : '$m:$s';
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final total = _duration.inMilliseconds;
    final value = _dragging
        ? _dragValue
        : _position.inMilliseconds.clamp(0, total == 0 ? 1 : total).toDouble();

    return Container(
      // 紧凑布局：以前是"64px 大图标 + 居中标题 + 播放行"三行，光组件本身就 ~230px，
      // 详情页一打开音频就吃掉大半屏。现在压成"小图标 + 标题一行 + 播放行一行"。
      padding: const EdgeInsets.fromLTRB(16, 12, 12, 10),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(14),
        gradient: LinearGradient(
          colors: [
            theme.colorScheme.primary.withValues(alpha: 0.16),
            theme.colorScheme.tertiary.withValues(alpha: 0.10),
          ],
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
        ),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (widget.title.isNotEmpty) ...[
            Row(
              children: [
                Icon(Icons.graphic_eq, size: 26, color: theme.colorScheme.primary),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    widget.title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.titleSmall,
                  ),
                ),
              ],
            ),
            const SizedBox(height: 2),
          ],
          if (_error != null)
            Text(
              '播放失败: $_error',
              style: TextStyle(color: theme.colorScheme.error),
              textAlign: TextAlign.center,
            )
          else
            Row(
              children: [
                IconButton.filled(
                  iconSize: 24,
                  visualDensity: VisualDensity.compact,
                  tooltip: _playing ? '暂停' : '播放',
                  onPressed: _toggle,
                  icon: Icon(_playing ? Icons.pause : Icons.play_arrow),
                ),
                const SizedBox(width: 8),
                Text(_fmt(_position), style: theme.textTheme.labelMedium),
                Expanded(
                  child: Slider(
                    value: value,
                    max: (total == 0 ? 1 : total).toDouble(),
                    onChanged: total == 0
                        ? null
                        : (v) => setState(() {
                              _dragging = true;
                              _dragValue = v;
                            }),
                    onChangeEnd: total == 0
                        ? null
                        : (v) async {
                            await _player.seek(Duration(milliseconds: v.toInt()));
                            setState(() {
                              _dragging = false;
                              _position = Duration(milliseconds: v.toInt());
                            });
                          },
                  ),
                ),
                Text(_fmt(_duration), style: theme.textTheme.labelMedium),
              ],
            ),
        ],
      ),
    );
  }
}
