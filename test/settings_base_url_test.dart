import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:viewer/core/settings_store.dart';

/// **后端端口自动发现**（用户 2026-09-22："不再显式指定端口，启动时自动挑一个"）。
///
/// 端口这件事只有一个真源：后端绑定成功后写的 `<项目根>\.run\api.json`。
/// 这组用例钉的是 Dart 侧读它的那条路 —— 四个读端（Dart / PowerShell / Kotlin /
/// ComfyUI 捕获扩展）里只要有一个读错，用户看到的就是"后端明明在跑、界面却连不上"。
///
/// 所以每一条都对着一个**具体的坏情形**：
///  · 没有文件 / 文件坏了 / 字段不对 → 回落默认值，**绝不抛**（它在启动路径上，抛了就是白屏）；
///  · `host=0.0.0.0` → 换成 127.0.0.1（客户端连 `0.0.0.0` 连不上，这是最容易写错的一条）；
///  · 用户手填过地址 → **不许**被本机端口发现覆盖（那是远程 / 局域网配置）。
void main() {
  // `SettingsStore.load()` 走 SharedPreferences（设置项本身），测试里要给它一个空的内存实现。
  TestWidgetsFlutterBinding.ensureInitialized();

  late Directory root;

  setUp(() {
    // 每个用例都从"设置里什么都没有"开始：不清的话上一个用例 `setBaseUrl` 写进去的值
    // 会被下一个用例读到（mock 的 prefs 是进程内共享的），于是"用户手填过地址"这条
    // 会悄悄影响后面的断言。
    SharedPreferences.setMockInitialValues({});
    root = Directory.systemTemp.createTempSync('comfyhub-port-dart');
    Directory('${root.path}${Platform.pathSeparator}.run').createSync(recursive: true);
  });

  tearDown(() {
    SettingsStore.portFileRootsOverride = null;
    if (root.existsSync()) root.deleteSync(recursive: true);
  });

  void writePortFile(Map<String, Object?> payload) {
    File('${root.path}${Platform.pathSeparator}.run${Platform.pathSeparator}api.json')
        .writeAsStringSync(jsonEncode(payload));
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
    File('${root.path}${Platform.pathSeparator}.run${Platform.pathSeparator}api.json')
        .writeAsStringSync('{"port": 51');
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
    File('${root.path}${Platform.pathSeparator}.run${Platform.pathSeparator}api.json').deleteSync();
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
}
