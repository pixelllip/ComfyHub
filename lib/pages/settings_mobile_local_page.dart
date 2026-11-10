import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../core/settings_store.dart';
import '../state/ai_workspace_store.dart';
import '../state/library_store.dart';
import '../widgets/settings_scaffold.dart';
import 'settings_pairing_page.dart';

/// 「本机数据与同步」——手机端专用的「本地服务」。
///
/// 桌面上那一项叫「本地服务（后端 + 数据库）」：拉后端、看日志、换运行根目录。
/// 手机上这些一**样都不成立**（没有 Rust exe、没有盘符、没有资源管理器），
/// 所以手机要显示的是它真正拥有的那一套：
///
///  * **桌面连接**：手机连的是哪台电脑、配没配对、现在通不通；
///  * **本机库**：会话存在本机（桌面离线也能翻），还有几条等着推到桌面；
///  * **同步**：立即上行 / 从桌面拉取 —— 这是 M4 的两半，此前没有入口，
///    只能等待"回到前台"自动补传；
///  * **本机数据在哪**：App 私有目录 + 产物缓存，卸载即清空（别让人以为是桌面那份）。
///
/// 手机端使用对应的专用功能，不显示仅桌面可用的功能。
class MobileLocalSettingsPage extends StatefulWidget {
  const MobileLocalSettingsPage({super.key});

  @override
  State<MobileLocalSettingsPage> createState() =>
      _MobileLocalSettingsPageState();
}

