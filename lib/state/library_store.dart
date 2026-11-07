import 'dart:async';

import 'package:flutter/foundation.dart';

import '../core/api_client.dart';
import '../core/settings_store.dart';
import '../models/models.dart';

/// 全局数据仓库：提示词库 / 媒体画廊 / 标签词表 的搜索状态与缓存。
class LibraryStore extends ChangeNotifier {
  /// [api] 只用于测试时注入假的 HTTP 客户端；生产代码传 null 即可。
  LibraryStore(this._settings, {ApiClient? api}) {
    _apiInjected = api != null;
    this.api = api ?? _libraryClient(_settings.baseUrl);
    _apiBaseUrl = _settings.baseUrl;
    _settings.addListener(_onSettingsChanged);
  }

  /// 库这一侧（画廊 / 提示词 / 标签）的 HTTP 客户端。
  ///
  /// 令牌**不用在这里接**：`ApiClient` 默认就去读进程内那一份配对令牌
  /// （[PairingTokenCache]）。手机连桌面时它是真值，桌面侧恒为 `null` ⇒ 桌面行为不变。
  static ApiClient _libraryClient(String baseUrl) => ApiClient(baseUrl);

  final SettingsStore _settings;

  /// 这一份 store 用的设置对象（**同一个实例**，不是新造的）。
  ///
  /// 给测试夹具用：界面上有些组件（筛选栏的折叠状态）从 `context` 读 `SettingsStore`，
  /// 而测试是手工搭 `Provider` 的 —— 有了它就不用把 `SettingsStore` 再穿一层参数
  /// （穿错一次就变成"测试里改了设置、界面读的是另一份"，那种 bug 很难看出来）。
  SettingsStore get settings => _settings;

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

  /// 请求代次：快速连点翻页时只让**最新**那次请求改状态。
  int _promptSeq = 0;

  /// 筛选 / 排序代次：筛选变了以后，旧的预取结果一律不认。
  int _promptGen = 0;

  /// 相邻页缓存（翻页时要在拖动过程中同时画出前后两页）。
  final Map<int, Paged<Prompt>> _promptPages = <int, Paged<Prompt>>{};
  final Map<int, Future<void>> _promptPrefetching = <int, Future<void>>{};

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

  /// 请求代次：快速连点翻页时只让**最新**那次请求改状态。
  int _mediaSeq = 0;

  /// 筛选 / 排序代次：筛选变了以后，旧的预取结果一律不认。
  int _mediaGen = 0;

