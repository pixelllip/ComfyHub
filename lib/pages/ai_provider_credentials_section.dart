import 'package:flutter/material.dart';

import '../core/ai_api_client.dart';
import '../core/api_client.dart' show ApiException;
import '../core/credential_store.dart';
import '../core/inbound_auth_api.dart';
import '../core/pairing_token.dart';
import '../core/provider_store.dart';
import '../models/ai_models.dart';
import 'ai_provider_widgets.dart';

/// 「凭据」子页：连接测试 + API Key 的保存 / 移除 / 推给桌面。
///
/// 从 `ai_provider_settings_page.dart`（当时 2,956 行）拆出来的第三块。
/// 四条不能破的规则仍然守着这里：
///  1. **API Key 输入框每次打开都是空的**（AIH-012），界面上只显示"已配置 / 未配置 / 来源"；
///     留空保存表示"不改已有密钥"，清除必须点「移除密钥」。
///  2. 保存成功要**说出来存在哪**（本机密钥库 / 后端托管）；
///  3. 「推给桌面」先确认，本机没有明文就**绝不发空值**去覆盖桌面上那把好的；
///  4. 连接测试失败要说人话，不美化。
class AiProviderCredentialsSection extends StatefulWidget {
  final AiApiClient api;
  final CredentialStore credentials;

  /// 连接测试 / 获取模型能不能做，看这一端自己支不支持探测（手机侧还没实现）。
  final ProviderStore providers;
  final InboundAuthApi Function(String baseUrl)? authApiFactory;
  final AiProvider provider;
  final Future<void> Function(String message) onChanged;

  const AiProviderCredentialsSection({
    super.key,
    required this.api,
    required this.credentials,
    required this.providers,
    this.authApiFactory,
    required this.provider,
    required this.onChanged,
  });

  @override
  State<AiProviderCredentialsSection> createState() =>
      _AiProviderCredentialsSectionState();
}

