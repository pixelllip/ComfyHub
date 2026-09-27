/// 「AI 文案台」：人工校验 + 批量修改规则类文案与界面注解。
///
/// 唯一设计真源：`docs/copy-review-design.md`（§3 端点 / §4 批量语义 / §6 界面形态）。
///
/// 这一页只做三件事，而它们的顺序就是用户的心智：
///  ① **看**：默认值（真源）与当前值摆在一起，一眼看出"这条被改过"；
///  ② **校验**：读过就标记一下，之后上游改了默认值会自动变回"待重看"；
///  ③ **批量**：勾多条 → 选一种操作 → **先看 diff** → 再应用。
///
/// 界面纪律（AGENTS §6）：列表用 `ListView.builder` 懒构建；下拉菜单一律走
/// `AppMenuButton`（`PopupMenuButton` 的 `ModalBarrier` 会把整页滚动吃掉）；
/// 384×853 的真机尺寸下不许溢出 —— 所以窄屏是"列表 / 详情"两块整屏切换，
/// 宽屏才左右分栏，按钮组一律 `Wrap`。
library;

import 'dart:convert';
import 'dart:typed_data';

import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/copy_api.dart';
import '../core/settings_store.dart';
import '../widgets/app_menu.dart';

/// 一条文案的三态（设计 §6）。
enum CopyReviewState {
  /// 还没人读过。
  unverified,

  /// 有人读过并确认了，且之后默认值没变过。
  verified,

  /// 上游改过默认值 —— **压过"已校验"**（设计 §1 纪律 2：你校验的是旧那一版）。
  baseChanged,
}

/// 三态判据。**默认值变过要压过已校验**，顺序不能反：
/// 反了就会出现"默认值已经改了、界面还说已校验"，而这正是这一页要防的那件事。
CopyReviewState copyReviewStateOf(CopyEntry entry) {
  if (entry.baseChanged) return CopyReviewState.baseChanged;
  final at = entry.verifiedAt;
  return (at != null && at.isNotEmpty) ? CopyReviewState.verified : CopyReviewState.unverified;
}

String copyReviewStateLabel(CopyReviewState state) => switch (state) {
      CopyReviewState.unverified => '未校验',
      CopyReviewState.verified => '已校验',
      CopyReviewState.baseChanged => '默认值变过，请重看',
    };

/// scope → 界面上那一组叫什么（设计 §6 的四个分组）。
String copyScopeLabel(String scope) => switch (scope) {
      'prompt' => '系统提示词',
      'tool' => '工具描述',
      'skill' => 'Skill 正文',
      'ui' => '界面注解',
      '' => '其他',
      _ => scope,
    };

String copyBatchOpLabel(CopyBatchOp op) => switch (op) {
      CopyBatchOp.replace => '替换文本',
      CopyBatchOp.regexReplace => '正则替换',
      CopyBatchOp.append => '末尾追加',
      CopyBatchOp.prepend => '开头插入',
      CopyBatchOp.deleteLines => '删掉含该文本的整行',
      CopyBatchOp.insertAfter => '在该行之后插入',
    };

String copyBatchOpHint(CopyBatchOp op) => switch (op) {
      CopyBatchOp.replace => '把 find 的每一处换成 replace',
      CopyBatchOp.regexReplace => '正则（默认不是多行，要 (?m) 自己写）',
      CopyBatchOp.append => '在正文末尾追加 replace',
      CopyBatchOp.prepend => '在正文开头插入 replace',
      CopyBatchOp.deleteLines => '含 find 的整行删掉',
      CopyBatchOp.insertAfter => '在含 find 的那一行后面插入 replace',
    };

/// 解析导入的覆盖层 JSON。宽容三种形状（导出原样 / 数组 / 裸映射）：
///
/// ```jsonc
/// { "overrides": [ { "key": "...", "text": "...", "note": "..." } ] }
/// [ { "key": "...", "text": "..." } ]
/// { "tool.x.description": "新文本" }
/// ```
///
/// 认不出来就抛 [FormatException]（带人话），**绝不当成空清单** ——
/// 那样用户会以为"导进去了但什么都没变"。
List<({String key, String text, String? note})> parseCopyOverridesJson(String raw) {
  dynamic decoded;
  try {
    decoded = jsonDecode(raw);
  } catch (_) {
    throw const FormatException('这不是合法的 JSON。');
  }

  final out = <({String key, String text, String? note})>[];
  void add(dynamic key, dynamic text, dynamic note) {
    final k = key is String ? key.trim() : '';
    if (k.isEmpty || text == null) return;
    out.add((key: k, text: text is String ? text : text.toString(), note: note is String ? note : null));
  }

  if (decoded is Map) {
    final list = decoded['overrides'];
    if (list is List) {
      for (final it in list) {
        if (it is Map) add(it['key'], it['text'], it['note']);
      }
    } else {
      decoded.forEach((k, v) => add(k, v, null));
    }
  } else if (decoded is List) {
    for (final it in decoded) {
      if (it is Map) add(it['key'], it['text'], it['note']);
    }
  } else {
    throw const FormatException('这份 JSON 既不是覆盖层对象也不是数组。');
  }
  return out;
}

/// 「AI 文案台」（设置页 → 「AI 文案台」）。
class CopyReviewPage extends StatefulWidget {
  /// [api] 仅供测试注入假后端；正式运行时按设置里的后端地址创建。
  final CopyApi? api;

  /// 导出通道（默认弹系统"另存为"）。测试注入后就能在用例里断言导出的 JSON。
  final Future<String?> Function(String fileName, String json)? saveJson;