  /// 相邻页缓存（翻页时要在拖动过程中同时画出前后两页）。
  final Map<int, Paged<MediaAsset>> _mediaPages = <int, Paged<MediaAsset>>{};
  final Map<int, Future<void>> _mediaPrefetching = <int, Future<void>>{};

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
      api = _libraryClient(_settings.baseUrl);
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
    // 整库刷新通常意味着"外面有东西变了"（上传 / 删除 / 关联）：相邻页缓存
    // 可能已经过期，别再拿它去拼翻页动画。
    _invalidateMediaPages();
    _invalidatePromptPages();
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
    final seq = ++_promptSeq;
    final gen = _promptGen;
    final requested = promptPage;
    loadingPrompts = true;
    promptsError = null;
    _safeNotify();
    try {
      var result = await _listPrompts(page: requested);
      if (seq != _promptSeq || gen != _promptGen) return;
      if (result.items.isEmpty &&
          result.total > 0 &&
          requested > result.pages) {
        promptPage = result.pages < 1 ? 1 : result.pages;
        result = await _listPrompts(page: promptPage);
        if (seq != _promptSeq || gen != _promptGen) return;
      }
      prompts = result;
      _promptPages[result.page] = result;
      _prefetchPromptNeighbours(result.page);
    } on ApiException catch (e) {
      if (seq != _promptSeq || gen != _promptGen) return;
      promptsError = e.toString();
      prompts = Paged.empty<Prompt>();
    } catch (e) {
      if (seq != _promptSeq || gen != _promptGen) return;
      promptsError = '$e';
      prompts = Paged.empty<Prompt>();
    } finally {
      // 快速连点翻页时会有多个请求在飞：只有**最新那一个**能收尾，
      // 否则旧请求回来会把 loading 提前关掉、把页码改回旧页。
      if (seq == _promptSeq) {
        loadingPrompts = false;
        _safeNotify();
      }
    }
  }

  /// 第 [page] 页的缓存（当前页永远以 [prompts] 为准）。
  Paged<Prompt>? promptPageData(int page) =>
      prompts.page == page ? prompts : _promptPages[page];

  /// 预取某一页（翻页动画需要同时画出前后两页）。
  ///
  /// 失败**不报错**：真正翻到那一页时 [refreshPrompts] 会再试，并照常把错误
  /// 显示在界面上；这里只是"提前拿一份"。
  Future<void> ensurePromptPage(int page) {
    if (page < 1) return Future<void>.value();
    final known = prompts.pages;
    if (known > 0 && page > known) return Future<void>.value();
    if (promptPageData(page) != null) return Future<void>.value();
    return _promptPrefetching[page] ??= _loadPromptPage(page);
  }

  Future<void> _loadPromptPage(int page) async {
    final gen = _promptGen;
    try {
      final result = await _listPrompts(page: page);
      if (_disposed || gen != _promptGen) return;
      _promptPages[page] = result;
      _safeNotify();
    } catch (_) {
      // 预取是 best-effort，见 [ensurePromptPage]
    } finally {
      _promptPrefetching.remove(page);
    }
  }

  void _prefetchPromptNeighbours(int page) {
    if (page > 1) unawaited(ensurePromptPage(page - 1));
    if (page < prompts.pages) unawaited(ensurePromptPage(page + 1));
  }

  void _invalidatePromptPages() {
    _promptGen++;
    _promptPages.clear();
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
    final seq = ++_mediaSeq;
    final gen = _mediaGen;
    final requested = mediaPage;
    loadingMedia = true;
    mediaError = null;
    _safeNotify();
    try {
      var result = await _listMedia(page: requested);
      if (seq != _mediaSeq || gen != _mediaGen) return;
      if (result.items.isEmpty &&
          result.total > 0 &&
          requested > result.pages) {
        mediaPage = result.pages < 1 ? 1 : result.pages;
        result = await _listMedia(page: mediaPage);
        if (seq != _mediaSeq || gen != _mediaGen) return;
      }
      media = result;
      _mediaPages[result.page] = result;
      _prefetchMediaNeighbours(result.page);
    } on ApiException catch (e) {
      if (seq != _mediaSeq || gen != _mediaGen) return;
      mediaError = e.toString();
      media = Paged.empty<MediaAsset>();
    } catch (e) {
      if (seq != _mediaSeq || gen != _mediaGen) return;
      mediaError = '$e';
      media = Paged.empty<MediaAsset>();
    } finally {
      if (seq == _mediaSeq) {
        loadingMedia = false;
        _safeNotify();
      }
    }
  }

  /// 第 [page] 页的缓存（当前页永远以 [media] 为准）。
  Paged<MediaAsset>? mediaPageData(int page) =>
      media.page == page ? media : _mediaPages[page];

  /// 预取某一页（翻页动画需要同时画出前后两页）。
  ///
  /// 失败**不报错**：真正翻到那一页时 [refreshMedia] 会再试，并照常把错误
  /// 显示在界面上；这里只是"提前拿一份"。
  Future<void> ensureMediaPage(int page) {
    if (page < 1) return Future<void>.value();
    final known = media.pages;
    if (known > 0 && page > known) return Future<void>.value();
    if (mediaPageData(page) != null) return Future<void>.value();
    return _mediaPrefetching[page] ??= _loadMediaPage(page);
  }

  Future<void> _loadMediaPage(int page) async {
    final gen = _mediaGen;
    try {
      final result = await _listMedia(page: page);
      if (_disposed || gen != _mediaGen) return;
      _mediaPages[page] = result;
      _safeNotify();
    } catch (_) {
      // 预取是 best-effort，见 [ensureMediaPage]
    } finally {
      _mediaPrefetching.remove(page);
    }
  }

  void _prefetchMediaNeighbours(int page) {
    if (page > 1) unawaited(ensureMediaPage(page - 1));
    if (page < media.pages) unawaited(ensureMediaPage(page + 1));
  }

  void _invalidateMediaPages() {
    _mediaGen++;
    _mediaPages.clear();
  }

  /// 「仅收藏」时画廊**无视集合**：每张被收藏的图按单张列出（不折叠、也不展开成组）。
  ///
  /// 为什么必须这样（用户 2026-10-24）：折叠视图里一个集合只留**最早那张**当代表。
  /// 收藏的若是别的成员，它就永远不出现在「仅收藏」里；代表本身没被收藏时整组直接消失。
  /// 所以这一档请求 `collapse=0`（见 [_listMedia]），界面也把 `isCollection` 的格子
  /// 当单图渲染（`gallery_page.dart` 里所有 `isCollection` 判据都过这个开关）。
  bool get mediaFavoritesFlat => mediaOnlyFavorite;

  Future<Paged<MediaAsset>> _listMedia({required int page}) => api.listMedia(
    q: mediaQuery.isEmpty ? null : mediaQuery,
    tags: mediaTags.toList(),
    tagMode: mediaTagMode,
    kind: mediaKind,
    favorite: mediaOnlyFavorite ? true : null,
    untagged: mediaUntagged,
    sort: mediaSort,
    // 画廊默认**折叠集合**：同一批微调的产物显示成一个格子（叠放封面 + 角标）。
    // 详情页/工具要的是"这一个集合的全部成员"，那时传 collectionId 自己列（不走折叠）。
    //
    // ⚠ 但「仅收藏」时**不许折叠**：折叠只认每个集合的代表（最早那张），
    // 代表没被收藏就整组消失、收藏了非代表的那张也永远看不到 ——
    // 这一档用户要看的是"我收藏过的那几张图"，见 [mediaFavoritesFlat]。
    collapse: !mediaFavoritesFlat,
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
    _invalidatePromptPages();
    refreshPrompts();
  }

  void toggleTag(String name) {
    if (selectedTags.contains(name)) {
      selectedTags.remove(name);
    } else {
      selectedTags.add(name);
    }
    promptPage = 1;
    _invalidatePromptPages();
    refreshPrompts();
  }

  void clearTags() {
    selectedTags.clear();
    promptPage = 1;
    _invalidatePromptPages();
    refreshPrompts();
  }

  void setTagMode(String mode) {
    tagMode = mode;
    promptPage = 1;
    _invalidatePromptPages();
    refreshPrompts();
  }

  void setPromptKind(String? kind) {
    promptKind = kind;
    promptPage = 1;
    _invalidatePromptPages();
    refreshPrompts();
  }

  void setOnlyFavorite(bool value) {
    onlyFavorite = value;
    promptPage = 1;
    _invalidatePromptPages();
    refreshPrompts();
  }

  void setOnlyUnlinked(bool value) {
    onlyUnlinked = value;
    promptPage = 1;
    _invalidatePromptPages();
    refreshPrompts();
  }

  void setPromptSort(String sort) {
    promptSort = sort;
    promptPage = 1;
    _invalidatePromptPages();
    refreshPrompts();
  }

  void setPromptPage(int page) {
    promptPage = page;
    final cached = _promptPages[page];
    if (cached != null) {
      // 相邻页已经预取好了：**立刻**把内容换过去，翻页动画不用等网络。
      // 顺带作废还在飞的那次请求（否则它回来会把页码/内容改回旧的一页）。
      _promptSeq++;
      promptsError = null;
      loadingPrompts = false;
      prompts = cached;
      _safeNotify();
      _prefetchPromptNeighbours(page);
      return;
    }
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
    _invalidateMediaPages();
    refreshMedia();
  }

  void toggleMediaTag(String name) {
    if (mediaTags.contains(name)) {
      mediaTags.remove(name);
    } else {
      mediaTags.add(name);
    }
    mediaPage = 1;
    _invalidateMediaPages();
    refreshMedia();
  }

  void clearMediaTags() {
    mediaTags.clear();
    mediaPage = 1;
    _invalidateMediaPages();
    refreshMedia();
  }

  void setMediaTagMode(String mode) {
    mediaTagMode = mode;
    mediaPage = 1;
    _invalidateMediaPages();
    refreshMedia();
  }

  void setMediaKind(String? kind) {
    mediaKind = kind;
    mediaPage = 1;
    _invalidateMediaPages();
    refreshMedia();
  }

  void setMediaOnlyFavorite(bool value) {
    mediaOnlyFavorite = value;
    mediaPage = 1;
    _invalidateMediaPages();
    refreshMedia();
  }

  void setMediaUntagged(bool value) {
    mediaUntagged = value;
    mediaPage = 1;
    _invalidateMediaPages();
    refreshMedia();
  }

  void setMediaSort(String sort) {
    mediaSort = sort;
    mediaPage = 1;
    _invalidateMediaPages();
    refreshMedia();
  }

  void setMediaPage(int page) {
    mediaPage = page;
    final cached = _mediaPages[page];
    if (cached != null) {
      // 相邻页已经预取好了：**立刻**把内容换过去，翻页动画不用等网络。
      // 顺带作废还在飞的那次请求（否则它回来会把页码/内容改回旧的一页）。
      _mediaSeq++;
      mediaError = null;
      loadingMedia = false;
      media = cached;
      _safeNotify();
      _prefetchMediaNeighbours(page);
      return;
    }
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
    _invalidatePromptPages();
    await Future.wait([refreshPrompts(), refreshTags(), refreshStats()]);
  }

  // 批量操作走的是后端已有的单条接口（本地库，几条到几十条的循环开销可以忽略），
  // 好处是 App 和后端不需要为"批量"再各维护一套语义。

  Future<void> setPromptsFavorite(Iterable<int> ids, bool favorite) async {
    for (final id in ids) {
      await api.setPromptFavorite(id, favorite);
    }
    _invalidatePromptPages();
    await Future.wait([refreshPrompts(), refreshStats()]);
  }

  Future<void> addTagsToPrompts(Iterable<int> ids, List<String> tags) async {
    for (final id in ids) {
      await api.addPromptTags(id, tags);
    }
    _invalidatePromptPages();
    await Future.wait([refreshPrompts(), refreshTags(), refreshStats()]);
  }

  Future<void> deletePrompts(Iterable<int> ids) async {
    for (final id in ids) {
      await api.deletePrompt(id);
    }
    _invalidatePromptPages();
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
    _invalidatePromptPages();
    await Future.wait([refreshPrompts(), refreshTags(), refreshStats()]);
    return deleted;
  }

  Future<void> deleteMedia(int id) async {
    await api.deleteMedia(id);
    _invalidateMediaPages();
    await Future.wait([refreshMedia(), refreshTags(), refreshStats()]);
  }

  Future<void> togglePromptFavorite(Prompt p) async {
    await api.setPromptFavorite(p.id, !p.favorite);
    _invalidatePromptPages();
    await Future.wait([refreshPrompts(), refreshStats()]);
  }

  Future<void> toggleMediaFavorite(MediaAsset m) async {
    await api.updateMedia(m.id, favorite: !m.favorite);
    _invalidateMediaPages();
    await refreshMedia();
  }
}
