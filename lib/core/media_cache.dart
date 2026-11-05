import 'dart:async';
import 'dart:io';

import 'package:path_provider/path_provider.dart';

import 'api_client.dart';

/// 缩略图预算：512px 一张 30~80KB，150MB 能放几千张 —— 正常使用碰不到。
const int kThumbCacheBudgetBytes = 150 * 1024 * 1024;

/// 原件默认预算（1.5GB）。设置页可调，见 [kOriginalCacheBudgetChoicesMb]。
const int kOriginalCacheBudgetBytes = 1536 * 1024 * 1024;

/// 设置页给用户的档位（MB）。**默认值必须在这张表里**，否则下拉框会出现
/// "当前值不在选项里"的空白项（`MediaCacheOrigBudget` 的 `null` 那一档）。
const List<int> kOriginalCacheBudgetChoicesMb = [512, 1024, 1536, 2048, 4096];

/// 超上限后一次性淘汰到预算的这个比例，**别写到 100% 才淘汰一条**：
/// 那样每下一条都会淘汰一条，缓存变成"永远只有最新那几条"。
const double kCacheEvictLowWater = 0.8;

/// 打开详情页时**自动**下载原件的单条上限。
///
/// 超过它就不在"打开详情"时自动下（交给用户点「保存到手机相册」）——
/// 一条 300MB 的视频，用户只是划过去看一眼，不该因此占掉 1/5 的预算、
/// 也不该让他事后自己去猜空间去哪了。
const int kAutoCacheMaxBytes = 200 * 1024 * 1024;

/// 保存到相册之后，**原文大于这个值**才删掉缓存副本（相册里已经有一份）。
///
/// 小图不删：删了只会让"再点开一次"重新下一遍，省下的空间可以忽略。
const int kEvictAfterSaveMinBytes = 64 * 1024 * 1024;

/// 手机端**产物缓存**：把看过的缩略图 / 原件落到本机，默认"收到就缓存"。
///
/// ## 为什么不再只靠 `Image.network`
///
/// 同步 API 只同步 `media_assets` 的元数据（大小 / 哈希 / 提示词），**字节一直在桌面**；
/// 画廊每张图都走网络，桌面一离线就只剩占位图。这里把**用户实际看过的**字节存到
/// `<app-support>/media-cache/`：
///   * 缩略图在格子第一次出现时下载；
///   * 原件在详情页打开时下载（视频 / 音频也落盘，供「保存到手机相册」用）。
///
/// 刻意**不**在同步阶段把所有原件一次拉下来：一个画廊可能是几十 GB，手机存不下，
/// 而且绝大多数条目用户根本不会打开。缓存是"看过的留下"，不是"整库镜像"。
///
/// ## 四条纪律
///
/// 1. **原子落盘**：先写 `.part` 再 rename；下载失败不留下半张图（否则下次会命中坏文件）。
/// 2. **并发去重**：同一个 id/尺寸同时只跑一个下载（快速滑动时同一格会 build 多次）。
/// 3. **失败静默但不假装**：缓存失败返回 `null`，调用方继续用网络图；**不**把缓存
///    当成"产物已经同步完成"。
/// 4. **有上限**（2026-11-01 用户要求）：两档预算分开算，超了按 **LRU 淘汰到 80%**，
///    不是"到顶就不缓存"。理由见 [_trimOnce] 与 [kCacheEvictLowWater]。
///
/// ## 为什么上限按"总字节"而不是"条数"
///
/// 同一条缓存可能是 40KB 的缩略图，也可能是 300MB 的视频；按条数设限等于
/// "100 张缩略图 = 100 个视频"，两条路完全不是一回事。所以缩略图与原件**各算各的**，
/// 且原件那条还有单条上限（[kAutoCacheMaxBytes]）。
class MediaCache {
  MediaCache({
    Directory? root,
    bool? enabled,
    int? thumbBudget,
    int? origBudget,
  }) : _rootOverride = root,
       _enabledOverride = enabled,
       _thumbBudget = thumbBudget ?? kThumbCacheBudgetBytes,
       _origBudget = origBudget ?? kOriginalCacheBudgetBytes;

  /// 生产单例（目录在第一次真正用到时才解析，测试里不会碰 path_provider）。
  static final MediaCache instance = MediaCache();

  final Directory? _rootOverride;
  final bool? _enabledOverride;

  final int _thumbBudget;
  int _origBudget;

  Directory? _root;
  String? _dirPath;

