import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:material_ui/material_ui.dart';
import 'package:provider/provider.dart';
import 'package:url_launcher/url_launcher.dart';
import 'package:webview_all/webview_all.dart';

import '../core/api_client.dart';
import '../core/settings_store.dart';
import '../core/theme.dart';
import '../models/train_models.dart';

/// 本地 LoRA 训练台（anima-lora-next 的自有 WebUI 内嵌）。
///
/// 为什么是 WebView 而不是把训练表单重画一遍：
///
/// * **同一份事实**：项目 / 数据集 / 打标 / 预处理 / 训练的状态全部由引擎的
///   `anima_daemon` 持有，WebUI 只是它的一层界面。Flutter 再画一套表单，就会
///   出现“App 里能改、网页里看不到”的第二份真相；
/// * **AI 与人是同一套操作**：AI 工作台调用的是同一批 `/studio/*` 工具，
///   人在训练台里做的筛选、caption 修改会立刻被 AI 看到（反之亦然）；
/// * **手机也能用**：WebUI 是响应式的，手机连的是桌面后端的局域网地址，
///   这里只要把 host 换成 `SettingsStore.baseUrl` 的 host 即可。
///
/// 页面本身只负责三件事：显示引擎状态、起停（`/api/train/*`）、把 WebUI 装进来。
class TrainPage extends StatefulWidget {
  /// 测试注入缝：默认按当前 `SettingsStore.baseUrl` 造一个真实客户端。
  ///
  /// 为什么留这个参数：WebView 本身在 widget test 里没有平台实现，但“装没装 /
  /// WebUI 起没起 / 手机宽度会不会溢出”这三件事必须能在 CI 里钉住 ——
  /// 它们恰好都不需要真的渲染 WebView。
  final ApiClient Function(BuildContext context)? apiFactory;

  const TrainPage({super.key, this.apiFactory});

  @override
  State<TrainPage> createState() => _TrainPageState();
}

class _TrainPageState extends State<TrainPage> {
  TrainEngineStatus? _status;
  String? _error;
  bool _loading = true;
  bool _busy = false;

