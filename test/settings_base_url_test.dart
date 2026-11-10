import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:viewer/core/settings_store.dart';

/// **后端端口自动发现**（不显式指定端口，启动时自动分配）。
///
/// 端口这件事只有一个真源：后端绑定成功后写的 `<项目根>\.run\api.json`。
/// 这组用例钉的是 Dart 侧读它的那条路 —— 四个读端（Dart / PowerShell / Kotlin /
/// ComfyUI 捕获扩展）中任一读取错误，界面都会显示"后端在运行、界面却连不上"。
///
/// 所以每一条都对着一个**具体的坏情形**：
///  · 没有文件 / 文件坏了 / 字段不对 → 回落默认值，**绝不抛**（它在启动路径上，抛了就是白屏）；
///  · `host=0.0.0.0` → 换成 127.0.0.1（客户端连 `0.0.0.0` 连不上，这是最容易写错的一条）；
///  · 手动填写了**其他机器**的地址 → **不得**被本机端口发现覆盖（远程 / 局域网配置）；
///  · 填写的其实是**本机**地址（设置页把自动发现的值原样存回）→ 仍按自动处理，
///    以端口记录为准（回归见下方用例）。
void main() {
  // `SettingsStore.load()` 走 SharedPreferences（设置项本身），测试里要给它一个空的内存实现。
  TestWidgetsFlutterBinding.ensureInitialized();

  late Directory root;

  setUp(() {
    // 每个用例都从"设置里什么都没有"开始：不清的话上一个用例 `setBaseUrl` 写进去的值
    // 会被下一个用例读到（mock 的 prefs 进程内共享），"填写过地址"这一状态
    // 会影响后续断言。
    SharedPreferences.setMockInitialValues({});
    root = Directory.systemTemp.createTempSync('comfyhub-port-dart');
    Directory('${root.path}${Platform.pathSeparator}.run')
        .createSync(recursive: true);
  });

  tearDown(() {
    SettingsStore.portFileRootsOverride = null;
    if (root.existsSync()) root.deleteSync(recursive: true);
  });

  test('筛选栏默认值：手机收起、桌面展开（用户 2026-10-31）', () async {
    SettingsStore.portFileRootsOverride = [root.path];
    addTearDown(() => SettingsStore.mobileFiltersDefaultOverride = null);

    SettingsStore.mobileFiltersDefaultOverride = true;
    SharedPreferences.setMockInitialValues({});
    final phone = SettingsStore();
    await phone.load();
    expect(phone.galleryFiltersCollapsed, isTrue);
    expect(phone.promptFiltersCollapsed, isTrue);

    SettingsStore.mobileFiltersDefaultOverride = false;
    SharedPreferences.setMockInitialValues({});
    final desktop = SettingsStore();
    await desktop.load();
    expect(desktop.galleryFiltersCollapsed, isFalse);
    expect(desktop.promptFiltersCollapsed, isFalse);
  });

  void writePortFile(Map<String, Object?> payload) {
    File(
      '${root.path}${Platform.pathSeparator}.run${Platform.pathSeparator}api.json',
    ).writeAsStringSync(jsonEncode(payload));
  }

  test('有端口记录时按它拼地址（这是"端口自动"能成立的关键一步）', () async {
    writePortFile({'port': 51234, 'host': '127.0.0.1', 'pid': 42});
    SettingsStore.portFileRootsOverride = [root.path];

    final settings = SettingsStore();
    await settings.load();

    expect(settings.baseUrl, 'http://127.0.0.1:51234');
    expect(settings.baseUrlIsAuto, isTrue, reason: '这是"自动发现"的地址，不是用户填的');
  });

  test('host 是 0.0.0.0（或空）时回落回环：客户端连不上通配地址', () async {
    writePortFile({'port': 50001, 'host': '0.0.0.0'});
    SettingsStore.portFileRootsOverride = [root.path];

    final settings = SettingsStore();
    await settings.load();
    expect(settings.baseUrl, 'http://127.0.0.1:50001');
  });

  test('没有记录 / 坏 JSON / 端口不合理时回落默认值，绝不抛', () async {
    SettingsStore.portFileRootsOverride = [root.path];
    final none = SettingsStore();
    await none.load();
    expect(none.baseUrl, 'http://127.0.0.1:8080', reason: '连记录都没有时用出厂默认');

    // 半截 JSON（写入过程中被读到）
    File(
      '${root.path}${Platform.pathSeparator}.run${Platform.pathSeparator}api.json',
    ).writeAsStringSync('{"port": 51');
    final broken = SettingsStore();
    await broken.load();
    expect(broken.baseUrl, 'http://127.0.0.1:8080');

    // 端口是 0 / 超出范围：那不是"记录"，是垃圾
    writePortFile({'port': 0});
    final zero = SettingsStore();
    await zero.load();
    expect(zero.baseUrl, 'http://127.0.0.1:8080');

    writePortFile({'port': 99999});
    final huge = SettingsStore();
    await huge.load();
    expect(huge.baseUrl, 'http://127.0.0.1:8080');
  });

  test('用户手填过地址时，端口发现不许动它（遥控 / 局域网配置优先）', () async {
    SettingsStore.portFileRootsOverride = [root.path];
    writePortFile({'port': 51234, 'host': '127.0.0.1'});

    final settings = SettingsStore();
    await settings.load();
    await settings.setBaseUrl('http://192.168.1.9:8080');
    expect(settings.baseUrlIsAuto, isFalse);

    // 后端换了端口（记录变了）：手填的地址一个字都不能改
    writePortFile({'port': 60001, 'host': '127.0.0.1'});
    final changed = await settings.refreshBaseUrlFromPortFile();
    expect(changed, isFalse);
    expect(settings.baseUrl, 'http://192.168.1.9:8080');
  });

  test('后端换了端口：自动模式下能被重新发现（重启后界面还能接着用）', () async {
    SettingsStore.portFileRootsOverride = [root.path];
    writePortFile({'port': 50001});
    final settings = SettingsStore();
    await settings.load();
    expect(settings.baseUrl, 'http://127.0.0.1:50001');

    writePortFile({'port': 50002});
    expect(await settings.refreshBaseUrlFromPortFile(), isTrue);
    expect(settings.baseUrl, 'http://127.0.0.1:50002');

    // 同一个端口再刷一次：不算"变了"（免得每次都去重建 api client）
    expect(await settings.refreshBaseUrlFromPortFile(), isFalse);

    // 记录消失（后端停了）：保持原样，绝不猜
    File(
      '${root.path}${Platform.pathSeparator}.run${Platform.pathSeparator}api.json',
    ).deleteSync();
    expect(await settings.refreshBaseUrlFromPortFile(), isFalse);
    expect(settings.baseUrl, 'http://127.0.0.1:50002');
  });

  test('清空地址 = 回到自动（按端口记录走）', () async {
    SettingsStore.portFileRootsOverride = [root.path];
    writePortFile({'port': 50077});
    final settings = SettingsStore();
    await settings.load();
    await settings.setBaseUrl('http://10.0.0.5:9000');
    expect(settings.baseUrlIsAuto, isFalse);

    await settings.setBaseUrl('');
    expect(settings.baseUrlIsAuto, isTrue);
    expect(settings.baseUrl, 'http://127.0.0.1:50077');
  });

  // ---------------------------------------------------------------------------
  //  "本机地址始终自动"（"直接启动后无法连接后端"的回归）
  //
  //  现场：`storage\app-support\shared_preferences.json` 里躺着
  //  `"flutter.comfyhub.baseUrl":"http://127.0.0.1:2175"`（设置页「保存并测试连接」
  //  把框里**自动发现来的**地址原样存了回去），而后端这次绑在 9610 上。
  //  旧判据为"偏好中有值 = 手动填写过"，因此 `refreshBaseUrlFromPortFile` 始终提前返回：
  //  后端活得好好的，App 对着 2175 等满 60 秒超时。
  // ---------------------------------------------------------------------------

  test('本机地址判据：回环算本机，别的机器不算', () {
    for (final local in [
      '127.0.0.1:2175',
      'http://127.0.0.1:8080',
      'http://localhost:1234',
      'http://[::1]:9000',
      'http://0.0.0.0:50001',
    ]) {
      expect(SettingsStore.isLocalBackendUrl(local), isTrue, reason: local);
    }
    for (final remote in [
      'http://192.168.1.9:8080',
      'http://10.0.2.2:8080', // 模拟器眼里的"宿主机" = 另一台设备
      'http://100.64.0.7:9000', // Tailscale
      'https://comfy.example.com',
      '',
    ]) {
      expect(SettingsStore.isLocalBackendUrl(remote), isFalse, reason: remote);
    }
  });

  test('偏好里存的是本机地址 → 当没填过，并按端口记录自愈（存量脏值）', () async {
    writePortFile({'port': 51234, 'host': '127.0.0.1'});
    SettingsStore.portFileRootsOverride = [root.path];
    SharedPreferences.setMockInitialValues({
      'flutter.comfyhub.baseUrl': 'http://127.0.0.1:2175',
    });
    // 先确认这份"脏偏好"真的喂进去了（读法与应用一致：**不带 `flutter.`**），
    // 否则下面的断言只是因为"偏好是空的"而通过 —— 那是假绿灯。
    expect(
      (await SharedPreferences.getInstance()).getString('comfyhub.baseUrl'),
      'http://127.0.0.1:2175',
    );

    final settings = SettingsStore();
    await settings.load();

    expect(
      settings.baseUrl,
      'http://127.0.0.1:51234',
      reason: '以 .run\\api.json 为准',
    );
    expect(settings.baseUrlIsAuto, isTrue);
    final prefs = await SharedPreferences.getInstance();
    // ⚠ 读的时候用**不带 `flutter.`** 的键（`getString` 自己会加前缀，见
    // shared_preferences 的 `_prefix`）—— 直接拿带前缀的串去读会读到 null，
    // 断言就变成了"永远为真"的假绿灯。
    expect(
      prefs.getString('comfyhub.baseUrl'),
      isNull,
      reason: '那条脏偏好要顺手清掉，免得下一个人以为"用户手填过"',
    );
  });

  test('「保存并测试连接」把框里的本机地址存回去 → 仍然跟着端口记录走', () async {
    SettingsStore.portFileRootsOverride = [root.path];
    writePortFile({'port': 51234, 'host': '127.0.0.1'});
    final settings = SettingsStore();
    await settings.load();

    // 设置页那个按钮：把框里（= 自动发现来的）那一串原样存一次
    await settings.setBaseUrl('http://127.0.0.1:51234');
    expect(settings.baseUrlIsAuto, isTrue, reason: '本机地址不是"用户手填的覆盖"');

    // 后端重启换了端口：必须能重新发现 —— 这是"重启后界面还能接着用"的全部依据
    writePortFile({'port': 60002, 'host': '127.0.0.1'});
    expect(await settings.refreshBaseUrlFromPortFile(), isTrue);
    expect(settings.baseUrl, 'http://127.0.0.1:60002');
  });

  test('手填一个**本机**地址也回到自动，且不进历史（历史只放别的机器）', () async {
    SettingsStore.portFileRootsOverride = [root.path];
    writePortFile({'port': 50077});
    final settings = SettingsStore();
    await settings.load();

    await settings.setBaseUrl('http://127.0.0.1:8080');
    expect(settings.baseUrl, 'http://127.0.0.1:50077');
    expect(settings.baseUrlIsAuto, isTrue);
    expect(settings.baseUrlHistory, isEmpty, reason: '回环地址进历史 = 摆一串点不通的候选');
  });

  test('历史里的回环脏值在读的时候就被洗掉，别的机器照留', () async {
    SettingsStore.portFileRootsOverride = [root.path];
    writePortFile({'port': 50077});
    SharedPreferences.setMockInitialValues({
      'flutter.comfyhub.baseUrlHistory': [
        'http://127.0.0.1:2175',
        'http://192.168.1.9:8080',
        'http://127.0.0.1:4484',
        'localhost:9959',
      ],
    });
    expect(
      (await SharedPreferences.getInstance()).getStringList(
        'comfyhub.baseUrlHistory',
      ),
      hasLength(4),
      reason: '脏历史要真的喂进去',
    );

    final settings = SettingsStore();
    await settings.load();

    expect(settings.baseUrlHistory, ['http://192.168.1.9:8080']);
    final prefs = await SharedPreferences.getInstance();
    expect(prefs.getStringList('comfyhub.baseUrlHistory'), [
      'http://192.168.1.9:8080',
    ], reason: '洗干净的也要写回磁盘，别留着那串点不通的');
  });
}
