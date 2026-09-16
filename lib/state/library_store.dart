import 'package:flutter/foundation.dart';

import '../core/api_client.dart';
import '../core/settings_store.dart';
import '../models/models.dart';

/// 全局数据仓库：提示词库 / 媒体画廊 / 标签词表 的搜索状态与缓存。
class LibraryStore extends ChangeNotifier {
  /// [api] 只用于测试时注入假的 HTTP 客户端；生产代码传 null 即可。
  LibraryStore(this._settings, {ApiClient? api}) {
    _apiInjected = api != null;
    this.api = api ?? ApiClient(_settings.baseUrl);
    _apiBaseUrl = _settings.baseUrl;
    _settings.addListener(_onSettingsChanged);
  }

  final SettingsStore _settings;

  late ApiClient api;
  String _apiBaseUrl = '';
  /// 测试注入的客户端不允许被 baseUrl 变化替换掉
  bool _apiInjected = false;

  // -------------------------------------------------------------------------
  //  提示词
  // -------------------------------------------------------------------------

  Paged<Prompt> prompts = Paged.empty<Prompt>();
  bool loadingPrompts = false;
  String? promptsError;

  String promptQuery = '';
  final Set<String> selectedTags = <String>{};
  String tagMode = 'any';
  String? promptKind;
  bool onlyFavorite = false;

  /// 只看「未关联产物」的提示词（用户要求）：产物被删掉之后，
  /// 对应提示词的 mediaCount 会变成 0，用这个筛出来一键清理。
  bool onlyUnlinked = false;
  String promptSort = 'newest';
  int promptPage = 1;

  // -------------------------------------------------------------------------
  //  媒体
  // -------------------------------------------------------------------------

  Paged<MediaAsset> media = Paged.empty<MediaAsset>();
  bool loadingMedia = false;
  String? mediaError;

  String mediaQuery = '';
  final Set<String> mediaTags = <String>{};
  String mediaTagMode = 'any';
  String? mediaKind;
  bool mediaOnlyFavorite = false;
  bool mediaUntagged = false;
  String mediaSort = 'newest';
  int mediaPage = 1;

  // -------------------------------------------------------------------------
  //  标签 / 统计
  // -------------------------------------------------------------------------

  List<Tag> tags = const [];
  List<String> tagCategories = const [];
  bool loadingTags = false;
  String? tagsError;

  LibraryStats stats = const LibraryStats();

  bool _disposed = false;

  String get baseUrl => _settings.baseUrl;

  void _onSettingsChanged() {
    if (_apiInjected) return;
    if (_settings.baseUrl != _apiBaseUrl) {
      api.dispose();
      api = ApiClient(_settings.baseUrl);
      _apiBaseUrl = _settings.baseUrl;
      refreshAll();
    }
  }

  @override
  void dispose() {
    _disposed = true;
    _settings.removeListener(_onSettingsChanged);
    api.dispose();
    super.dispose();
  }

  void _safeNotify() {
    if (!_disposed) notifyListeners();
  }

  // -------------------------------------------------------------------------
  //  加载
  // -------------------------------------------------------------------------

  Future<void> refreshAll() async {
    await Future.wait([
      refreshPrompts(),
      refreshMedia(),
      refreshTags(),
      refreshStats(),
    ]);
  }

  Future<void> refreshStats() async {
    try {
      stats = await api.stats();
      _safeNotify();
    } catch (_) {
      // 统计失败不影响主流程
    }
  }

  /// 同 [refreshMedia]：批量删除后当前页可能已经不存在，自动回到最后一页，
  /// 避免"删了几个却显示整个库空了"。
  Future<void> refreshPrompts() async {
    loadingPrompts = true;
    promptsError = null;
    _safeNotify();
    try {
      var result = await _listPrompts(page: promptPage);
      if (result.items.isEmpty && result.total > 0 && promptPage > result.pages) {
        promptPage = result.pages < 1 ? 1 : result.pages;
        result = await _listPrompts(page: promptPage);
      }
      prompts = result;
    } on ApiException catch (e) {
      promptsError = e.toString();
      prompts = Paged.empty<Prompt>();
    } catch (e) {
      promptsError = '$e';
      prompts = Paged.empty<Prompt>();
    } finally {
      loadingPrompts = false;
      _safeNotify();
    }
  }