class _MobileLocalSettingsPageState extends State<MobileLocalSettingsPage> {
  bool _busy = false;
  int? _pending;
  String? _notice;
  bool _noticeError = false;
  String? _health;
  bool _healthOk = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _refresh());
  }

  /// 首次进来把"待上行"的数读一次。
  ///
  /// 会话数直接用 `AiWorkspaceStore` 里已经加载好的那一份（AI 工作台是默认页，
  /// 冷启动就会加载）；这里**不再重复 load 一遍**，免得设置页也去拉 Provider /
  /// run-kit —— 那会把一个"看状态"的页面变成一次启动动作。
  Future<void> _refresh() => _refreshPending();

  Future<void> _refreshPending() async {
    final store = context.read<AiWorkspaceStore>();
    try {
      final n = await store.pendingOutboxCount();
      if (mounted) setState(() => _pending = n);
    } catch (e) {
      if (mounted) {
        setState(() {
          _notice = '读本机上行队列失败，$e';
          _noticeError = true;
        });
      }
    }
  }

  Future<void> _testConnection() async {
    final store = context.read<LibraryStore>();
    setState(() {
      _busy = true;
      _health = null;
    });
    try {
      final h = await store.api.health();
      if (!mounted) return;
      final ok = h['status'] == 'ok';
      setState(() {
        _healthOk = ok;
        _health = ok
            ? '连上了，版本 ${h['version'] ?? '未知'}，数据库 ${h['database'] ?? '未知'}。'
            : '连上了，但服务状态是 ${h['status']}。';
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _healthOk = false;
        _health = '连不上这台电脑。$e';
      });
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _syncUp() async {
    final store = context.read<AiWorkspaceStore>();
    setState(() {
      _busy = true;
      _notice = null;
      _noticeError = false;
    });
    try {
      final applied = await store.syncOutbox();
      final pending = await store.pendingOutboxCount();
      if (!mounted) return;
      final notice = store.pushNotice;
      setState(() {
        _pending = pending;
        _noticeError = pending > 0;
        if (notice != null && notice.isNotEmpty) {
          _notice = notice;
        } else if (applied > 0) {
          _notice = '已把本机 $applied 条记录同步到桌面。';
        } else {
          _notice = pending > 0 ? '本机还有 $pending 条没同步到桌面。' : '本机没有待同步的记录。';
        }
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _notice = '同步到桌面失败，$e';
        _noticeError = true;
      });
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _pullDown() async {
    final store = context.read<AiWorkspaceStore>();
    setState(() {
      _busy = true;
      _notice = null;
      _noticeError = false;
    });
    try {
      final applied = await store.pullChanges();
      if (!mounted) return;
      final notice = store.pushNotice;
      final failed = notice != null && notice.contains('失败');
      setState(() {
        _noticeError = failed;
        if (applied > 0) {
          _notice = notice ?? '从桌面拉取了 $applied 条变化。';
        } else if (failed) {
          _notice = notice;
        } else {
          _notice = '桌面那边没有新的变化。';
        }
      });
      await _refreshPending();
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _notice = '从桌面拉取失败，$e';
        _noticeError = true;
      });
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final settings = context.watch<SettingsStore>();
    final store = context.watch<AiWorkspaceStore>();
    final paired = (store.pairingToken ?? '').trim().isNotEmpty;
    final base = settings.baseUrl.trim();
    final pending = _pending;

    return SettingsScaffold(
      title: '本机数据与同步',
      sections: [
        SettingsSection(
          title: '桌面连接',
          estimatedHeight: 260,
          child: Card(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(16, 12, 16, 12),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    base.isEmpty ? '还没填桌面地址' : base,
                    style: theme.textTheme.titleSmall,
                  ),
                  const SizedBox(height: 4),
                  Text(
                    paired
                        ? '已经和这台电脑配对，聊天记录会推给它。'
                        : '还没有配对令牌。配对之后才能同步与取密钥。',
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: paired
                          ? theme.colorScheme.onSurfaceVariant
                          : theme.colorScheme.error,
                    ),
                  ),
                  if (_health != null) ...[
                    const SizedBox(height: 8),
                    Text(
                      _health!,
                      style: theme.textTheme.bodySmall?.copyWith(
                        color: _healthOk
                            ? theme.colorScheme.onSurfaceVariant
                            : theme.colorScheme.error,
                      ),
                    ),
                  ],
                  const SizedBox(height: 12),
                  Wrap(
                    spacing: 8,
                    runSpacing: 8,
                    children: [
                      OutlinedButton.icon(
                        onPressed: _busy ? null : _testConnection,
                        icon: const Icon(Icons.wifi_tethering, size: 18),
                        label: const Text('测试连接'),
                      ),
                      OutlinedButton.icon(
                        onPressed: _busy
                            ? null
                            : () => Navigator.of(context).push(
                                MaterialPageRoute<void>(
                                  builder: (_) => const PairingSettingsPage(),
                                ),
                              ),
                        icon: const Icon(Icons.qr_code_2, size: 18),
                        label: const Text('换电脑或重新配对'),
                      ),
                    ],
                  ),
                ],
              ),
            ),
          ),
        ),
        SettingsSection(
          title: '本机库',
          estimatedHeight: 300,
          child: Card(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(16, 12, 16, 12),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  _row(theme, '本机会话', '${store.conversations.length} 条'),
                  const Divider(height: 20),
                  _row(
                    theme,
                    '待上行',
                    pending == null ? '正在读…' : '$pending 条',
                  ),
                  const SizedBox(height: 6),
                  Text(
                    '手机上的聊天先落在本机库，桌面离线也能翻看。桌面在线时它会自己推上去，'
                    '桌面上改过的东西在回到前台时也会拉下来。',
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: theme.colorScheme.onSurfaceVariant,
                    ),
                  ),
                  const SizedBox(height: 12),
                  Wrap(
                    spacing: 8,
                    runSpacing: 8,
                    children: [
                      FilledButton.icon(
                        onPressed: _busy ? null : _syncUp,
                        icon: const Icon(Icons.cloud_upload_outlined, size: 18),
                        label: const Text('立即同步到桌面'),
                      ),
                      OutlinedButton.icon(
                        onPressed: _busy ? null : _pullDown,
                        icon: const Icon(Icons.cloud_download_outlined, size: 18),
                        label: const Text('从桌面拉取'),
                      ),
                    ],
                  ),
                  if (_notice != null && _notice!.isNotEmpty) ...[
                    const SizedBox(height: 10),
                    Text(
                      _notice!,
                      style: theme.textTheme.bodySmall?.copyWith(
                        color: _noticeError
                            ? theme.colorScheme.error
                            : theme.colorScheme.onSurfaceVariant,
                      ),
                    ),
                  ],
                ],
              ),
            ),
          ),
        ),
        SettingsSection(
          title: '本机数据在哪',
          estimatedHeight: 180,
          child: Card(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(16, 12, 16, 12),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    '都在这台 App 的私有目录里，卸载 App 就一起没了。'
                    '桌面那份是另一套，两边各存各的，只通过配对同步。',
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: theme.colorScheme.onSurfaceVariant,
                    ),
                  ),
                  const SizedBox(height: 8),
                  Text(
                    '产物缓存（缩略图与原件）在设置首页的「产物缓存（本机）」里，'
                    '要存进手机相册就在画廊里打开那一张，用详情菜单的「保存到手机相册」。',
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: theme.colorScheme.onSurfaceVariant,
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ],
    );
  }

  Widget _row(ThemeData theme, String label, String value) {
    return Row(
      children: [
        SizedBox(
          width: 96,
          child: Text(label, style: theme.textTheme.bodyMedium),
        ),
        Expanded(
          child: Text(
            value,
            style: theme.textTheme.titleSmall?.copyWith(
              fontWeight: FontWeight.w700,
            ),
          ),
        ),
      ],
    );
  }
}
