import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/ai_api_client.dart';
import '../core/provider_store.dart';
import '../models/ai_models.dart';
import '../state/ai_workspace_store.dart';
import 'ai_model_capability_card.dart';
import 'ai_provider_dialogs.dart';

/// 一行模型的高度（常数）。
///
/// **别改成按内容自适应**：`SliverFixedExtentList` 就是靠这个常数精确算出
/// `maxScrollExtent` 的，一旦不固定，滚动条滑块又会随滚动变短（用户报的 bug ④）。
const double kModelRowHeight = 64;

/// 模态 → 图标（模型行右侧的能力一览）。
const Map<AiModality, IconData> kModalityIcons = {
  AiModality.text: Icons.notes_outlined,
  AiModality.image: Icons.image_outlined,
  AiModality.video: Icons.movie_outlined,
  AiModality.audio: Icons.graphic_eq,
  AiModality.document: Icons.description_outlined,
};

/// 「模型目录」子页：一整块滚动区 + 底部保存条。
///
/// 从 `ai_provider_settings_page.dart`（当时 2,956 行）拆出来的第二块。
/// 上方那块 [header]（Provider 信息 + 凭据）由详情页装配进来 —— 这一层只管
/// 模型行、能力概览、保存与「获取可用模型 / 手动添加」。
///
/// 两条不变量没动：
///  1. **固定行高 + 编辑弹窗**（[_editor]）：`SliverFixedExtentList` 的 extent 是常数，
///     滚动条首帧就精确；能力编辑搬进弹窗，不再每行 7 个 chip + 下拉框；
///  2. 本地改动先留在本组件（`_dirty`），点「保存模型目录」才落库；父级重建
///     不会把用户没保存的编辑冲掉。
class AiProviderModelsSection extends StatefulWidget {
  /// 滚动区顶部那一块：Provider 信息 + 凭据（由详情页拼好传进来）。
  final Widget header;

  final AiApiClient api;

  /// 清单来源（手机 = 本机库）：**模型目录的保存**走它，连接的测试/发现看它的 [ProviderStore.supportsProbe]。
  final ProviderStore providers;
  final AiProvider provider;
  final List<AiModel> models;

  /// 「模型能力目录」那一块（在线数据集 = **models.dev**）。
  ///
  /// ⚠ 它是**全局**状态（一个 `remote.json`），却只显示在**这一条** Provider 的详情里 ——
  /// 2026-10 用户要求："模型能力目录区域占用窗口区域太大，改成嵌在具体 provider 信息里"。
  /// 折叠起来默认不展开：它是低频动作（更新目录 / 校准档位），不该把模型目录挤下去。
  final AiCapabilityStatus? capability;
  final bool capabilityBusy;
  final bool probeBusy;
  final VoidCallback? onRefreshCapability;
  final VoidCallback? onUpdateCapabilityRemote;
  final ValueChanged<bool>? onProbeToggle;
  final VoidCallback? onProbe;
  final Future<void> Function(String message) onChanged;

  const AiProviderModelsSection({
    super.key,
    required this.header,
    required this.api,
    required this.providers,
    required this.provider,
    required this.models,
    required this.capability,
    required this.capabilityBusy,
    required this.probeBusy,
    required this.onRefreshCapability,
    required this.onUpdateCapabilityRemote,
    required this.onProbeToggle,
    required this.onProbe,
    required this.onChanged,
  });

  @override
  State<AiProviderModelsSection> createState() =>
      _AiProviderModelsSectionState();
}

class _AiProviderModelsSectionState extends State<AiProviderModelsSection> {
  bool _busy = false;
  final _models = <AiModel>[];

  /// 本地有未保存的改动（增删模型、改能力勾选）。
  bool _dirty = false;

  /// 任何本地模型改动都要置脏：否则父级重建时会把用户没保存的编辑冲掉。
  void _markDirty() => _dirty = true;

  @override
  void initState() {
    super.initState();
    _models.addAll(widget.models);
  }