  Future<Paged<Prompt>> _listPrompts({required int page}) => api.listPrompts(
        q: promptQuery.isEmpty ? null : promptQuery,
        tags: selectedTags.toList(),
        tagMode: tagMode,
        kind: promptKind,
        favorite: onlyFavorite ? true : null,
        hasMedia: onlyUnlinked ? false : null,
        sort: promptSort,
        page: page,
        size: _settings.pageSize,
      );

  /// 未关联产物的提示词条数（一键清除入口要先告诉用户会删掉多少）。
  Future<int> countUnlinkedPrompts() async {
    try {
      final page = await api.listPrompts(hasMedia: false, page: 1, size: 1);
      return page.total;
    } catch (_) {
      return 0;
    }
  }

  /// 未关联任何提示词的产物条数（画廊那边的「清除未关联产物」）。
  Future<int> countUnlinkedMedia() async {
    try {
      final page = await api.listMedia(untagged: true, page: 1, size: 1);
      return page.total;
    } catch (_) {
      return 0;
    }
  }

  /// 拉当前这一页产物。
  ///
  /// **页码超出总页数时要回到最后一页**：在"只有一页多"的画廊里多选删除后，
  /// 原来的页码可能已经不存在了 —— 后端对越界页码返回空数组，界面就会显示
  /// "画廊还是空的"，其实库里还有东西，刷新一下才恢复（bug 清单第 2 条）。
  /// 这里多发一次请求把它纠正过来，用户感觉不到。
  Future<void> refreshMedia() async {
    loadingMedia = true;
    mediaError = null;
    _safeNotify();
    try {
      var result = await _listMedia(page: mediaPage);
      if (result.items.isEmpty && result.total > 0 && mediaPage > result.pages) {
        mediaPage = result.pages < 1 ? 1 : result.pages;
        result = await _listMedia(page: mediaPage);
      }
      media = result;
    } on ApiException catch (e) {
      mediaError = e.toString();
      media = Paged.empty<MediaAsset>();
    } catch (e) {
      mediaError = '$e';
      media = Paged.empty<MediaAsset>();
    } finally {
      loadingMedia = false;
      _safeNotify();
    }
  }

  Future<Paged<MediaAsset>> _listMedia({required int page}) => api.listMedia(
        q: mediaQuery.isEmpty ? null : mediaQuery,
        tags: mediaTags.toList(),
        tagMode: mediaTagMode,
        kind: mediaKind,
        favorite: mediaOnlyFavorite ? true : null,
        untagged: mediaUntagged,
        sort: mediaSort,
        page: page,
        size: _settings.pageSize,
      );

  Future<void> refreshTags() async {
    loadingTags = true;
    tagsError = null;
    _safeNotify();
    try {
      final results = await Future.wait([
        api.listTags(sort: 'popular'),
        api.tagCategories(),
      ]);
      tags = results[0] as List<Tag>;
      tagCategories = results[1] as List<String>;
    } on ApiException catch (e) {
      tagsError = e.toString();
    } catch (e) {
      tagsError = '$e';
    } finally {
      loadingTags = false;
      _safeNotify();
    }
  }

  // -------------------------------------------------------------------------
  //  提示词筛选
  // -------------------------------------------------------------------------

  void setPromptQuery(String value) {
    promptQuery = value;
    promptPage = 1;
    refreshPrompts();
  }

  void toggleTag(String name) {
    if (selectedTags.contains(name)) {
      selectedTags.remove(name);
    } else {
      selectedTags.add(name);
    }
    promptPage = 1;
    refreshPrompts();
  }

  void clearTags() {
    selectedTags.clear();
    promptPage = 1;
    refreshPrompts();
  }

  void setTagMode(String mode) {
    tagMode = mode;
    promptPage = 1;
    refreshPrompts();
  }

  void setPromptKind(String? kind) {
    promptKind = kind;
    promptPage = 1;
    refreshPrompts();
  }

  void setOnlyFavorite(bool value) {
    onlyFavorite = value;
    promptPage = 1;
    refreshPrompts();
  }

  void setOnlyUnlinked(bool value) {
    onlyUnlinked = value;
    promptPage = 1;
    refreshPrompts();
  }

  void setPromptSort(String sort) {
    promptSort = sort;
    promptPage = 1;
    refreshPrompts();
  }

  void setPromptPage(int page) {
    promptPage = page;
    refreshPrompts();
  }