  /// 导入通道（默认弹系统"打开文件"，返回文件正文；`null` = 用户取消）。
  final Future<String?> Function()? pickJson;

  const CopyReviewPage({super.key, this.api, this.saveJson, this.pickJson});

  @override
  State<CopyReviewPage> createState() => _CopyReviewPageState();
}

class _CopyReviewPageState extends State<CopyReviewPage> {
  CopyApi? _api;

  List<CopyEntry> _entries = const [];
  bool _loading = true;
  String? _error;

  // 筛选：scope 与"只看待校验"交给后端（`?scope=…&verified=0`），
  // 关键字与"只看我改过的"在本地过 —— 关键字每敲一个字打一次后端没有意义
  // （清单本来就在内存里），而"要哪些条"的问题交给后端就不用把整份清单搬过来。
  String _scope = '';
  final TextEditingController _search = TextEditingController();
  bool _onlyUnverified = false;
  bool _onlyOverridden = false;

  /// 批量勾选（key）。
  final Set<String> _selected = {};

  // 详情
  CopyEntry? _current;
  bool _detailLoading = false;
  String? _detailError;
  bool _detailBusy = false;
  final TextEditingController _edit = TextEditingController();
  final TextEditingController _note = TextEditingController();

  /// 窄屏（< 900）一次只显示一块：列表 or 详情。宽屏用不上它。
  bool _showDetail = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      _api = widget.api ??
          CopyApi(context.read<SettingsStore?>()?.baseUrl ?? SettingsStore.defaultBaseUrl());
      _reload();
    });
  }

  @override
  void dispose() {
    // 注入进来的客户端由调用方负责；自己建的那个自己关（同 AiProviderSettingsPage 的约定）。
    if (widget.api == null) _api?.dispose();
    _search.dispose();
    _edit.dispose();
    _note.dispose();
    super.dispose();
  }

  // -------------------------------------------------------------------------
  //  数据
  // -------------------------------------------------------------------------

  Future<void> _reload() async {
    final api = _api;
    if (api == null) return;
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final entries = await api.catalog(
        scope: _scope.isEmpty ? null : _scope,
        verifiedOnly: _onlyUnverified ? false : null,
      );
      if (!mounted) return;
      final keys = entries.map((e) => e.key).toSet();
      setState(() {
        _entries = entries;
        _loading = false;
        // 勾选里可能已经有"这一版清单里没有的 key"（换筛选 / 别处删了）——
        // 留着它，批量面板就会拿一条看不见的 key 去改，所以清掉。
        _selected.removeWhere((k) => !keys.contains(k));
        // 详情里那条用新数据（默认值可能刚被上游改过），但**不动编辑框**：
        // 用户可能正在改字，刷新清单不该把没保存的内容吞掉。
        final k = _current?.key;
        if (k != null) _current = entries.where((e) => e.key == k).firstOrNull ?? _current;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _loading = false;
        _error = '$e';
      });
    }
  }

  /// 打开一条详情。先用清单里那份**立刻画出来**，再去拉详情端点覆盖 ——
  /// 详情拉不到要如实说（写在详情顶部），但**不擦掉**已经有的默认值/当前值。
  Future<void> _openDetail(CopyEntry entry) async {
    setState(() {
      _current = entry;
      _showDetail = true;
      _detailError = null;
      _edit.text = entry.currentText ?? '';
      _note.text = entry.note ?? '';
    });
    final api = _api;
    if (api == null) return;
    setState(() => _detailLoading = true);
    try {
      final fresh = await api.detail(entry.key);
      if (!mounted) return;
      setState(() {
        _current = fresh;
        _detailLoading = false;
        _edit.text = fresh.currentText ?? '';
        _note.text = fresh.note ?? '';
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _detailLoading = false;
        _detailError = '读取详情失败：$e';
      });
    }
  }

  /// 本地过的那两个筛子（见 [_scope] 上方注释）。
  List<CopyEntry> get _visibleEntries {
    final kw = _search.text.trim().toLowerCase();
    return _entries.where((e) {
      if (_onlyOverridden && !e.overridden) return false;
      if (kw.isEmpty) return true;
      return [
        e.key,
        e.title ?? '',
        e.currentText ?? '',
        e.defaultText ?? '',
      ].any((s) => s.toLowerCase().contains(kw));
    }).toList();
  }

  // -------------------------------------------------------------------------
  //  写操作（单条）
  // -------------------------------------------------------------------------

  void _snack(String message, {bool error = false}) {
    if (!mounted) return;
    if (error) setState(() => _error = message);
    ScaffoldMessenger.maybeOf(context)?.showSnackBar(SnackBar(content: Text(message)));
  }

  Future<void> _saveCurrent() async {
    final entry = _current;
    final api = _api;
    if (entry == null || api == null) return;
    setState(() => _detailBusy = true);
    try {
      final note = _note.text.trim();
      await api.put(entry.key, _edit.text, note: note.isEmpty ? null : note);
      if (!mounted) return;
      await _openDetail(entry);
      await _reload();
      _snack('已保存：${entry.key}（校验状态会被清空，需要重新标记）');
    } catch (e) {
      if (mounted) _snack('保存失败：$e', error: true);
    } finally {
      if (mounted) setState(() => _detailBusy = false);
    }
  }

  /// 还原默认 = 删掉覆盖层那一行，**用户改的文字就没了** —— 所以先问一次。
  Future<void> _resetCurrent() async {
    final entry = _current;
    final api = _api;
    if (entry == null || api == null) return;
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('还原默认？'),
        content: Text('会删掉 ${entry.key} 的覆盖层那一行，你改的那段文字就没了（默认值不受影响）。'),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('取消')),
          FilledButton(onPressed: () => Navigator.pop(ctx, true), child: const Text('还原默认')),
        ],
      ),
    );
    if (ok != true || !mounted) return;
    setState(() => _detailBusy = true);
    try {
      await api.reset(entry.key);
      if (!mounted) return;
      await _openDetail(entry);
      await _reload();
      _snack('已还原默认：${entry.key}');
    } catch (e) {
      if (mounted) _snack('还原失败：$e', error: true);
    } finally {
      if (mounted) setState(() => _detailBusy = false);
    }
  }

  Future<void> _verifyCurrent() async {
    final entry = _current;
    final api = _api;
    if (entry == null || api == null) return;
    setState(() => _detailBusy = true);
    try {
      final note = _note.text.trim();
      await api.verify(entry.key, note: note.isEmpty ? null : note);
      if (!mounted) return;
      await _openDetail(entry);
      await _reload();
      _snack('已标记校验：${entry.key}');
    } catch (e) {
      if (mounted) _snack('标记校验失败：$e', error: true);
    } finally {
      if (mounted) setState(() => _detailBusy = false);
    }
  }

  // -------------------------------------------------------------------------
  //  导出 / 导入（设计 §6 最后一行）
  // -------------------------------------------------------------------------

  Future<void> _export() async {
    final overridden = _entries.where((e) => e.overridden).toList();
    final payload = {
      'kind': 'comfyhub.copy-overrides',
      'version': 1,
      'exportedAt': DateTime.now().toUtc().toIso8601String(),
      'overrides': [
        for (final e in overridden)
          {'key': e.key, 'text': e.currentText ?? '', 'note': ?e.note},
      ],
    };
    final json = const JsonEncoder.withIndent('  ').convert(payload);
    final saver = widget.saveJson ?? _saveJsonViaPicker;
    try {
      final path = await saver('comfyhub-copy-overrides.json', json);
      if (!mounted) return;
      if (path == null) {
        _snack('已取消导出。');
        return;
      }
      _snack('已导出 ${overridden.length} 条覆盖到 $path');
    } catch (e) {
      _snack('导出失败：$e', error: true);
    }
  }

  Future<String?> _saveJsonViaPicker(String fileName, String json) async {
    final uri = await FilePicker.saveFile(
      fileName: fileName,
      bytes: Uint8List.fromList(utf8.encode(json)),
      mimeType: 'application/json',
      dialogTitle: '导出文案覆盖层',
    );
    return uri?.toString();
  }

  Future<String?> _pickJsonViaPicker() async {
    final file = await FilePicker.pickFile(
      dialogTitle: '选择要导入的文案覆盖层 JSON',
      type: FileType.custom,
      allowedExtensions: const ['json'],
    );
    if (file == null) return null;
    return utf8.decode(await file.readAsBytes());
  }

  /// 导入 = **先 dryRun 校验、再落盘**（设计 §6）。
  ///
  /// ⚠ 形状上的一处对不上，如实记在这里：设计 §4 的批量端点只有**一种 op 作用于
  /// 所有 keys**，而"整份覆盖层"是**每条一个文本** —— 用六种 op 里任何一种都表达不了。
  /// 所以这里按"每条一次 dryRun"走（用 `replace` + `find=当前值` 问后端"换成这样会被机器
  /// 校验拦掉吗"），真正的写入仍走 `PUT`（那才是"整段替换"的语义）。
  /// 代价是 N 次预览请求；覆盖层是几十条的量级，可以接受。
  Future<void> _import() async {
    final reader = widget.pickJson ?? _pickJsonViaPicker;
    String? raw;
    try {
      raw = await reader();
    } catch (e) {
      _snack('读取文件失败：$e', error: true);
      return;
    }
    if (raw == null || raw.trim().isEmpty || !mounted) return;

    final List<({String key, String text, String? note})> items;
    try {
      items = parseCopyOverridesJson(raw);
    } on FormatException catch (e) {
      _snack('导入失败：${e.message}', error: true);
      return;
    }
    if (items.isEmpty) {
      _snack('这份 JSON 里没有可用的覆盖（要 {key, text} 形状）。', error: true);
      return;
    }

    final api = _api;
    if (api == null) return;
    final byKey = {for (final e in _entries) e.key: e};
    final rows = <_ImportRow>[];
    for (final item in items) {
      final entry = byKey[item.key];
      final current = entry?.currentText;
      if (entry == null) {
        rows.add(_ImportRow(item, skip: '清单里没有这条 key（先点右上角刷新清单）'));
        continue;
      }
      if (current == null) {
        rows.add(_ImportRow(item, skip: '清单没给这条的当前文本，没法预览'));
        continue;
      }
      if (current == item.text) {
        rows.add(_ImportRow(item, skip: '与当前值相同'));
        continue;
      }
      try {
        final res = await api.batch(
          keys: [item.key],
          op: CopyBatchOp.replace,
          find: current,
          replace: item.text,
          dryRun: true,
        );
        rows.add(_ImportRow(item, diff: res.items.isEmpty ? null : res.items.first));
      } catch (e) {
        rows.add(_ImportRow(item, blocked: '预览请求失败：$e'));
      }
      if (!mounted) return;
    }

    final go = await showDialog<bool>(
      context: context,
      builder: (ctx) => _ImportPreviewDialog(rows: rows),
    );
    if (go != true || !mounted) return;

    var ok = 0;
    final failures = <String>[];
    for (final row in rows) {
      if (!row.willApply) continue;
      try {
        await api.put(row.item.key, row.item.text, note: row.item.note);
        ok++;
      } catch (e) {
        failures.add('${row.item.key}：$e');
      }
    }
    if (!mounted) return;
    await _openDetailIfVisible();
    await _reload();
    final skipped = rows.where((r) => !r.willApply).length;
    _snack('导入完成：改成功 $ok 条，跳过 $skipped 条'
        '${failures.isEmpty ? '' : '，失败 ${failures.length} 条'}');
    if (failures.isNotEmpty) _snack('导入失败：${failures.join('；')}', error: true);
  }

  /// 导入之后详情里那条也可能是刚改过的，重新拉一次。
  Future<void> _openDetailIfVisible() async {
    final cur = _current;
    if (cur != null) await _openDetail(cur);
  }

  // -------------------------------------------------------------------------
  //  批量
  // -------------------------------------------------------------------------

  Future<void> _openBatch() async {
    final api = _api;
    if (api == null || _selected.isEmpty) return;
    final changed = await showDialog<int>(
      context: context,
      builder: (_) => _CopyBatchPanel(api: api, keys: _selected.toList()..sort()),
    );
    if (!mounted) return;
    if (changed == null) return; // 取消：一个字都没改，勾选也留着（用户可能只是想再看看）
    setState(_selected.clear);
    await _openDetailIfVisible();
    await _reload();
    _snack('批量修改完成：$changed 条发生变化（校验状态已被清空，需要重新看）');
  }

  // -------------------------------------------------------------------------
  //  build
  // -------------------------------------------------------------------------

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      appBar: AppBar(
        title: const Text('AI 文案台'),
        bottom: PreferredSize(
          preferredSize: const Size.fromHeight(1),
          child: Divider(height: 1, color: theme.dividerColor),
        ),
        actions: [
          IconButton(
            tooltip: '刷新清单',
            onPressed: _loading ? null : _reload,
            icon: const Icon(Icons.refresh),
          ),
          AppMenuButton<String>(
            options: const [
              MenuOption(value: 'export', label: '导出 JSON', subtitle: '只导出你改过的那几条', icon: Icons.download_outlined),
              MenuOption(value: 'import', label: '导入 JSON', subtitle: '先 dryRun 校验，再写入', icon: Icons.upload_outlined),
            ],
            onSelected: (v) => v == 'export' ? _export() : _import(),
            button: (ctx, controller, isOpen) => IconButton(
              tooltip: '导出 / 导入',
              onPressed: () => isOpen ? controller.close() : controller.open(),
              icon: const Icon(Icons.more_vert),
            ),
          ),
        ],
      ),
      body: LayoutBuilder(
        builder: (context, constraints) {
          final wide = constraints.maxWidth >= 900;
          if (!wide) {
            return _showDetail ? _detailView(theme, narrow: true) : _listView(theme);
          }
          return Row(
            children: [
              SizedBox(width: 400, child: _listView(theme)),
              const VerticalDivider(width: 1),
              Expanded(child: _detailView(theme, narrow: false)),
            ],
          );
        },
      ),
    );
  }

  // --- 左：清单 -----------------------------------------------------------

  Widget _listView(ThemeData theme) {
    final entries = _visibleEntries;
    final rows = _groupedRows(entries);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(padding: const EdgeInsets.fromLTRB(12, 12, 12, 6), child: _filters(theme)),
        if (_loading) const LinearProgressIndicator(minHeight: 2),
        if (_error != null)
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 4, 12, 4),
            child: _banner(theme, _error!, error: true),
          ),
        Expanded(
          child: rows.isEmpty
              ? Center(
                  child: Padding(
                    padding: const EdgeInsets.all(24),
                    child: Text(
                      _loading
                          ? '正在读文案清单…'
                          // "一条都没有"与"根本没读到"必须分开说：后者说成"没有匹配的"
                          // 会让用户以为清单是空的（而后端可能只是没起来）。
                          : (_error != null && _entries.isEmpty
                              ? '还没读到清单 —— 上面那条横幅就是原因。'
                              : '没有匹配的文案。换个筛选条件试试。'),
                      textAlign: TextAlign.center,
                      style: theme.textTheme.bodySmall,
                    ),
                  ),
                )
              // 长列表懒构建（AGENTS §6）：分组标题也是行，所以它们一起按需构建。
              : ListView.builder(
                  itemCount: rows.length,
                  itemBuilder: (context, i) => _rowWidget(theme, rows[i]),
                ),
        ),
        if (_selected.isNotEmpty) _batchBar(theme),
      ],
    );
  }

  Widget _filters(ThemeData theme) => Wrap(
        spacing: 8,
        runSpacing: 6,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          SizedBox(
            width: 210,
            child: TextField(
              controller: _search,
              onChanged: (_) => setState(() {}),
              decoration: InputDecoration(
                isDense: true,
                border: const OutlineInputBorder(),
                hintText: '搜 key / 标题 / 正文',
                prefixIcon: const Icon(Icons.search, size: 18),
                suffixIcon: _search.text.isEmpty
                    ? null
                    : IconButton(
                        tooltip: '清空搜索',
                        icon: const Icon(Icons.close, size: 16),
                        onPressed: () => setState(_search.clear),
                      ),
              ),
            ),
          ),
          AppMenuButton<String>(
            options: const [
              MenuOption(value: '', label: '全部范围'),
              MenuOption(value: 'prompt', label: '系统提示词'),
              MenuOption(value: 'tool', label: '工具描述'),
              MenuOption(value: 'skill', label: 'Skill 正文'),
              MenuOption(value: 'ui', label: '界面注解'),
            ],
            onSelected: (v) {
              setState(() => _scope = v);
              _reload();
            },
            button: (ctx, controller, isOpen) => OutlinedButton.icon(
              onPressed: () => isOpen ? controller.close() : controller.open(),
              icon: const Icon(Icons.filter_alt_outlined, size: 18),
              label: Text(_scope.isEmpty ? '全部范围' : copyScopeLabel(_scope)),
            ),
          ),
          FilterChip(
            label: const Text('只看待校验'),
            selected: _onlyUnverified,
            onSelected: (v) {
              setState(() => _onlyUnverified = v);
              _reload();
            },
          ),
          FilterChip(
            label: const Text('只看我改过的'),
            selected: _onlyOverridden,
            onSelected: (v) => setState(() => _onlyOverridden = v),
          ),
        ],
      );

  /// 按 scope 分组，扁平成"标题行 + 条目行"的序列（[ListView.builder] 直接吃它）。
  List<_CopyListRow> _groupedRows(List<CopyEntry> entries) {
    const order = ['prompt', 'tool', 'skill', 'ui'];
    final groups = <String, List<CopyEntry>>{};
    for (final e in entries) {
      groups.putIfAbsent(e.scopeOf, () => []).add(e);
    }
    int rank(String s) {
      final i = order.indexOf(s);
      return i < 0 ? order.length : i;
    }

    final scopes = groups.keys.toList()
      ..sort((a, b) => rank(a) != rank(b) ? rank(a).compareTo(rank(b)) : a.compareTo(b));
    return [
      for (final s in scopes) ...[
        _CopyListRow.header(s, groups[s]!.length),
        for (final e in groups[s]!) _CopyListRow.entry(e),
      ],
    ];
  }

  Widget _rowWidget(ThemeData theme, _CopyListRow row) {
    final entry = row.entry;
    if (entry == null) {
      return Padding(
        padding: const EdgeInsets.fromLTRB(16, 14, 16, 4),
        child: Row(
          children: [
            Text(copyScopeLabel(row.header!), style: theme.textTheme.titleSmall),
            const SizedBox(width: 6),
            Text('${row.count}', style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline)),
          ],
        ),
      );
    }
    final state = copyReviewStateOf(entry);
    return ListTile(
      selected: _current?.key == entry.key,
      leading: Checkbox(
        value: _selected.contains(entry.key),
        onChanged: (_) => setState(() {
          if (!_selected.remove(entry.key)) _selected.add(entry.key);
        }),
      ),
      title: Text(
        (entry.title != null && entry.title!.isNotEmpty) ? entry.title! : entry.key,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
      ),
      subtitle: Row(
        children: [
          Flexible(child: _CopyStateBadge(state: state)),
          const SizedBox(width: 6),
          Expanded(
            child: Text(
              entry.key,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
            ),
          ),
        ],
      ),
      trailing: const Icon(Icons.chevron_right, size: 18),
      onTap: () => _openDetail(entry),
    );
  }

  Widget _batchBar(ThemeData theme) => Material(
        color: theme.colorScheme.surfaceContainerHighest,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
          child: Row(
            children: [
              Expanded(
                child: Text('已选 ${_selected.length} 条', style: theme.textTheme.bodySmall),
              ),
              TextButton(onPressed: () => setState(_selected.clear), child: const Text('取消选择')),
              const SizedBox(width: 8),
              FilledButton.icon(
                onPressed: _openBatch,
                icon: const Icon(Icons.edit_note, size: 18),
                label: const Text('批量修改'),
              ),
            ],
          ),
        ),
      );

  // --- 右：详情 -----------------------------------------------------------

  Widget _detailView(ThemeData theme, {required bool narrow}) {
    final entry = _current;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (narrow)
          Padding(
            padding: const EdgeInsets.fromLTRB(4, 8, 12, 0),
            child: Row(
              children: [
                IconButton(
                  tooltip: '返回清单',
                  onPressed: () => setState(() => _showDetail = false),
                  icon: const Icon(Icons.arrow_back),
                ),
                Expanded(
                  child: Text('文案详情', style: theme.textTheme.titleSmall),
                ),
              ],
            ),
          ),
        if (_detailLoading) const LinearProgressIndicator(minHeight: 2),
        Expanded(
          child: entry == null
              ? Center(
                  child: Padding(
                    padding: const EdgeInsets.all(24),
                    child: Text(
                      '从左边挑一条文案：这里显示它的默认值（真源，只读）、当前值、以及它在哪被用。',
                      textAlign: TextAlign.center,
                      style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
                    ),
                  ),
                )
              : ListView(
                  padding: const EdgeInsets.fromLTRB(16, 12, 16, 32),
                  children: _detailChildren(theme, entry, narrow: narrow),
                ),
        ),
      ],
    );
  }

  List<Widget> _detailChildren(ThemeData theme, CopyEntry entry, {required bool narrow}) {
    final state = copyReviewStateOf(entry);
    final refs = entry.refs;
    return [
      Wrap(
        spacing: 8,
        runSpacing: 6,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          Text(
            (entry.title != null && entry.title!.isNotEmpty) ? entry.title! : entry.key,
            style: theme.textTheme.titleMedium,
          ),
          _CopyStateBadge(state: state),
        ],
      ),
      const SizedBox(height: 4),
      SelectableText(
        entry.key,
        style: theme.textTheme.labelSmall?.copyWith(
          color: theme.colorScheme.outline,
          fontFamily: 'monospace',
        ),
      ),
      if (_detailError != null) ...[
        const SizedBox(height: 8),
        _banner(theme, _detailError!, error: true),
      ],
      if (state == CopyReviewState.baseChanged) ...[
        const SizedBox(height: 8),
        _banner(
          theme,
          '默认值已经变过（上游改了真源）。你上次校验的是旧那一版 —— 请重看一遍再标记已校验。',
          error: false,
        ),
      ],
      if (refs.isNotEmpty) ...[
        const SizedBox(height: 12),
        Text('用在哪（点一下知道它在哪被用）', style: theme.textTheme.labelMedium),
        const SizedBox(height: 6),
        Card(
          margin: EdgeInsets.zero,
          child: Padding(
            padding: const EdgeInsets.all(10),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                for (final r in refs)
                  Padding(
                    padding: const EdgeInsets.symmetric(vertical: 2),
                    child: SelectableText(
                      r,
                      style: theme.textTheme.labelSmall?.copyWith(fontFamily: 'monospace'),
                    ),
                  ),
              ],
            ),
          ),
        ),
      ],
      const SizedBox(height: 14),
      _compare(theme, entry, sideBySide: !narrow),
      const SizedBox(height: 14),
      TextField(
        controller: _note,
        enabled: !_detailBusy,
        decoration: const InputDecoration(
          isDense: true,
          border: OutlineInputBorder(),
          labelText: '备注（为什么改 / 校验时想到了什么）',
        ),
      ),
      const SizedBox(height: 10),
      Wrap(
        spacing: 8,
        runSpacing: 6,
        children: [
          FilledButton.icon(
            onPressed: _detailBusy ? null : _saveCurrent,
            icon: const Icon(Icons.save_outlined, size: 18),
            label: const Text('保存覆盖'),
          ),
          OutlinedButton.icon(
            // 没改过就没有"还原"可言：按钮置灰比点了没反应清楚。
            onPressed: (_detailBusy || !entry.overridden) ? null : _resetCurrent,
            icon: const Icon(Icons.settings_backup_restore, size: 18),
            label: const Text('还原默认'),
          ),
          OutlinedButton.icon(
            onPressed: _detailBusy ? null : _verifyCurrent,
            icon: const Icon(Icons.verified_outlined, size: 18),
            label: const Text('标记已校验'),
          ),
        ],
      ),
      if (entry.verifiedAt != null && entry.verifiedAt!.isNotEmpty) ...[
        const SizedBox(height: 10),
        Text('上次校验：${entry.verifiedAt}', style: theme.textTheme.labelSmall),
      ],
      if (entry.note != null && entry.note!.isNotEmpty) ...[
        const SizedBox(height: 4),
        Text('备注：${entry.note}', style: theme.textTheme.labelSmall),
      ],
      if (entry.baseHash != null) ...[
        const SizedBox(height: 4),
        SelectableText(
          '改它时的默认值指纹：${entry.baseHash}',
          style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
        ),
      ],
    ];
  }

  Widget _compare(ThemeData theme, CopyEntry entry, {required bool sideBySide}) {
    final def = _readOnlyBox(theme, '默认值（真源，只读）', entry.defaultText);
    final cur = _editableBox(theme, entry);
    if (!sideBySide) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [def, const SizedBox(height: 12), cur],
      );
    }
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Expanded(child: def),
        const SizedBox(width: 12),
        Expanded(child: cur),
      ],
    );
  }

  Widget _readOnlyBox(ThemeData theme, String label, String? text) => Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(label, style: theme.textTheme.labelMedium),
          const SizedBox(height: 4),
          Container(
            width: double.infinity,
            constraints: const BoxConstraints(minHeight: 140, maxHeight: 320),
            padding: const EdgeInsets.all(10),
            decoration: BoxDecoration(
              color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.35),
              borderRadius: BorderRadius.circular(8),
              border: Border.all(color: theme.dividerColor),
            ),
            // 缺字段就如实说"没给"，不编一句默认值 —— 编出来的默认值会让用户
            // 以为那就是真源，而真源其实在别处。
            child: SingleChildScrollView(
              child: SelectableText(
                text ?? '（后端没有给这条的默认文本）',
                style: theme.textTheme.bodySmall,
              ),
            ),
          ),
        ],
      );

  Widget _editableBox(ThemeData theme, CopyEntry entry) => Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('当前值（可改，保存后对**下一次**运行生效）', style: theme.textTheme.labelMedium),
          const SizedBox(height: 4),
          TextField(
            controller: _edit,
            enabled: !_detailBusy,
            minLines: 6,
            maxLines: 14,
            style: theme.textTheme.bodySmall,
            decoration: InputDecoration(
              isDense: true,
              border: const OutlineInputBorder(),
              hintText: entry.overridden ? null : '（还没改过：保存后才会写进覆盖层）',
            ),
          ),
        ],
      );

  Widget _banner(ThemeData theme, String text, {required bool error}) => Container(
        width: double.infinity,
        padding: const EdgeInsets.all(10),
        decoration: BoxDecoration(
          color: (error ? theme.colorScheme.errorContainer : theme.colorScheme.secondaryContainer)
              .withValues(alpha: 0.5),
          borderRadius: BorderRadius.circular(8),
        ),
        child: Text(
          text,
          style: theme.textTheme.bodySmall?.copyWith(
            color: error ? theme.colorScheme.onErrorContainer : theme.colorScheme.onSecondaryContainer,
          ),
        ),
      );
}

