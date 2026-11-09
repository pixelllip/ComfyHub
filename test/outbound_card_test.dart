import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:viewer/models/models.dart';
import 'package:viewer/widgets/outbound_card.dart';

/// 设置 → 本地服务 → 「联网代理」那张卡。
///
/// 为什么单独测它：这一节只挂在 Windows 桌面端那页子设置里（`SettingsScaffold` +
/// 后端在跑才有数据），整页用例太贵；而它的**文案**就是这次修复的一半
/// （"公网到底走不走代理"得一眼看出来），值得钉住。
///
/// 形态按 AGENTS §6.0：360（窗口最窄那档）不许溢出。
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const proxyInfo = OutboundInfo(
    route: 'proxy',
    proxy: '127.0.0.1:7890',
    source: 'system',
    systemEnabled: true,
    systemServer: '127.0.0.1:7890',
    overrides: ['localhost', '127.*', '192.168.*', '<local>'],
  );

  const directInfo = OutboundInfo(route: 'direct', source: 'none');

  Future<void> pumpCard(
    WidgetTester tester, {
    OutboundInfo? info,
    bool loading = false,
    bool probing = false,
    String? error,
    double width = 360,
    VoidCallback? onRefresh,
    VoidCallback? onProbe,
  }) async {
    tester.view.physicalSize = Size(width * 2, 1200);
    tester.view.devicePixelRatio = 2;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            padding: const EdgeInsets.all(16),
            child: OutboundCard(
              info: info,
              loading: loading,
              probing: probing,
              error: error,
              onRefresh: onRefresh,
              onProbe: onProbe,
            ),
          ),
        ),
      ),
    );
  }

  testWidgets('走代理时说出地址与来源，并说明内网直连', (tester) async {
    await pumpCard(tester, info: proxyInfo);
    expect(find.text('公网请求走 HTTP 代理 127.0.0.1:7890'), findsOneWidget);
    expect(find.textContaining('来自系统的代理设置'), findsOneWidget);
    expect(find.textContaining('本机与局域网地址永远直连'), findsOneWidget);
    // 例外列表要列出来（用户能对着系统设置核对）
    expect(find.textContaining('这些地址直连'), findsOneWidget);
  });

  testWidgets('直连时说清"没找到代理设置"，并给出可操作的下一步', (tester) async {
    await pumpCard(tester, info: directInfo);
    expect(find.text('公网请求直连，没有走代理'), findsOneWidget);
    expect(find.textContaining('COMFYHUB_HTTPS_PROXY'), findsOneWidget);
  });

  testWidgets('PAC 读不了要如实说，不能装成"代理生效了"', (tester) async {
    await pumpCard(
      tester,
      info: const OutboundInfo(
        route: 'direct',
        source: 'none',
        autoConfigUrl: 'http://wpad/wpad.dat',
        warning: '系统的联网设置用的是自动配置脚本（PAC），这个脚本本项目读不了。',
      ),
    );
    expect(find.textContaining('自动配置脚本（PAC）'), findsOneWidget);
  });

  testWidgets('探针结果区分"对方回了"与"没连上"', (tester) async {
    await pumpCard(
      tester,
      info: const OutboundInfo(
        route: 'proxy',
        proxy: '127.0.0.1:7890',
        source: 'system',
        probes: [
          OutboundProbe(
            host: 'civitai.com',
            via: 'proxy',
            route: 'HTTP 代理 127.0.0.1:7890',
            status: 200,
            elapsedMs: 2100,
          ),
          OutboundProbe(
            host: 'danbooru.donmai.us',
            via: 'proxy',
            route: 'HTTP 代理 127.0.0.1:7890',
            status: 503,
            elapsedMs: 900,
          ),
          OutboundProbe(
            host: 'example.com',
            via: 'direct',
            route: '直连（DIRECT）',
            error: '连接超时',
            elapsedMs: 5000,
          ),
        ],
      ),
    );
    expect(find.textContaining('civitai.com 返回了 HTTP 200'), findsOneWidget);
    expect(find.textContaining('danbooru.donmai.us 返回了 HTTP 503'), findsOneWidget);
    expect(find.textContaining('example.com 没连上'), findsOneWidget);
    // 那句"5xx 是对方回的"的判据要在界面上
    expect(find.textContaining('收到 4xx / 5xx 说明网络是通的'), findsOneWidget);
  });

  testWidgets('测试出网期间按钮禁用，点一下会回调', (tester) async {
    var probed = 0;
    await pumpCard(tester, info: proxyInfo, onProbe: () => probed++);
    await tester.tap(find.text('测试出网'));
    expect(probed, 1);

    await pumpCard(
      tester,
      info: proxyInfo,
      probing: true,
      onProbe: () => probed++,
    );
    expect(find.text('正在测试…'), findsOneWidget);
    await tester.tap(find.text('正在测试…'));
    expect(probed, 1, reason: '正在测试时按钮必须是禁用的');
  });

  testWidgets('加载失败也要说人话，不能空白', (tester) async {
    await pumpCard(tester, error: '无法连接后端');
    expect(find.textContaining('读取失败，无法连接后端'), findsOneWidget);
  });

  testWidgets('320 宽也不溢出', (tester) async {
    await pumpCard(tester, info: proxyInfo, width: 320);
    expect(tester.takeException(), isNull);
  });
}