  bool get hasPromptFilter =>
      promptQuery.isNotEmpty ||
      selectedTags.isNotEmpty ||
      promptKind != null ||
      onlyFavorite ||
      onlyUnlinked;

  // -------------------------------------------------------------------------
  //  媒体筛选
  // -------------------------------------------------------------------------

  void setMediaQuery(String value) {
    mediaQuery = value;
    mediaPage = 1;
    refreshMedia();
  }

  void toggleMediaTag(String name) {
    if (mediaTags.contains(name)) {
      mediaTags.remove(name);
    } else {
      mediaTags.add(name);
    }
    mediaPage = 1;
    refreshMedia();
  }

  void clearMediaTags() {
    mediaTags.clear();
    mediaPage = 1;
    refreshMedia();
  }

  void setMediaTagMode(String mode) {
    mediaTagMode = mode;
    mediaPage = 1;
    refreshMedia();
  }

  void setMediaKind(String? kind) {
    mediaKind = kind;
    mediaPage = 1;
    refreshMedia();
  }

  void setMediaOnlyFavorite(bool value) {
    mediaOnlyFavorite = value;
    mediaPage = 1;
    refreshMedia();
  }

  void setMediaUntagged(bool value) {
    mediaUntagged = value;
    mediaPage = 1;
    refreshMedia();
  }

  void setMediaSort(String sort) {
    mediaSort = sort;
    mediaPage = 1;
    refreshMedia();
  }

  void setMediaPage(int page) {
    mediaPage = page;
    refreshMedia();
  }

  bool get hasMediaFilter =>
      mediaQuery.isNotEmpty ||
      mediaTags.isNotEmpty ||
      mediaKind != null ||
      mediaOnlyFavorite ||
      mediaUntagged;

  // -------------------------------------------------------------------------
  //  变更
  // -------------------------------------------------------------------------

  Future<void> deletePrompt(int id) async {
    await api.deletePrompt(id);
    await Future.wait([refreshPrompts(), refreshTags(), refreshStats()]);
  }

  // 批量操作走的是后端已有的单条接口（本地库，几条到几十条的循环开销可以忽略），
  // 好处是 App 和后端不需要为"批量"再各维护一套语义。

  Future<void> setPromptsFavorite(Iterable<int> ids, bool favorite) async {
    for (final id in ids) {
      await api.setPromptFavorite(id, favorite);
    }
    await Future.wait([refreshPrompts(), refreshStats()]);
  }

  Future<void> addTagsToPrompts(Iterable<int> ids, List<String> tags) async {
    for (final id in ids) {
      await api.addPromptTags(id, tags);
    }
    await Future.wait([refreshPrompts(), refreshTags(), refreshStats()]);
  }

  Future<void> deletePrompts(Iterable<int> ids) async {
    for (final id in ids) {
      await api.deletePrompt(id);
    }
    await Future.wait([refreshPrompts(), refreshTags(), refreshStats()]);
  }

  /// 批量删除未关联产物的提示词，返回实际删掉的条数。
  ///
  /// 循环按批拉取再删：**不能边删边翻页**（页码会因为前面的记录被删而整体前移，
  /// 漏掉一部分）；每次都重新取第一页，直到取不到为止。
  /// 后端单页上限 200，所以超出部分靠这个循环兜住。
  Future<int> deleteUnlinkedPrompts({int max = 1000}) async {
    var deleted = 0;
    while (deleted < max) {
      final page = await api.listPrompts(hasMedia: false, page: 1, size: 200);
      if (page.items.isEmpty) break;
      for (final p in page.items) {
        await api.deletePrompt(p.id);
        deleted++;
      }
    }
    await Future.wait([refreshPrompts(), refreshTags(), refreshStats()]);
    return deleted;
  }

  Future<void> deleteMedia(int id) async {
    await api.deleteMedia(id);
    await Future.wait([refreshMedia(), refreshTags(), refreshStats()]);
  }

  Future<void> togglePromptFavorite(Prompt p) async {
    await api.setPromptFavorite(p.id, !p.favorite);
    await Future.wait([refreshPrompts(), refreshStats()]);
  }

  Future<void> toggleMediaFavorite(MediaAsset m) async {
    await api.updateMedia(m.id, favorite: !m.favorite);
    await refreshMedia();
  }
}