/// 三态徽标（设计 §6）。长标签放在 `Flexible` 里可收缩，窄屏不会把它顶到屏幕外。
class _CopyStateBadge extends StatelessWidget {
  final CopyReviewState state;

  const _CopyStateBadge({required this.state});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final (Color fg, Color bg, IconData icon) = switch (state) {
      CopyReviewState.unverified => (
          theme.colorScheme.onSurfaceVariant,
          theme.colorScheme.surfaceContainerHighest,
          Icons.help_outline,
        ),
      CopyReviewState.verified => (
          theme.colorScheme.onPrimaryContainer,
          theme.colorScheme.primaryContainer,
          Icons.verified_outlined,
        ),
      CopyReviewState.baseChanged => (
          theme.colorScheme.onErrorContainer,
          theme.colorScheme.errorContainer,
          Icons.error_outline,
        ),
    };
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
      decoration: BoxDecoration(color: bg, borderRadius: BorderRadius.circular(6)),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: 13, color: fg),
          const SizedBox(width: 3),
          Flexible(
            child: Text(
              copyReviewStateLabel(state),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.labelSmall?.copyWith(color: fg),
            ),
          ),
        ],
      ),
    );
  }
}

/// 清单里的一行：分组标题行或条目行（[ListView.builder] 需要一个扁平的序列）。
class _CopyListRow {
  final String? header;
  final int count;
  final CopyEntry? entry;