  /// 同一个 key 的下载只跑一次。
  final Map<String, Future<File?>> _inflight = {};

  /// 目录只扫一次（之后靠 [_record] / [_forget] 增量维护）。
  Future<void>? _scan;

  /// 文件名 → 大小 + 最后一次访问时间（LRU 的唯一依据）。
  final Map<String, _CacheEntry> _index = {};
  int _thumbBytes = 0;
  int _origBytes = 0;

  /// 淘汰串行化：两次 trim 撞在一起会各删一遍、把统计删漂。
  Future<void> _trimQueue = Future<void>.value();

  /// 当前平台要不要做磁盘缓存（桌面不需要：原件本来就在桌面上）。
  bool get enabled {
    if (_enabledOverride != null) return _enabledOverride;
    try {
      return Platform.isAndroid || Platform.isIOS;
    } catch (_) {
      return false;
    }
  }

  int get thumbBudgetBytes => _thumbBudget;
  int get originalBudgetBytes => _origBudget;

  /// 改原件预算（设置页）。**立刻生效并顺手淘汰一次** —— 用户把 2GB 调到 512MB
  /// 之后，占用必须当场降下来，不能等下一次下载。
  void setOriginalBudget(int bytes) {
    final clamped = bytes.clamp(64 * 1024 * 1024, 64 * 1024 * 1024 * 1024);
    if (clamped == _origBudget) return;
    _origBudget = clamped;
    unawaited(trim());
  }

  /// 打开详情页时要不要**自动**下载原件（[sizeBytes] 为 0 = 桌面没给大小）。
  ///
  /// 视频 / 音频先按"不知道大小就不自动下"处理：详情页本身能在线播，
  /// 想存到手机是用户的显式动作（「保存到手机相册」）。
  static bool shouldAutoCache({
    required int sizeBytes,
    required bool streaming,
  }) {
    if (sizeBytes <= 0) return !streaming;
    return sizeBytes <= kAutoCacheMaxBytes;
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
    _dirPath = base.path;
    return base;
  }

  static String _nameFor(int mediaId, {required bool thumb}) =>
      'media-$mediaId${thumb ? '.thumb' : '.orig'}';

  static String _keyFor(int mediaId, {required bool thumb}) =>
      '$mediaId:${thumb ? 1 : 0}';

  /// 文件名 → 下载去重的 key（`media-7.thumb` → `7:1`）；认不出来回 null。
  static String? _keyOfName(String name) {
    if (name.endsWith('.thumb')) {
      final id = name.substring(name.indexOf('-') + 1, name.length - 6);
      return '$id:1';
    }
    if (name.endsWith('.orig')) {
      final id = name.substring(name.indexOf('-') + 1, name.length - 5);
      return '$id:0';
    }
    return null;
  }

  static bool? _thumbOfName(String name) {
    if (name.endsWith('.thumb')) return true;
    if (name.endsWith('.orig')) return false;
    return null;
  }

  /// 缓存文件的**稳定路径**（不存在也返回路径，调用方自己判断）。
  ///
  /// `thumb=true` 与原件分开存：缩略图只有 512px，原件可能是 4K；共用一个文件会让
  /// "先看格子、再点详情"时详情页拿到缩略图。
  Future<File> fileFor(int mediaId, {required bool thumb}) async {
    final dir = await _cacheDir();
    return File('${dir.path}/${_nameFor(mediaId, thumb: thumb)}');
  }

