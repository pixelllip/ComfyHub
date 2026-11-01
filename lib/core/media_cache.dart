import 'dart:io';

import 'package:path_provider/path_provider.dart';

import 'api_client.dart';

/// 手机端**产物缓存**：把看过的缩略图 / 原件落到本机，默认"收到就缓存"。
///
/// ## 为什么不再只靠 `Image.network`
///
/// 同步 API 只同步 `media_assets` 的元数据（大小 / 哈希 / 提示词），**字节一直在桌面**；
/// 画廊每张图都走网络，桌面一离线就只剩占位图。这里把**用户实际看过的**字节存到
/// `<app-support>/media-cache/`：
///   * 缩略图在格子第一次出现时下载；
///   * 原件在详情页打开时下载（视频 / 音频也落盘，供「保存到手机」用）。
///
/// 刻意**不**在同步阶段把所有原件一次拉下来：一个画廊可能是几十 GB，手机存不下，
/// 而且绝大多数条目用户根本不会打开。缓存是"看过的留下"，不是"整库镜像"。
///
/// ## 三条纪律
///
/// 1. **原子落盘**：先写 `.part` 再 rename；下载失败不留下半张图（否则下次会命中坏文件）。
/// 2. **并发去重**：同一个 id/尺寸同时只跑一个下载（快速滑动时同一格会 build 多次）。
/// 3. **失败静默但不假装**：缓存失败返回 `null`，调用方继续用网络图；**不**把缓存
///    当成"产物已经同步完成"。
class MediaCache {
  MediaCache({Directory? root, bool? enabled})
    : _rootOverride = root,
      _enabledOverride = enabled;

  /// 生产单例（目录在第一次真正用到时才解析，测试里不会碰 path_provider）。
  static final MediaCache instance = MediaCache();

  final Directory? _rootOverride;
  final bool? _enabledOverride;

  Directory? _root;

  /// 同一个 key 的下载只跑一次。
  final Map<String, Future<File?>> _inflight = {};

  /// 当前平台要不要做磁盘缓存（桌面不需要：原件本来就在桌面上）。
  bool get enabled {
    if (_enabledOverride != null) return _enabledOverride;
    try {
      return Platform.isAndroid || Platform.isIOS;
    } catch (_) {
      return false;
    }
  }

  Future<Directory> _cacheDir() async {
    final cached = _root;
    if (cached != null) return cached;
    final base =
        _rootOverride ??
        Directory(
          '${(await getApplicationSupportDirectory()).path}/media-cache',
        );
    if (!await base.exists()) await base.create(recursive: true);
    _root = base;
    return base;
  }

  /// 缓存文件的**稳定路径**（不存在也返回路径，调用方自己判断）。
  ///
  /// `thumb=true` 与原件分开存：缩略图只有 512px，原件可能是 4K；共用一个文件会让
  /// "先看格子、再点详情"时详情页拿到缩略图。
  Future<File> fileFor(int mediaId, {required bool thumb}) async {
    final dir = await _cacheDir();
    return File('${dir.path}/media-$mediaId${thumb ? '.thumb' : '.orig'}');
  }

  /// 已经缓存好的文件；没有 / 是空文件（上次写坏）返回 `null`。
  Future<File?> cached(int mediaId, {required bool thumb}) async {
    if (!enabled) return null;
    try {
      final file = await fileFor(mediaId, thumb: thumb);
      if (!await file.exists()) return null;
      if (await file.length() <= 0) {
        await file.delete();
        return null;
      }
      return file;
    } catch (_) {
      return null;
    }
  }

  /// 确保有缓存；没有就下载。失败返回 `null`，**绝不抛到界面**。
  ///
  /// [url] 可以是相对路径（`ApiClient.absolute` 会补 baseUrl）。
  Future<File?> ensure(
    int mediaId, {
    required String url,
    required bool thumb,
    required ApiClient api,
  }) {
    if (!enabled) return Future<File?>.value();
    final key = '$mediaId:${thumb ? 1 : 0}';
    final running = _inflight[key];
    if (running != null) return running;
    final future = _download(mediaId, url: url, thumb: thumb, api: api);
    _inflight[key] = future;
    return future.whenComplete(() => _inflight.remove(key));
  }

  Future<File?> _download(
    int mediaId, {
    required String url,
    required bool thumb,
    required ApiClient api,
  }) async {
    try {
      final existing = await cached(mediaId, thumb: thumb);
      if (existing != null) return existing;
      final bytes = await api.downloadBytes(url);
      if (bytes.isEmpty) return null;
      final file = await fileFor(mediaId, thumb: thumb);
      final part = File('${file.path}.part');
      await part.writeAsBytes(bytes, flush: true);
      if (await file.exists()) await file.delete();
      await part.rename(file.path);
      return file;
    } catch (e) {
      // 缓存是"锦上添花"：失败就继续用网络图，别把详情页打成错误页。
      // 但日志要留，真机上排查"为什么离线没有图"就靠它。
      // ignore: avoid_print
      print('[media-cache] 缓存 $mediaId（${thumb ? 'thumb' : 'orig'}）失败：$e');
      return null;
    }
  }
}