  /// 父级把模型目录加载完之后会重建本组件（key 带 revision）。
  ///
  /// **必须有这一条**：只靠 `initState` 初始化的话，父级异步补上目录时
  /// 本地 `_models` 还停在空列表，界面就一直显示"还没有模型" ——
  /// 这正是"获取完可用模型并加入后，退出重进看不到模型"的成因。
  /// 用户在本地已经改过（还没保存）时**不要**用父级数据覆盖。
  @override
  void didUpdateWidget(AiProviderModelsSection oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (_dirty) return;
    if (widget.provider.id != oldWidget.provider.id ||
        !_sameModels(widget.models, oldWidget.models)) {
      setState(() {
        _models
          ..clear()
          ..addAll(widget.models);
      });
    }
  }

  static bool _sameModels(List<AiModel> a, List<AiModel> b) {
    if (identical(a, b)) return true;
    if (a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      if (a[i].id != b[i].id) return false;
    }
    return true;
  }

  void _toast(String message, {bool error = false}) {
    if (!mounted) return;
    ScaffoldMessenger.of(context)
      ..clearSnackBars()
      ..showSnackBar(
        SnackBar(
          content: Text(message),
          behavior: SnackBarBehavior.floating,
          duration: Duration(seconds: error ? 8 : 3),
          backgroundColor: error
              ? Theme.of(context).colorScheme.errorContainer
              : null,
        ),
      );
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    // 模型目录那一行的表头（固定几条）+ 模型行。**必须用 builder 懒构建**：
    // 以前是 `children: [..., for (var i...) _modelTile(...)]`，
    // 每个模型卡里有 6 个 FilterChip + 一个下拉框，几十个模型全在首帧就建出来，
    // 滚动时整个列表跟着重建 —— 这就是"AI 模型与凭据界面滑动卡顿"的主因。
    final modelsHeader = <Widget>[
      const Divider(height: 32),
      // `Wrap` 而不是 `Row` + `Spacer`：320dp 的手机上「模型目录 + 获取可用模型 + 手动添加」
      // 三个挨在一排会 `RenderFlex overflowed by 17 pixels`（`phone_layout_overflow_test`
      // 那条 320dp 用例盯的就是它）。窄了就换行，不裁。
      Wrap(
        crossAxisAlignment: WrapCrossAlignment.center,
        spacing: 8,
        runSpacing: 4,
        children: [
          Text('模型目录', style: theme.textTheme.titleSmall),
          TextButton.icon(
            // 同上：手机上还做不到（要本机自己去拉上游的 /models）
            onPressed: (_busy || !widget.providers.supportsProbe)
                ? null
                : _discover,
            icon: const Icon(Icons.cloud_download_outlined, size: 18),
            label: const Text('获取可用模型'),
          ),
          TextButton.icon(
            onPressed: _busy ? null : _addModel,
            icon: const Icon(Icons.add, size: 18),
            label: const Text('手动添加'),
          ),
        ],
      ),
      Text(
        '本 harness 不按模型名字猜能力。',
        style: theme.textTheme.labelSmall?.copyWith(
          color: theme.colorScheme.outline,
        ),
      ),
      const SizedBox(height: 8),
      if (_models.isEmpty) Text('还没有模型', style: theme.textTheme.bodySmall),
      // 「模型能力目录」：**嵌在这一条 Provider 的详情里**（2026-10 用户要求）。
      // 放在模型目录之后、且默认折叠 —— 它是低频动作，不该把上面的密钥/连接测试挤走。
      const SizedBox(height: 16),
      _capabilitySection(theme),
    ];
    final count = _models.length;

    // 列表结构是 **固定行高 + 编辑弹窗**（用户报的 bug ③ 卡顿 / ④ 滚动条不准）：
    //
    //   · 懒构建的 `ListView` / `SliverList` 只能用"**已布局**子项的平均高度"估算
    //     `maxScrollExtent`。卡片高矮不一时（支持推理的卡片多出 7 个档位 chip），
    //     越往下滚估算值越大 —— 实测 69 个模型时从 3512 一路涨到 21318，
    //     滚动条滑块于是从 17.9% 缩到 3.5%，位置也全错。
    //   · `SliverFixedExtentList` 的 extent 是**算出来的常数**，首帧就精确，
    //     滚动条不再跳；每行也不再是"7 个 FilterChip + 下拉框"，快速滑动不掉帧。
    //
    // 能力编辑没有丢：点一行（或右侧的调节按钮）打开编辑弹窗，见 [AiModelEditorDialog]。
    return Column(
      children: [
        Expanded(
          child: CustomScrollView(
            slivers: [
              SliverPadding(
                padding: const EdgeInsets.fromLTRB(16, 16, 16, 8),
                sliver: SliverToBoxAdapter(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [widget.header, ...modelsHeader],
                  ),
                ),
              ),
              SliverFixedExtentList(
                itemExtent: kModelRowHeight,
                delegate: SliverChildBuilderDelegate(
                  (context, i) => RepaintBoundary(child: _modelRow(theme, i)),
                  childCount: count,
                ),
              ),
              const SliverToBoxAdapter(child: SizedBox(height: 16)),
            ],
          ),
        ),
        // 保存按钮常驻底部：69 个模型不用一路滚到底才找得到它
        Container(
          decoration: BoxDecoration(
            border: Border(top: BorderSide(color: theme.dividerColor)),
            color: theme.colorScheme.surface,
          ),
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
          child: Row(
            children: [
              if (_dirty)
                Text(
                  '有未保存的改动',
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: theme.colorScheme.primary,
                  ),
                )
              else
                Text(
                  '模型目录已与后端一致',
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: theme.colorScheme.outline,
                  ),
                ),
              const Spacer(),
              FilledButton.icon(
                onPressed: _busy || _models.isEmpty ? null : _saveModels,
                icon: const Icon(Icons.save_outlined, size: 18),
                label: const Text('保存模型目录'),
              ),
            ],
          ),
        ),
      ],
    );
  }

  /// 「模型能力目录」那张卡，**折叠**着挂在这一条 Provider 的详情末尾。
  ///
  /// 为什么折叠而不是直接摊开：它有"立即更新 / 校准档位"两个按钮和一行状态标签，
  /// 展开一次就有两百多像素 —— 而模型目录才是这一页的主体（2026-10 用户报的原话：
  /// 「模型能力目录区域占用窗口区域太大」）。标题上永远看得见新鲜度那一行，
  /// 折叠状态下也知道"目录是什么时候的"，不用展开才知道。
  Widget _capabilitySection(ThemeData theme) {
    final s = widget.capability;
    return Theme(
      // 去掉分隔线：这块是**卡片区**（下面自带 Card），不是又一个分组标题
      data: theme.copyWith(dividerColor: Colors.transparent),
      child: ExpansionTile(
        // key 跟着 Provider 走：每一条的展开与否是**它自己的**选择（外面找得到
        // PageStorage 就跨切换保留，找不到就重新折叠 —— 两种都不影响别的行为）
        key: PageStorageKey('capability-${widget.provider.id}'),
        initiallyExpanded: false,
        tilePadding: EdgeInsets.zero,
        childrenPadding: EdgeInsets.zero,
        title: Row(
          children: [
            Icon(
              Icons.auto_awesome_motion_outlined,
              size: 18,
              color: theme.colorScheme.primary,
            ),
            const SizedBox(width: 6),
            Text('模型能力目录', style: theme.textTheme.titleSmall),
            const SizedBox(width: 8),
            if (widget.capabilityBusy || widget.probeBusy)
              const SizedBox(
                width: 10,
                height: 12,
                child: CircularProgressIndicator(strokeWidth: 2),
              )
            else
              Flexible(
                child: Text(
                  s == null
                      ? '正在读取…'
                      : (s.error != null ? '读取失败' : s.remoteLabel),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: s?.remoteStale == true
                        ? theme.colorScheme.error
                        : theme.colorScheme.outline,
                  ),
                ),
              ),
          ],
        ),
        children: [
          AiModelCapabilityCard(
            status: widget.capability,
            busy: widget.capabilityBusy,
            probeBusy: widget.probeBusy,
            onRefreshStatus: widget.onRefreshCapability,
            onUpdateRemote: widget.onUpdateCapabilityRemote,
            onProbeToggle: widget.onProbeToggle,
            onProbe: widget.onProbe,
            // 卡片自己那圈 Card 外边距在折叠区里会多出一圈空白，去掉
            margin: EdgeInsets.zero,
          ),
        ],
      ),
    );
  }

  /// 一行模型 = **固定高度**（[kModelRowHeight]），能力概览用图标表达。
  Widget _modelRow(ThemeData theme, int index) {
    final m = _models[index];
    return InkWell(
      onTap: _busy ? null : () => _editModel(index),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16),
        child: Row(
          children: [
            Expanded(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    m.displayName,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.titleSmall,
                  ),
                  const SizedBox(height: 2),
                  Text(
                    m.id,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.outline,
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 8),
            _capabilityIcons(theme, m),
            IconButton(
              tooltip: '编辑能力',
              visualDensity: VisualDensity.compact,
              icon: const Icon(Icons.tune, size: 18),
              onPressed: _busy ? null : () => _editModel(index),
            ),
            IconButton(
              tooltip: '移除',
              visualDensity: VisualDensity.compact,
              icon: const Icon(Icons.close, size: 16),
              onPressed: _busy
                  ? null
                  : () => setState(() {
                      _models.removeAt(index);
                      _markDirty();
                    }),
            ),
          ],
        ),
      ),
    );
  }

  /// 能力一览：只画**声明过**的（未知即不支持，宁可不画也不猜）。
  Widget _capabilityIcons(ThemeData theme, AiModel m) {
    final items = <(IconData, String)>[
      for (final modality in AiModality.values)
        if (m.supports(modality)) (kModalityIcons[modality]!, modality.label),
      if (m.tools) (Icons.handyman_outlined, '工具'),
      if (m.reasoning) (Icons.psychology_outlined, '推理'),
    ];
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (final (icon, label) in items)
          Tooltip(
            message: label,
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 2),
              child: Icon(
                icon,
                size: 15,
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ),
      ],
    );
  }

  /// 打开某个模型的能力编辑弹窗；点了「确定」才写回列表（仍是本地改动，等「保存模型目录」落库）。
  Future<void> _editModel(int index) async {
    final edited = await showDialog<AiModel>(
      context: context,
      builder: (_) => AiModelEditorDialog(model: _models[index], busy: _busy),
    );
    if (edited == null || !mounted) return;
    setState(() {
      _models[index] = edited;
      _markDirty();
    });
  }

  /// 拉取候选模型。**发现只是候选**：勾选后先进本地列表，仍需点「保存模型目录」才落库；
  /// 能力（图片/视频/工具…）不会因为发现而被推断，一律保持"未声明"（AIH-009 / AIH-011）。
  Future<void> _discover() async {
    if (!widget.providers.supportsProbe) {
      // 兜底（按钮已经禁用了）：说清"这一端还做不到"，而不是去问桌面（它不认识本机这条 id）
      _toast('手机上还没有"获取模型"，这一步要由本机自己去拉上游的 /models，本批未实现。', error: true);
      return;
    }
    setState(() => _busy = true);
    try {
      final result = await widget.api.discoverModels(widget.provider.id);
      if (!mounted) return;
      if (!result.ok) {
        _toast('获取模型失败，${result.display}', error: true);
        return;
      }
      if (result.candidates.isEmpty) {
        _toast('端点没有返回模型列表（${result.message}）', error: true);
        return;
      }
      final picked = await showDialog<List<AiModelCandidate>>(
        context: context,
        builder: (_) => AiDiscoverDialog(result: result),
      );
      if (picked == null || picked.isEmpty) return;
      var added = 0;
      setState(() {
        _markDirty();
        for (final c in picked) {
          if (_models.any((m) => m.id == c.id)) continue;
          _models.add(
            AiModel(
              providerId: widget.provider.id,
              id: c.id,
              displayName: c.displayName,
              // 用发现阶段预填/用户确认过的能力，而不是无脑"仅文本"
              inputModalities: c.modalities.map((m) => m.wire).toList(),
              attachmentTransports: {
                for (final m in c.modalities)
                  if (m != AiModality.text) m.wire: const ['inline_base64'],
              },
              tools: c.tools,
              reasoning: c.reasoning,
              // 思考档位/方言也从内置目录预填（AIH-056）：用户能在模型卡片上改
              thinkingEfforts: c.thinkingEfforts,
              thinkingFormat: c.thinkingFormat,
              contextWindow: c.contextWindow,
              maxOutputTokens: c.maxOutputTokens,
              // 来源如实记录：接口声明 → discovered；内置目录 → builtin；未识别 → manual（用户接受默认）
              capabilitySource: switch (c.capabilitySource) {
                'discovered' => 'discovered',
                'builtin' => 'builtin',
                'tested' => 'tested',
                _ => 'manual',
              },
            ),
          );
          added++;
        }
      });
      if (added == 0) {
        _toast('这些候选已经在目录里了。');
        return;
      }
      // 立刻落库：不要让用户以为"加了但没生效"
      await _saveModels();
      _toast('已加入并保存 $added 个模型');
    } catch (e) {
      _toast('获取模型失败，$e', error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _addModel() async {
    final draft = await showDialog<AiModelDraft>(
      context: context,
      builder: (_) => const AiModelDialog(),
    );
    if (draft == null) return;
    if (draft.id.isEmpty) {
      _toast('模型 ID 不能为空', error: true);
      return;
    }
    if (_models.any((m) => m.id == draft.id)) {
      _toast('模型 ${draft.id} 已经在目录里了');
      return;
    }
    setState(() {
      _markDirty();
      _models.add(
        AiModel(
          providerId: widget.provider.id,
          id: draft.id,
          displayName: draft.displayName,
          inputModalities: draft.modalities.map((m) => m.wire).toList(),
          attachmentTransports: {
            for (final m in draft.modalities)
              if (m != AiModality.text) m.wire: const ['inline_base64'],
          },
          tools: draft.tools,
          capabilitySource: 'manual',
        ),
      );
    });
    // 点一次「添加」就应该真的加上：立刻落库
    await _saveModels();
    _toast('已添加并保存模型「${draft.displayName}」');
  }

  Future<void> _saveModels() async {
    setState(() => _busy = true);
    try {
      // 手机走本机库（一条网络请求都不打）；桌面走后端库。**同一份 toJson()**，两端不翻译。
      await widget.providers.saveModels(widget.provider.id, _models);
      _dirty = false; // 已经落库，之后父级重建可以直接覆盖本地副本
      // 工作台手里那份模型目录只在"打开模型选择器 / 重新进页面"时才重读；用户在这里
      // 改完思考强度 / 能力声明、保存后直接切回工作台，看到的还是旧声明
      // （用户 bug 2026-10-24："在 AI 模型设置里手动指定过模型的思考强度，但保存之后
      // 并没有在 AI 工作台应用"）。保存成功就当场把工作台那份刷掉，别等他再点一次选择器。
      if (mounted) {
        try {
          final workspace = Provider.of<AiWorkspaceStore>(
            context,
            listen: false,
          );
          await workspace.refreshModels();
          await workspace.refreshProviders();
        } catch (_) {
          // 设置页也可能在没有工作台 store 的环境里单独打开（测试 / 独立入口）；
          // 刷新失败不该把"保存成功"显示成失败。
          debugPrint('[ai-provider] 保存模型目录后刷新工作台失败（不影响已落库的结果）');
        }
      }
      await widget.onChanged('模型目录已保存（${_models.length} 个）。');
    } catch (e) {
      _toast('保存模型目录失败，$e', error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }
}