  /// 换一次 URL / 点一次刷新就 +1，让 WebView 整棵重建（否则它不会重新请求）。
  int _reload = 0;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _refresh());
  }

  ApiClient _client() =>
      widget.apiFactory?.call(context) ??
      ApiClient(context.read<SettingsStore>().baseUrl);

  Future<void> _refresh() async {
    if (!mounted) return;
    setState(() {
      _loading = _status == null;
      _error = null;
    });
    final client = _client();
    try {
      final status = await client.trainStatus();
      if (!mounted) return;
      setState(() {
        _status = status;
        _loading = false;
        _reload++;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = '$e';
        _loading = false;
      });
    } finally {
      client.dispose();
    }
  }

  Future<void> _start() async {
    setState(() => _busy = true);
    final client = _client();
    try {
      final status = await client.trainStart();
      if (!mounted) return;
      setState(() {
        _status = status;
        _reload++;
        _error = null;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() => _error = '$e');
    } finally {
      client.dispose();
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _stop() async {
    setState(() => _busy = true);
    final client = _client();
    try {
      final status = await client.trainStop();
      if (!mounted) return;
      setState(() {
        _status = status;
        _reload++;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() => _error = '$e');
    } finally {
      client.dispose();
      if (mounted) setState(() => _busy = false);
    }
  }

  /// WebUI 的 URL：端口来自后端状态，host 用**手机当前连的那个后端 host**。
  ///
  /// 手机上的 `127.0.0.1` 是手机自己，不是桌面；所以绝不能直接拿
  /// `status.webui.url`（那是桌面视角的回环地址）。桌面/模拟器上 baseUrl 的
  /// host 本来就是 127.0.0.1 / 10.0.2.2，替换后仍然对。
  String? _webviewUrl() {
    final status = _status;
    final port = status?.webui.port;
    if (status == null || !status.webui.up || port == null) return null;
    final base = Uri.tryParse(context.read<SettingsStore>().baseUrl);
    var host = base?.host ?? '';
    if (host.isEmpty) host = '127.0.0.1';
    final token = status.webui.token;
    final query = (token == null || token.isEmpty)
        ? ''
        : '?token=${Uri.encodeQueryComponent(token)}';
    return 'http://$host:$port/$query';
  }

  Future<void> _openInBrowser() async {
    final url = _webviewUrl();
    if (url == null) return;
    final uri = Uri.parse(url);
    if (!await launchUrl(uri, mode: LaunchMode.externalApplication)) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text('打不开浏览器，$url')));
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final url = _webviewUrl();
    return Scaffold(
      appBar: AppBar(
        title: const Text('LoRA 训练台'),
        actions: [
          IconButton(
            tooltip: '刷新引擎状态',
            icon: const Icon(Icons.refresh),
            onPressed: _busy ? null : _refresh,
          ),
          if (url != null)
            IconButton(
              tooltip: '在浏览器打开 WebUI',
              icon: const Icon(Icons.open_in_new),
              onPressed: _openInBrowser,
            ),
          PopupMenuButton<String>(
            tooltip: '引擎操作',
            onSelected: (value) {
              if (value == 'start') _start();
              if (value == 'stop') _stop();
            },
            itemBuilder: (_) => [
              const PopupMenuItem(
                value: 'start',
                child: Text('启动训练引擎（守护进程 + WebUI）'),
              ),
              const PopupMenuItem(value: 'stop', child: Text('停止（不打断正在跑的训练）')),
            ],
          ),
        ],
      ),
      body: _body(theme, url),
    );
  }

  Widget _body(ThemeData theme, String? url) {
    if (_loading) {
      return const Center(child: CircularProgressIndicator());
    }
    if (_error != null && _status == null) {
      return _ErrorCard(message: _error!, onRetry: _refresh);
    }
    final status = _status;
    if (status == null) {
      return _ErrorCard(message: '拿不到训练引擎状态', onRetry: _refresh);
    }
    if (!status.installed) {
      return _NotInstalledCard(status: status, onRetry: _refresh);
    }
    if (url == null) {
      return _StartCard(
        status: status,
        busy: _busy,
        error: _error,
        onStart: _start,
        onRefresh: _refresh,
      );
    }
    return Column(
      children: [
        _StatusStrip(status: status),
        if (_error != null)
          MaterialBanner(
            content: Text(
              _error!,
              maxLines: 3,
              overflow: TextOverflow.ellipsis,
            ),
            actions: [
              TextButton(
                onPressed: () => setState(() => _error = null),
                child: const Text('知道了'),
              ),
            ],
          ),
        Expanded(
          child: kIsWeb
              ? _PlainNotice(
                  text: 'Web 版不内嵌 WebView，请用 Android / Windows 客户端打开。',
                  url: url,
                )
              : _TrainWebView(
                  // 换一次 URL / 刷新一次就整棵重建（连带换 controller）：
                  // WebView 不会因为 widget 重建就自己重新发请求。
                  key: ValueKey('train-webview-$url-$_reload'),
                  url: url,
                  onMainFrameError: (message) {
                    if (!mounted) return;
                    setState(() => _error = message);
                  },
                ),
        ),
      ],
    );
  }
}

/// 训练台 WebView 的导航判据（**纯函数**，便于单测钉住"别把外链吃进 WebView"）。
///
/// 旧实现用 `target.startsWith(baseUrl)` 判同源，那是**字符串前缀**：
/// `http://127.0.0.1:8240` 会把 `http://127.0.0.1:82401` 也放行。这里按
/// origin（scheme + host + port）比较。
enum TrainLinkAction { allow, external, block }

TrainLinkAction decideTrainNavigation({
  required String baseUrl,
  required String targetUrl,
  required bool isMainFrame,
}) {
  // 子 frame（图片 / iframe）一律放行：训练台的 /studio/* 图片不能因为
  // origin 判断被拦掉。
  if (!isMainFrame) return TrainLinkAction.allow;
  final target = Uri.tryParse(targetUrl);
  if (target == null) return TrainLinkAction.block;
  // `about:blank` / `data:` / `blob:` 是 WebView 自己的内部导航（预览、下载
  // 链接都可能是 blob:），不该被踢去浏览器。
  if (!target.hasScheme ||
      target.scheme == 'about' ||
      target.scheme == 'data' ||
      target.scheme == 'blob') {
    return TrainLinkAction.allow;
  }
  final base = Uri.tryParse(baseUrl);
  if (base != null && base.hasScheme && target.origin == base.origin) {
    return TrainLinkAction.allow;
  }
  return TrainLinkAction.external;
}

/// 内嵌 WebUI 的那一层。
///
/// 为什么单独抽一个 StatefulWidget：`WebViewController` 与 `WebViewWidget` 是
/// **一一对应**的（一个 controller 只能挂一个 widget），而刷新状态 / WebUI 换端口
/// 时必须重新发请求 —— 所以用外层给的 key 整棵重建，连带换一个 controller。
class _TrainWebView extends StatefulWidget {
  final String url;
  final void Function(String message) onMainFrameError;

  const _TrainWebView({
    super.key,
    required this.url,
    required this.onMainFrameError,
  });

  @override
  State<_TrainWebView> createState() => _TrainWebViewState();
}

class _TrainWebViewState extends State<_TrainWebView> {
  late final WebViewController _controller;

  @override
  void initState() {
    super.initState();
    _controller = WebViewController()
      ..setJavaScriptMode(JavaScriptMode.unrestricted)
      ..setNavigationDelegate(
        NavigationDelegate(
          onNavigationRequest: _onNavigationRequest,
          onWebResourceError: _onWebResourceError,
        ),
      )
      ..loadRequest(Uri.parse(widget.url));
  }

  /// 只放行训练台自己的 origin；外部链接交给系统浏览器，WebView 保持训练台。
  /// 判据在 [decideTrainNavigation]（纯函数，单测钉住）。
  Future<NavigationDecision> _onNavigationRequest(
    NavigationRequest request,
  ) async {
    final action = decideTrainNavigation(
      baseUrl: widget.url,
      targetUrl: request.url,
      isMainFrame: request.isMainFrame,
    );
    switch (action) {
      case TrainLinkAction.allow:
        return NavigationDecision.navigate;
      case TrainLinkAction.block:
        return NavigationDecision.prevent;
      case TrainLinkAction.external:
        final target = Uri.tryParse(request.url);
        if (target != null) {
          await launchUrl(target, mode: LaunchMode.externalApplication);
        }
        return NavigationDecision.prevent;
    }
  }

  void _onWebResourceError(WebResourceError error) {
    // 图片/接口的 404 不该把整页判死；只有主文档失败才提示。
    if (error.isForMainFrame == false) return;
    // Windows 上 isForMainFrame 可能是 null（平台没给）：用 URL 再对一次，
    // 免得一个子资源 404 就把整页标成"加载失败"。
    if (error.isForMainFrame == null &&
        error.url != null &&
        error.url != widget.url) {
      return;
    }
    widget.onMainFrameError('WebUI 加载失败，${error.description}');
  }

  @override
  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    // WebView 的默认背景是白的：深色主题下加载瞬间会闪一屏白。
    // 背景色要跟着主题走，而且必须在依赖就绪之后设（initState 里拿不到 Theme）。
    _controller.setBackgroundColor(Theme.of(context).colorScheme.surface);
  }

  @override
  Widget build(BuildContext context) => WebViewWidget(controller: _controller);
}