  const _CopyListRow.header(this.header, this.count) : entry = null;
  const _CopyListRow.entry(this.entry) : header = null, count = 0;
}

/// 导入预览里的一行。
class _ImportRow {
  final ({String key, String text, String? note}) item;
  final CopyDiff? diff;

  /// 本端就判掉、不必问后端的（清单里没这条 / 与当前值相同…）。
  final String? skip;

  /// 后端或网络给的拦截原因。
  final String? blocked;

  const _ImportRow(this.item, {this.diff, this.skip, this.blocked});

  String? get blockedReason => blocked ?? diff?.blocked;
  bool get willApply => skip == null && blockedReason == null;
}

class _ImportPreviewDialog extends StatelessWidget {
  final List<_ImportRow> rows;

  const _ImportPreviewDialog({required this.rows});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final apply = rows.where((r) => r.willApply).length;
    return AlertDialog(
      title: const Text('导入预览'),
      content: SizedBox(
        width: 720,
        height: 420,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              '会改 $apply 条，跳过 ${rows.length - apply} 条。'
              '这里是 dryRun（一个字都还没写）；点「写入」才真的改。',
              style: theme.textTheme.bodySmall,
            ),
            const SizedBox(height: 8),
            Expanded(
              child: ListView.builder(
                itemCount: rows.length,
                itemBuilder: (context, i) {
                  final r = rows[i];
                  return Padding(
                    padding: const EdgeInsets.only(bottom: 8),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(r.item.key, style: theme.textTheme.labelMedium, maxLines: 1, overflow: TextOverflow.ellipsis),
                        if (r.blockedReason != null)
                          Text('跳过：${r.blockedReason}',
                              style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.error))
                        else if (r.skip != null)
                          Text('跳过：${r.skip}', style: theme.textTheme.labelSmall)
                        else
                          Text('（会改）', style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.primary)),
                      ],
                    ),
                  );
                },
              ),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.pop(context, false), child: const Text('取消')),
        FilledButton(
          onPressed: apply == 0 ? null : () => Navigator.pop(context, true),
          child: const Text('写入'),
        ),
      ],
    );
  }
}

