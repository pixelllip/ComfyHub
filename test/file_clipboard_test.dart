// 剪贴板里的**文件**（Windows 的 CF_HDROP）—— Dart 这一半的行为。
//
// 原生那一半在 `windows/runner/clipboard_channel.cpp`（只靠编译验证 + 实机使用），
// 这里钉的是**调用契约**与**失败降级**，因为后者决定了它会不会把别的东西弄坏：
//
//   `Ctrl+V` 的拦截判据就是 `hasFiles()`。它要是抛异常 / 卡住，普通文本粘贴
//   就会被一起弄坏 —— 而"粘不上字"这种症状，用户根本不会往剪贴板通道上想。

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/core/file_clipboard.dart';

const MethodChannel _channel = MethodChannel('comfyhub/clipboard');

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late List<MethodCall> calls;
  late bool hasFiles;
  late List<String> files;
  late bool writeOk;
  Object? throwOn;

  setUp(() {
    calls = [];
    hasFiles = false;
    files = [];
    writeOk = true;
    throwOn = null;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(_channel, (call) async {
      calls.add(call);
      if (throwOn == call.method) {
        throw PlatformException(code: 'channel-error');
      }
      switch (call.method) {
        case 'hasFiles':
          return hasFiles;
        case 'readFiles':
          return files;
        case 'writeFiles':
          return writeOk;
      }
      return null;
    });
  });

  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(_channel, null);
  });

  test('hasFiles 如实转发原生结论（这是 Ctrl+V 拦截的唯一判据）', () async {
    expect(await FileClipboard.hasFiles(), isFalse);
    hasFiles = true;
    expect(await FileClipboard.hasFiles(), isTrue);
    expect(calls.last.method, 'hasFiles');
  });

  test('readFiles 只取字符串，脏值直接丢掉', () async {
    files = [r'C:\a\b.png', r'D:\c\d.webp'];
    expect(await FileClipboard.readFiles(), [r'C:\a\b.png', r'D:\c\d.webp']);

    files = [];
    expect(await FileClipboard.readFiles(), isEmpty, reason: '剪贴板里没文件就回空表');
  });

  test('writeFiles 把路径列表原样传给原生', () async {
    writeOk = true;
    expect(await FileClipboard.writeFiles([r'C:\a\b.png']), isTrue);
    final call = calls.single;
    expect(call.method, 'writeFiles');
    expect(call.arguments, [r'C:\a\b.png']);
  });

  test('空列表不发请求（没什么可复制的）', () async {
    expect(await FileClipboard.writeFiles(const []), isFalse);
    expect(calls, isEmpty);
  });

  test('★ 通道抛异常时一律降级，绝不把异常漏给调用方', () async {
    // 这三条是"别把普通粘贴弄坏"的底线：
    // hasFiles 抛 → 当没有文件（放行，让文本框自己粘）；
    // readFiles 抛 → 当没有文件；
    // writeFiles 抛 → 如实回 false（界面据此提示"改用复制文件地址"）。
    throwOn = 'hasFiles';
    expect(await FileClipboard.hasFiles(), isFalse);
    throwOn = 'readFiles';
    expect(await FileClipboard.readFiles(), isEmpty);
    throwOn = 'writeFiles';
    expect(await FileClipboard.writeFiles([r'C:\a.png']), isFalse);
  });
}