class _StatusStrip extends StatelessWidget {
  final TrainEngineStatus status;

  const _StatusStrip({required this.status});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = AppColors.of(context);
    Widget pill(String label, bool ok, String detail) => Container(
      padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 4),
      decoration: BoxDecoration(
        // 状态底色走 surface 角色 + 语义色描边（不再用 green@0.5 这种叠色）
        color: theme.colorScheme.surfaceContainer,
        borderRadius: const BorderRadius.all(Radius.circular(999)),
        border: Border.all(
          color: ok ? colors.success : theme.colorScheme.outlineVariant,
        ),
      ),
      child: Text(
        '$label · $detail',
        style: theme.textTheme.labelSmall?.copyWith(
          color: ok ? colors.success : theme.colorScheme.onSurfaceVariant,
        ),
      ),
    );

    return Container(
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(12, 8, 12, 8),
      decoration: BoxDecoration(
        color: theme.colorScheme.surfaceContainerLow,
        border: Border(
          bottom: BorderSide(color: theme.colorScheme.outlineVariant),
        ),
      ),
      child: Wrap(
        spacing: 8,
        runSpacing: 6,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          pill(
            '守护进程',
            status.daemon.up,
            status.daemon.up ? '在线 :${status.daemon.port}' : '离线',
          ),
          pill(
            'WebUI',
            status.webui.up,
            status.webui.up ? '在线 :${status.webui.port}' : '离线',
          ),
          pill('项目', true, '${status.projects} 个'),
          if (status.problems.isNotEmpty)
            Tooltip(
              message: status.problems.join('\n'),
              child: Text(
                '${status.problems.length} 个提示',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.error,
                ),
              ),
            ),
        ],
      ),
    );
  }
}

class _NotInstalledCard extends StatelessWidget {
  final TrainEngineStatus status;
  final Future<void> Function() onRetry;