/// 批量面板（设计 §6）：勾多条 → 选 op → **先看 diff** → 再应用。
///
/// 「应用」在**看到过 diff 之前是点不动的**，而且输入一改预览立刻作废 ——
/// 这就是"没有预览的批量改文案就是赌博"（设计 §4 硬规矩 1）在界面上的落点。
class _CopyBatchPanel extends StatefulWidget {
  final CopyApi api;
  final List<String> keys;

  const _CopyBatchPanel({required this.api, required this.keys});

  @override
  State<_CopyBatchPanel> createState() => _CopyBatchPanelState();
}

class _CopyBatchPanelState extends State<_CopyBatchPanel> {
  CopyBatchOp _op = CopyBatchOp.replace;
  final TextEditingController _find = TextEditingController();
  final TextEditingController _replace = TextEditingController();
  final TextEditingController _regex = TextEditingController();

  ({List<CopyDiff> items, int changed})? _preview;
  bool _busy = false;
  String? _error;

  /// 设计 §4 硬规矩 4：`prompt.nsfw` 参与批量时要二次确认。
  bool get _hasNsfw => widget.keys.contains('prompt.nsfw');

  @override
  void dispose() {
    _find.dispose();
    _replace.dispose();
    _regex.dispose();
    super.dispose();
  }