  /// 已经缓存好的文件；没有 / 是空文件（上次写坏）返回 `null`。
  ///
  /// **命中就算一次访问**：把 mtime 推到现在，淘汰时它自然排在最后（LRU）。
  Future<File?> cached(int mediaId, {required bool thumb}) async {
    if (!enabled) return null;
    try {
      final name = _nameFor(mediaId, thumb: thumb);
      final file = await fileFor(mediaId, thumb: thumb);
      final stat = await file.stat();
      if (stat.size <= 0) {
        await file.delete();
        _forget(name);
        return null;
      }
      _touch(file, name, stat.size, thumb: thumb);
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
    final key = _keyFor(mediaId, thumb: thumb);
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
    File? part;
    try {
      final existing = await cached(mediaId, thumb: thumb);
      if (existing != null) return existing;
      final name = _nameFor(mediaId, thumb: thumb);
      final file = await fileFor(mediaId, thumb: thumb);
      part = File('${file.path}.part');
      // **流式落盘**：视频 / 大图不能先整个读进内存（见 ApiClient.downloadToFile）。
      final size = await api.downloadToFile(url, part);
      if (size <= 0) {
        await _deleteQuietly(part);
        return null;
      }
      // 单条就比整档预算还大：留在盘上只会把别人全挤掉，还把"清得掉吗"变谜。
      if (size > _budgetFor(thumb)) {
        await _deleteQuietly(part);
        // ignore: avoid_print
        print('[media-cache] $name 有 $size 字节，超过本档预算，不缓存');
        return null;
      }
      if (await file.exists()) await file.delete();
      await part.rename(file.path);
      _record(name, size, thumb: thumb);
      // 写进来就可能超了：顺手淘汰（当前这条还在 `_inflight` 里，不会被删）。
      await trim();
      return file;
    } catch (e) {
      // 下载中途断了 / 写盘失败，**残片当场清掉**（别等下次扫描的"超过 1 小时"，
      // 那期间它是纯粹的白占空间，而且下一条会写进同一个文件名）。
      if (part != null) await _deleteQuietly(part);
      // 缓存是"锦上添花"：失败就继续用网络图，别把详情页打成错误页。
      // 但日志要留，真机上排查"为什么离线没有图"就靠它。
      // ignore: avoid_print
      print('[media-cache] 缓存 $mediaId（${thumb ? 'thumb' : 'orig'}）失败，$e');
      return null;
    }
  }

  /// 删掉单条的缓存（保存到相册之后收空间用；**只删原件，不动缩略图**）。
  Future<bool> remove(int mediaId, {required bool thumb}) async {
    if (!enabled) return false;
    try {
      final file = await fileFor(mediaId, thumb: thumb);
      if (!await file.exists()) {
        _forget(_nameFor(mediaId, thumb: thumb));
        return false;
      }
      await file.delete();
      _forget(_nameFor(mediaId, thumb: thumb));
      return true;
    } catch (_) {
      return false;
    }
  }

  /// 当前占用（设置页「已用 / 上限」）。桌面端返回全 0，不碰 path_provider。
  Future<MediaCacheStats> stats() async {
    if (enabled) {
      try {
        await _ensureScanned();
      } catch (_) {
        // 目录都列不出来就按 0 报，别让设置页炸掉
      }
    }
    var thumbCount = 0;
    var origCount = 0;
    for (final entry in _index.values) {
      if (entry.thumb) {
        thumbCount++;
      } else {
        origCount++;
      }
    }
    return MediaCacheStats(
      thumbBytes: _thumbBytes,
      origBytes: _origBytes,
      thumbCount: thumbCount,
      origCount: origCount,
      thumbBudget: _thumbBudget,
      origBudget: _origBudget,
    );
  }

  /// 清空整个缓存目录（设置页「清理缓存」）。返回释放的字节数。
  ///
  /// 正在下载的那一条**跳过**（`.part` 与目标文件）：删了会让那次下载变成
  /// "写进了一个已经没了的路径"，用户看到的是保存失败。
  Future<int> clear() async {
    if (!enabled) return 0;
    final dir = await _cacheDir();
    var freed = 0;
    try {
      await for (final entity in dir.list()) {
        if (entity is! File) continue;
        final name = entity.uri.pathSegments.last;
        final key = _keyOfName(name.endsWith('.part')
            ? name.substring(0, name.length - 5)
            : name);
        if (key != null && _inflight.containsKey(key)) continue;
        try {
          freed += await entity.length();
          await entity.delete();
        } catch (_) {
          // 删不掉就跳过（别让一个坏文件挡住整次清理）
        }
      }
    } catch (_) {
      // 列目录失败，返回已经清掉的部分
    }
    _index.clear();
    _thumbBytes = 0;
    _origBytes = 0;
    return freed;
  }

  /// 按 LRU 把两档各自淘汰到预算以内。
  ///
  /// **淘汰到 80% 而不是刚好压线**（[kCacheEvictLowWater]）：刚好压线的话，
  /// 下一条写进来又会触发一轮淘汰 —— 每写一条删一条，缓存形同虚设。
  Future<void> trim() {
    _trimQueue = _trimQueue.then((_) => _trimOnce()).catchError((_) {});
    return _trimQueue;
  }

  Future<void> _trimOnce() async {
    if (!enabled) return;
    try {
      await _ensureScanned();
    } catch (_) {
      return;
    }
    final dirPath = _dirPath;
    if (dirPath == null) return;
    for (final thumb in const [true, false]) {
      final budget = _budgetFor(thumb);
      var total = thumb ? _thumbBytes : _origBytes;
      if (total <= budget) continue;
      final target = (budget * kCacheEvictLowWater).round();
      final doomed =
          _index.entries
              .where((e) => e.value.thumb == thumb && !_isInflight(e.key))
              .toList()
            ..sort((a, b) => a.value.at.compareTo(b.value.at));
      for (final item in doomed) {
        if (total <= target) break;
        try {
          await File('$dirPath/${item.key}').delete();
        } catch (_) {
          // 删不掉（比如正被播放器开着）：跳过，别卡住整轮
          continue;
        }
        _index.remove(item.key);
        total -= item.value.size;
      }
      if (thumb) {
        _thumbBytes = total;
      } else {
        _origBytes = total;
      }
    }
  }

  bool _isInflight(String name) {
    final key = _keyOfName(name);
    return key != null && _inflight.containsKey(key);
  }

  int _budgetFor(bool thumb) => thumb ? _thumbBudget : _origBudget;

  Future<void> _ensureScanned() => _scan ??= _scanDir();

  Future<void> _scanDir() async {
    final dir = await _cacheDir();
    try {
      await for (final entity in dir.list()) {
        if (entity is! File) continue;
        final name = entity.uri.pathSegments.last;
        if (name.endsWith('.part')) {
          // 上一个进程留下的残片：**超过 1 小时才删**（可能有一次下载正在进行）。
          try {
            final stat = await entity.stat();
            if (DateTime.now().difference(stat.modified) >
                const Duration(hours: 1)) {
              await entity.delete();
            }
          } catch (_) {
            // 忽略
          }
          continue;
        }
        final thumb = _thumbOfName(name);
        if (thumb == null) continue;
        try {
          final stat = await entity.stat();
          if (stat.size <= 0) {
            await entity.delete();
            continue;
          }
          _index[name] = _CacheEntry(
            size: stat.size,
            at: stat.modified,
            thumb: thumb,
          );
        } catch (_) {
          // 单个文件读不出来不影响别的
        }
      }
    } catch (_) {
      // 目录列不出来：当作空缓存
    }
    _recount();
  }

  void _recount() {
    var thumb = 0;
    var orig = 0;
    for (final entry in _index.values) {
      if (entry.thumb) {
        thumb += entry.size;
      } else {
        orig += entry.size;
      }
    }
    _thumbBytes = thumb;
    _origBytes = orig;
  }

  void _record(String name, int size, {required bool thumb}) {
    _forget(name);
    _index[name] = _CacheEntry(size: size, at: DateTime.now(), thumb: thumb);
    if (thumb) {
      _thumbBytes += size;
    } else {
      _origBytes += size;
    }
  }

  void _forget(String name) {
    final prev = _index.remove(name);
    if (prev == null) return;
    if (prev.thumb) {
      _thumbBytes = (_thumbBytes - prev.size).clamp(0, 1 << 62);
    } else {
      _origBytes = (_origBytes - prev.size).clamp(0, 1 << 62);
    }
  }

  /// 记一次访问：更新内存里的 mtime，并把文件本身的 mtime 也推到现在
  /// （下次冷启动扫描时 LRU 顺序还在）。
  void _touch(
    File file,
    String name,
    int size, {
    required bool thumb,
  }) {
    final now = DateTime.now();
    final entry = _index[name];
    if (entry == null) {
      _index[name] = _CacheEntry(size: size, at: now, thumb: thumb);
      if (thumb) {
        _thumbBytes += size;
      } else {
        _origBytes += size;
      }
    } else {
      entry.at = now;
    }
    unawaited(file.setLastModified(now).then<void>((_) {}, onError: (_) {}));
  }

  Future<void> _deleteQuietly(File file) async {
    try {
      await file.delete();
    } catch (_) {
      // 已经没了 / 删不掉：都不影响"这次不缓存"
    }
  }
}

/// 设置页显示的缓存占用快照。
class MediaCacheStats {
  const MediaCacheStats({
    required this.thumbBytes,
    required this.origBytes,
    required this.thumbCount,
    required this.origCount,
    required this.thumbBudget,
    required this.origBudget,
  });

  final int thumbBytes;
  final int origBytes;
  final int thumbCount;
  final int origCount;
  final int thumbBudget;
  final int origBudget;

  int get totalBytes => thumbBytes + origBytes;
  int get totalBudget => thumbBudget + origBudget;
}

class _CacheEntry {
  _CacheEntry({required this.size, required this.at, required this.thumb});

  final int size;
  DateTime at;
  final bool thumb;
}