  const _NotInstalledCard({required this.status, required this.onRetry});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 620),
        child: Card(
          margin: const EdgeInsets.all(20),
          child: Padding(
            padding: const EdgeInsets.all(18),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Icon(
                      Icons.school_outlined,
                      color: theme.colorScheme.primary,
                    ),
                    const SizedBox(width: 10),
                    Text('还没装训练引擎', style: theme.textTheme.titleMedium),
                  ],
                ),
                const SizedBox(height: 10),
                Text(
                  '训练是可选能力，缺了只影响“在本机训 LoRA”，不影响出图。'
                  '引擎是 anima-lora-next（train.py + .venv\\Scripts\\python.exe），'
                  '默认找 <项目>\\training\\anima_lora，也认同级目录 anima-lora-next 与 '
                  'ANIMA_LORA_ROOT / COMFYHUB_TRAIN_DIR。',
                  style: theme.textTheme.bodySmall,
                ),
                const SizedBox(height: 10),
                SelectableText(
                  status.engineDir ?? '（没有找到引擎目录）',
                  style: theme.textTheme.bodySmall?.copyWith(
                    fontFamily: 'monospace',
                  ),
                ),
                if (status.problems.isNotEmpty) ...[
                  const SizedBox(height: 10),
                  Text(
                    status.problems.join('\n'),
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: theme.colorScheme.error,
                    ),
                  ),
                ],
                const SizedBox(height: 14),
                // 窄屏（384dp）上按钮 + 一句说明放同一行会横向溢出；用 Wrap 让
                // 说明自动换到下一行。桌面宽屏仍然是一行。
                Wrap(
                  spacing: 10,
                  runSpacing: 6,
                  crossAxisAlignment: WrapCrossAlignment.center,
                  children: [
                    FilledButton.icon(
                      onPressed: onRetry,
                      icon: const Icon(Icons.refresh, size: 18),
                      label: const Text('重新探测'),
                    ),
                    Text(
                      '装好之后回到这里即可，AI 工作台也会自动多出 anima_studio_* 工具。',
                      style: theme.textTheme.labelSmall,
                    ),
                  ],
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _StartCard extends StatelessWidget {
  final TrainEngineStatus status;
  final bool busy;
  final String? error;
  final Future<void> Function() onStart;
  final Future<void> Function() onRefresh;

  const _StartCard({
    required this.status,
    required this.busy,
    required this.error,
    required this.onStart,
    required this.onRefresh,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 620),
        child: Card(
          margin: const EdgeInsets.all(20),
          child: Padding(
            padding: const EdgeInsets.all(18),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Icon(
                      Icons.play_circle_outline,
                      color: theme.colorScheme.primary,
                    ),
                    const SizedBox(width: 10),
                    Text(
                      '训练引擎已安装，WebUI 没起',
                      style: theme.textTheme.titleMedium,
                    ),
                  ],
                ),
                const SizedBox(height: 8),
                SelectableText(
                  status.engineDir ?? '',
                  style: theme.textTheme.bodySmall?.copyWith(
                    fontFamily: 'monospace',
                  ),
                ),
                const SizedBox(height: 8),
                Text(
                  '点下面的按钮会起 anima_daemon（训练队列）和 anima_lora_webui（这个页面）。'
                  '起进程走 pythonw + 无窗口，日志在 <项目>\\.run\\train-*.log。',
                  style: theme.textTheme.bodySmall,
                ),
                if (error != null) ...[
                  const SizedBox(height: 10),
                  Text(
                    error!,
                    style: TextStyle(color: theme.colorScheme.error),
                  ),
                ],
                const SizedBox(height: 14),
                Row(
                  children: [
                    FilledButton.icon(
                      onPressed: busy ? null : () => onStart(),
                      icon: busy
                          ? const SizedBox(
                              width: 16,
                              height: 16,
                              child: CircularProgressIndicator(strokeWidth: 2),
                            )
                          : const Icon(Icons.play_arrow, size: 18),
                      label: const Text('启动训练台'),
                    ),
                    const SizedBox(width: 10),
                    TextButton.icon(
                      onPressed: busy ? null : () => onRefresh(),
                      icon: const Icon(Icons.refresh, size: 17),
                      label: const Text('只刷新状态'),
                    ),
                  ],
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _ErrorCard extends StatelessWidget {
  final String message;
  final Future<void> Function() onRetry;

  const _ErrorCard({required this.message, required this.onRetry});

  @override
  Widget build(BuildContext context) {
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 560),
        child: Card(
          margin: const EdgeInsets.all(20),
          child: Padding(
            padding: const EdgeInsets.all(18),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text('连不上训练引擎', style: Theme.of(context).textTheme.titleMedium),
                const SizedBox(height: 8),
                SelectableText(message),
                const SizedBox(height: 14),
                FilledButton(onPressed: onRetry, child: const Text('重试')),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _PlainNotice extends StatelessWidget {
  final String text;
  final String url;

  const _PlainNotice({required this.text, required this.url});

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(text),
            const SizedBox(height: 8),
            SelectableText(url),
          ],
        ),
      ),
    );
  }
}