  /// 任何输入变化都让上一次预览作废：不然"看到的是 A、应用的是 B"。
  void _invalidate() => setState(() {
        _preview = null;
        _error = null;
      });

  Future<void> _dryRun() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final res = await widget.api.batch(
        keys: widget.keys,
        op: _op,
        find: _op.usesFind ? _find.text : null,
        replace: _op.usesReplace ? _replace.text : null,
        regex: _op.usesRegex ? _regex.text : null,
        dryRun: true,
      );
      if (!mounted) return;
      setState(() => _preview = res);
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _apply() async {
    if (_preview == null) return; // 双保险：按钮本来就点不动
    if (_hasNsfw) {
      final ok = await showDialog<bool>(
        context: context,
        builder: (ctx) => AlertDialog(
          title: const Text('这批里有 prompt.nsfw'),
          content: const Text(
            'prompt.nsfw 就是你有意注入的那一段「越狱」人格提示词（AGENTS §10：改它之前先问用户）。\n\n'
            '批量改完它会被清空校验状态，并且影响**下一次**请求注入的正文。确定要改吗？',
          ),
          actions: [
            TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('先不改')),
            FilledButton(onPressed: () => Navigator.pop(ctx, true), child: const Text('仍然应用')),
          ],
        ),
      );
      if (ok != true || !mounted) return;
    }
    setState(() => _busy = true);
    try {
      final res = await widget.api.batch(
        keys: widget.keys,
        op: _op,
        find: _op.usesFind ? _find.text : null,
        replace: _op.usesReplace ? _replace.text : null,
        regex: _op.usesRegex ? _regex.text : null,
        confirm: _hasNsfw,
        dryRun: false,
      );
      if (!mounted) return;
      Navigator.pop(context, res.changed);
    } catch (e) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = '$e';
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return AlertDialog(
      title: Text('批量修改 ${widget.keys.length} 条'),
      content: SizedBox(
        width: 760,
        height: 560,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            AppMenuButton<CopyBatchOp>(
              options: [
                for (final op in CopyBatchOp.values)
                  MenuOption(value: op, label: copyBatchOpLabel(op), subtitle: copyBatchOpHint(op)),
              ],
              onSelected: (op) => setState(() {
                _op = op;
                _preview = null;
              }),
              button: (ctx, controller, isOpen) => OutlinedButton.icon(
                onPressed: () => isOpen ? controller.close() : controller.open(),
                icon: const Icon(Icons.playlist_play, size: 18),
                label: Text('操作：${copyBatchOpLabel(_op)}', overflow: TextOverflow.ellipsis),
              ),
            ),
            const SizedBox(height: 8),
            if (_op.usesFind)
              TextField(
                controller: _find,
                onChanged: (_) => _invalidate(),
                decoration: const InputDecoration(isDense: true, border: OutlineInputBorder(), labelText: 'find（要找的文本）'),
              ),
            if (_op.usesRegex)
              TextField(
                controller: _regex,
                onChanged: (_) => _invalidate(),
                decoration: const InputDecoration(isDense: true, border: OutlineInputBorder(), labelText: 'regex（默认不是多行，要 (?m) 自己写）'),
              ),
            if (_op.usesReplace)
              TextField(
                controller: _replace,
                onChanged: (_) => _invalidate(),
                minLines: 2,
                maxLines: 4,
                decoration: const InputDecoration(isDense: true, border: OutlineInputBorder(), labelText: 'replace（换成什么）'),
              ),
            const SizedBox(height: 8),
            Text('将作用在：${widget.keys.join('、')}',
                style: theme.textTheme.labelSmall, maxLines: 2, overflow: TextOverflow.ellipsis),
            if (_hasNsfw) ...[
              const SizedBox(height: 4),
              Text('⚠ 含 prompt.nsfw：应用前会再确认一次。',
                  style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.error)),
            ],
            if (_error != null) ...[
              const SizedBox(height: 6),
              Text(_error!, style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.error)),
            ],
            const Divider(height: 16),
            Expanded(
              child: _preview == null
                  ? Center(
                      child: Text(
                        '先点「先看 diff」——没有预览的批量改文案就是赌博。',
                        textAlign: TextAlign.center,
                        style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.outline),
                      ),
                    )
                  : _previewList(theme),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(onPressed: _busy ? null : () => Navigator.pop(context), child: const Text('取消')),
        FilledButton.tonal(
          onPressed: _busy ? null : _dryRun,
          child: Text(_busy && _preview == null ? 'dryRun 中…' : '先看 diff'),
        ),
        FilledButton(
          onPressed: (_busy || _preview == null) ? null : _apply,
          child: const Text('应用'),
        ),
      ],
    );
  }

  Widget _previewList(ThemeData theme) {
    final preview = _preview!;
    final items = preview.items;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text('dryRun 结果：${preview.changed} 条会变，'
            '${items.where((d) => d.blocked != null).length} 条被拦下（一条改坏不连坐）。',
            style: theme.textTheme.bodySmall),
        const SizedBox(height: 6),
        Expanded(
          child: ListView.builder(
            itemCount: items.length,
            itemBuilder: (context, i) {
              final d = items[i];
              return Card(
                margin: const EdgeInsets.only(bottom: 8),
                child: Padding(
                  padding: const EdgeInsets.all(10),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Row(
                        children: [
                          Expanded(
                            child: Text(d.key,
                                style: theme.textTheme.labelMedium,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis),
                          ),
                          const SizedBox(width: 6),
                          if (d.blocked != null)
                            Text('被拦下', style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.error))
                          else if (d.changed)
                            Text('会改', style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.primary))
                          else
                            Text('没命中，不变', style: theme.textTheme.labelSmall),
                        ],
                      ),
                      if (d.blocked != null)
                        Padding(
                          padding: const EdgeInsets.only(top: 4),
                          child: Text(d.blocked!,
                              style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.error)),
                        ),
                      if (d.diff != null && d.diff!.isNotEmpty)
                        Container(
                          width: double.infinity,
                          margin: const EdgeInsets.only(top: 6),
                          padding: const EdgeInsets.all(8),
                          decoration: BoxDecoration(
                            color: theme.colorScheme.surfaceContainerHighest.withValues(alpha: 0.4),
                            borderRadius: BorderRadius.circular(6),
                          ),
                          // 用 Text 而不是 SelectableText：diff 是"看一眼"的东西，
                          // 而 SelectableText 会在每一条里插一个可聚焦的编辑区，
                          // 长清单里反而不好选（要复制的话整条 key 都能复制）。
                          child: Text(
                            d.diff!,
                            style: theme.textTheme.labelSmall?.copyWith(fontFamily: 'monospace'),
                          ),                        ),
                    ],
                  ),
                ),
              );
            },
          ),
        ),
      ],
    );
  }
}