class _AiProviderCredentialsSectionState
    extends State<AiProviderCredentialsSection> {
  /// 密钥输入框**永远是空字符串**：只写不读，不留任何回显可能（AIH-012）。
  final _key = TextEditingController();
  bool _busy = false;
  AiProviderTestResult? _testResult;

  /// 本组件内的反馈统一走 SnackBar：任何一步失败都要看得见。
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
  void dispose() {
    _key.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = widget.provider;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Text('连接测试', style: theme.textTheme.titleSmall),
            const SizedBox(width: 10),
            OutlinedButton.icon(
              // 手机上**还不能**：这一步要由本机自己向上游发请求（用本机密钥 + 端点信任档那套），
              // 而桌面那一份 `provider_probe` 手机侧还没实现。禁用 + 下面那句话，
              // 比让用户点下去拿到一句"Provider 不存在: x"（桌面不认识手机这条 id）诚实得多。
              onPressed: (_busy || !widget.providers.supportsProbe)
                  ? null
                  : _test,
              icon: const Icon(Icons.network_check, size: 18),
              label: const Text('测试连接'),
            ),
          ],
        ),
        if (!widget.providers.supportsProbe)
          Text(
            '连接测试与「获取模型」目前只在桌面可用，因为要由那一端自己向上游发请求。'
            '这台手机上还做不到，所以这两个按钮先禁用着。',
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
          )
        else
          Text(
            '测试只发一次请求，10 秒超时，密钥不会进日志。',
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
          ),
        if (_testResult != null) ...[
          const SizedBox(height: 8),
          AiNote(text: _testResult!.display, error: !_testResult!.ok),
        ],
        const SizedBox(height: 16),
        Text('API Key', style: theme.textTheme.titleSmall),
        const SizedBox(height: 6),
        Row(
          children: [
            AiCredentialDot(status: p.credential),
            const SizedBox(width: 6),
            Text(p.credential.label, style: theme.textTheme.bodySmall),
          ],
        ),
        const SizedBox(height: 8),
        if (p.credential.source == 'env')
          Text(
            '该凭据由环境变量 ${p.credentialRef} 提供，是只读的，不能在应用内修改或移除。',
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
          )
        else ...[
          // 手机上密钥存在**本机**（Android Keystore），桌面那份不会同步过来 ——
          // 不说清楚，用户会以为"桌面配过就够了"，然后每发一条都失败（M6）
          if (widget.credentials.isLocal)
            Padding(
              padding: const EdgeInsets.only(bottom: 8),
              child: Text(
                '这一条保存在本机密钥库，只在这台手机上生效。想让桌面也用这一把，点下面的「推给桌面」（值会离开本机，桌面必须已允许这台设备改密钥）。反过来「从桌面同步密钥」在「远程访问与配对」那张卡里。',
                style: theme.textTheme.labelSmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
            ),
          TextField(
            controller: _key,
            obscureText: true,
            autocorrect: false,
            enableSuggestions: false,
            decoration: const InputDecoration(
              labelText: '粘贴密钥（留空表示不修改已有密钥）。',
              helperText: '粘贴你的密钥',
              border: OutlineInputBorder(),
              isDense: true,
            ),
          ),
          const SizedBox(height: 8),
          // 手机上 360dp 宽时三个按钮一行放不下（实测溢出 52px）—— 用 Wrap
          // 放不下就折行，按钮一个都不能被裁掉（AGENTS §6.1 / §6.0）。
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              FilledButton.icon(
                onPressed: _busy ? null : _saveKey,
                icon: const Icon(Icons.key, size: 18),
                label: const Text('保存密钥'),
              ),
              OutlinedButton.icon(
                onPressed: _busy || !p.credential.configured
                    ? null
                    : _removeKey,
                icon: const Icon(Icons.delete_outline, size: 18),
                label: const Text('移除密钥'),
              ),
              // 密钥上行（§14.2）：只有"手机上、且有 ref、且本机真有这把"才出现。
              // 桌面自己就是真源，没有"推给桌面"这回事（`RemoteCredentialStore.reveal` 直接抛）。
              if (widget.credentials.isLocal &&
                  (p.credentialRef ?? '').trim().isNotEmpty)
                OutlinedButton.icon(
                  onPressed: _busy || !p.credential.configured
                      ? null
                      : _pushKeyToDesktop,
                  icon: const Icon(Icons.upload_outlined, size: 18),
                  label: const Text('推给桌面'),
                ),
            ],
          ),
        ],
      ],
    );
  }

  Future<void> _test() async {
    if (!widget.providers.supportsProbe) {
      // 兜底（按钮已经禁用了）：真被调到也要说人话，别去撞桌面那条不存在的 id
      _toast('手机上还没有"连接测试"，这一步要由本机自己向上游发请求，本批未实现。', error: true);
      return;
    }
    setState(() {
      _busy = true;
      _testResult = null;
    });
    try {
      final result = await widget.api.testProvider(widget.provider.id);
      if (mounted) setState(() => _testResult = result);
      _toast(
        result.ok ? '连接正常，${result.message}' : '连接失败，${result.display}',
        error: !result.ok,
      );
    } catch (e) {
      _toast('连接测试失败，$e', error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _saveKey() async {
    final value = _key.text;
    if (value.trim().isEmpty) {
      _toast('密钥留空就是不修改，要清除请点「移除密钥」。');
      return;
    }
    setState(() => _busy = true);
    try {
      final status = await widget.credentials.save(widget.provider, value);
      _key.clear(); // 立刻丢弃，不在内存里多留一秒
      final where = widget.credentials.isLocal ? '本机密钥库' : status.source;
      await widget.onChanged('密钥已保存（$where）。值不会再被读回。');
      _toast('密钥已保存（$where），值不会再被读回。');
    } catch (e) {
      _toast('保存密钥失败，$e', error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _removeKey() async {
    setState(() => _busy = true);
    try {
      await widget.credentials.clear(widget.provider);
      await widget.onChanged('密钥已移除。');
      _toast('密钥已移除。');
    } catch (e) {
      _toast('移除失败，$e', error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// **把本机这一把推给桌面**（`docs/pairing-design.md` §14.2 密钥上行）。
  ///
  /// 四步，一步都不能省：
  ///  ① **先问一次**：值会离开本机，这是用户该知情的事；
  ///  ② `reveal` 取明文（**全仓唯一允许读密钥的地方**）：本机没有就如实说"没有可推的"，
  ///     **绝不拿空串去覆盖桌面上那把好的**；
  ///  ③ `PUT /api/ai/credentials`，带上配对令牌；`rev` **不带** = 用户显式点的强制覆盖
  ///     （带 rev 才是"等值才应用"，见 §14.3）；
  ///  ④ 回执**逐条如实**：`applied` 说成功、`rejected` 说清是哪一种（开关没开 / 桌面更新过 /
  ///     名字不合法），旧后端（404/501）直说"还没有这条接口"。
  Future<void> _pushKeyToDesktop() async {
    final provider = widget.provider;
    final ref = provider.credentialRef?.trim() ?? '';
    if (ref.isEmpty) return;

    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('把这一把密钥推给桌面？'),
        content: Text(
          '「$ref」这把 key 会离开本机，写到桌面那一份凭据里 —— 桌面以后就用它发请求。\n\n'
          '桌面必须已经允许这台设备改密钥，否则会被拒（在桌面上的设备卡片里勾选）。',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('推上去'),
          ),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;

    setState(() => _busy = true);
    InboundAuthApi? owned;
    try {
      final value = await widget.credentials.reveal(provider);
      if (value == null || value.isEmpty) {
        _toast('本机没有这把密钥（没有可推的）—— 推空值会把桌面上那把好的覆盖掉，所以这里不发请求。', error: true);
        return;
      }
      final factory = widget.authApiFactory;
      final auth = factory != null
          ? factory(widget.api.baseUrl)
          : (owned = InboundAuthApi(widget.api.baseUrl));
      final CredentialUploadResult result;
      try {
        result = await auth.uploadCredentials({
          ref: value,
        }, token: PairingTokenCache.read());
      } finally {
        // 自己建的客户端自己收；注入进来的（测试/MockClient）不许关
        owned?.dispose();
      }
      if (result.applied.contains(ref)) {
        await widget.onChanged('「$ref」已推到桌面。');
        _toast('已推给桌面 $ref');
      } else if (result.rejected.isNotEmpty) {
        final r = result.rejected.firstWhere(
          (e) => e.ref == ref,
          orElse: () => result.rejected.first,
        );
        _toast('桌面拒收「${r.ref}」，${r.hint}', error: true);
      } else {
        // 既没说成功也没说被拒 —— 如实说"桌面没回这一条"，别假装成功
        _toast('桌面既没回成功也没回被拒（回执里没有 $ref）—— 别当它成了。', error: true);
      }
    } on ApiException catch (e) {
      if (e.statusCode == 404 || e.statusCode == 501) {
        _toast('这台后端还没有"上行密钥"的接口（旧版本）—— 升级桌面后端再试。', error: true);
      } else {
        _toast('推给桌面失败，${e.message}', error: true);
      }
    } catch (e) {
      _toast('推给桌面失败，$e', error: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }
}
