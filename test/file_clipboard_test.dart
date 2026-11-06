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
  late bool hasImage;
  late Map<String, Object?>? image;
  Object? throwOn;

  setUp(() {
    calls = [];
    hasFiles = false;
    files = [];
    writeOk = true;
    hasImage = false;
    image = null;
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
            case 'hasImage':
              return hasImage;
            case 'readImage':
              return image;
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

  group('剪贴板里的位图（截图 / 浏览器复制图片）', () {
    test('hasImage 如实转发原生结论', () async {
      expect(await FileClipboard.hasImage(), isFalse);
      hasImage = true;
      expect(await FileClipboard.hasImage(), isTrue);
    });

    test('readImage：PNG 原样带回，格式不被改写成 bmp', () async {
      // PNG 签名 + 两个填充字节：只要不是空串，这一层不校验像素，
      // 校验的是"原生给的格式与字节**原样**到调用方手里"。
      final png = Uint8List.fromList([0x89, 0x50, 0x4E, 0x47, 1, 2, 3]);
      image = {'format': 'png', 'bytes': png};
      final got = await FileClipboard.readImage();
      expect(got, isNotNull);
      expect(got!.isPng, isTrue);
      expect(got.bytes, png);
    });

    test('readImage：bmp 也原样带回（转 PNG 是调用方的事）', () async {
      image = {
        'format': 'bmp',
        'bytes': Uint8List.fromList([1, 2, 3, 4]),
      };
      final got = await FileClipboard.readImage();
      expect(got!.isPng, isFalse);
    });

    test('readImage：没有图回 null（调用方据此退回粘文本）', () async {
      expect(await FileClipboard.readImage(), isNull);
      // 通道说"有"，但拿回来是空 / 脏值 —— 同样按"没有"处理，不许造一张空图
      image = {'format': 'png', 'bytes': Uint8List(0)};
      expect(await FileClipboard.readImage(), isNull);
      image = {'format': 'png', 'bytes': 'not-bytes'};
      expect(await FileClipboard.readImage(), isNull);
    });

    test('readImage：格式缺失时默认按 png（原生只会给这两种）', () async {
      image = {
        'bytes': Uint8List.fromList([1, 2, 3]),
      };
      final got = await FileClipboard.readImage();
      expect(got!.format, 'png');
    });

    test('★ 通道没接上时 hasImage/readImage 降级，不把粘贴弄坏', () async {
      throwOn = 'hasImage';
      expect(await FileClipboard.hasImage(), isFalse);
      throwOn = 'readImage';
      expect(await FileClipboard.readImage(), isNull);
    });
  });
}
