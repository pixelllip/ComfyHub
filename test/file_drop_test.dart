// 拖进窗口的文件（通道 `comfyhub/file_drop`）—— Dart 这一半的行为。
//
// 方向是 **native → Dart**（原生收到 `WM_DROPFILES` 主动调 `droppedFiles`），所以
// 这里不像 `file_clipboard_test.dart` 那样 mock **出站**调用，而是用
// `handlePlatformMessage` **假造一条从原生来的消息**，钉住"收到之后怎么办"：
//
//   * 路径（尤其**中文 / 非 BMP 字符**）要原样进流 —— 这是最容易错的一档：
//     UTF-8 转错就成了乱码，而乱码照样是个合法字符串，**一个错都不报**；
//   * 空列表 / 参数畸形 / 别的方法名 / 非 Windows —— 一律静默降级，
//     绝不能抛到 UI 上（抛出去会砸在 AI 工作台那条聊天链路上）；
//   * 广播流：多个页面同时听、同一条通道只有一个接收者。
//
// 原生那一半（`DragAcceptFiles` / `WM_DROPFILES` 两趟 `DragQueryFileW` /
// `DragFinish` / UTF-16→UTF-8）只能靠**编译 + 实机拖一次**验证 ——
// 见 `windows/runner/file_drop_channel.cpp` 的注释。

import 'package:flutter/services.dart'; // 顺带带出 ByteData（不另引 dart:typed_data）
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/core/file_drop.dart';

const StandardMethodCodec _codec = StandardMethodCodec();

/// 假造一条从原生来的方法调用，回原生收到的那个**回复信封**。
Future<ByteData?> _push(Object? arguments, {String method = 'droppedFiles'}) {
  return TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
      .handlePlatformMessage(
        'comfyhub/file_drop',
        _codec.encodeMethodCall(MethodCall(method, arguments)),
        null,
      );
}

/// 回复必须是**成功信封**（首字节 0）。
///
/// 这一条比 `expect(_push(...), completes)` 硬得多：`MethodChannel` 会把接收者抛出的
/// 异常**自己吞掉**、包成一个错误信封（首字节 1）—— 所以"它抛没抛"只能从回复里看
/// 出来，光看 future 完成是看不出来的。
void _expectSuccessReply(ByteData? reply) {
  expect(reply, isNotNull, reason: '通道上没有接收者，或者接收者没回话');
  expect(reply!.getUint8(0), 0, reason: '首字节非 0 = 接收者抛了，被包成了错误信封');
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late bool Function() originalProbe;

  setUp(() {
    originalProbe = FileDrop.platformProbe;
    FileDrop.platformProbe = () => true; // 绝大多数用例按"就在 Windows 上"跑
  });

  tearDown(() {
    FileDrop.platformProbe = originalProbe;
  });

  test('★ 一批路径原样进流（中文 / 空格 / 非 BMP 字符一个都不许变形）', () async {
    final received = <List<String>>[];
    addTearDown(FileDrop.stream.listen(received.add).cancel);

    const paths = <String>[
      r'D:\图片素材\猫咪 🐱.png',
      r'D:\My Docs\说明 书.webp',
      r'C:\a\b.jpg',
    ];
    _expectSuccessReply(await _push(<String, Object?>{'paths': paths}));
    await pumpEventQueue();

    expect(received, [paths], reason: '顺序与内容都要原样（路径是别的进程给的，不许加工）');
  });

  test('广播流：两个页面同时听，同一批各收一次（接收者只有一个）', () async {
    final first = <List<String>>[];
    final second = <List<String>>[];
    addTearDown(FileDrop.stream.listen(first.add).cancel);
    addTearDown(FileDrop.stream.listen(second.add).cancel);

    _expectSuccessReply(
      await _push(<String, Object?>{
        'paths': [r'C:\one.png'],
      }),
    );
    await pumpEventQueue();

    expect(first, [
      [r'C:\one.png'],
    ]);
    expect(second, [
      [r'C:\one.png'],
    ], reason: '第二个听众也得收到；只到一边 = 接收者被后装的那个顶掉了');
  });

  test('空 paths（或没有 paths 键）什么都不推 —— 别让监听方白跑一趟', () async {
    final received = <List<String>>[];
    addTearDown(FileDrop.stream.listen(received.add).cancel);

    _expectSuccessReply(await _push(<String, Object?>{'paths': <String>[]}));
    _expectSuccessReply(await _push(<String, Object?>{}));
    await pumpEventQueue();

    expect(received, isEmpty);
  });

  test('混着脏值时只取字符串（不猜、不修）', () async {
    final received = <List<String>>[];
    addTearDown(FileDrop.stream.listen(received.add).cancel);

    _expectSuccessReply(
      await _push(<String, Object?>{
        'paths': <Object?>[r'D:\ok.png', 3, true, null, <String, Object?>{}],
      }),
    );
    await pumpEventQueue();

    expect(received, [
      [r'D:\ok.png'],
    ]);
  });

  test('★ 参数畸形 / 别的方法名：不回错误信封，也不往流里推', () async {
    final received = <List<String>>[];
    addTearDown(FileDrop.stream.listen(received.add).cancel);

    final malformed = <Object?>[
      null,
      42,
      'D:\\a.png', // 不是 map
      <Object?>[r'D:\a.png'], // 是 list 但不是 map
      <String, Object?>{'paths': 'D:\\a.png'}, // paths 不是 list
      <String, Object?>{'paths': 7},
      <String, Object?>{
        'path': <String>[r'D:\a.png'],
      }, // 键名写错
      <String, Object?>{
        'paths': <Object?>[1, true, null],
      }, // 元素全不是字符串
    ];
    for (final arguments in malformed) {
      _expectSuccessReply(await _push(arguments));
    }
    // 方法名无法识别（通道只接受 `droppedFiles`）——同样不报错、不推送
    _expectSuccessReply(
      await _push(<String, Object?>{
        'paths': [r'D:\a.png'],
      }, method: 'other'),
    );
    await pumpEventQueue();

    expect(received, isEmpty);
  });

  test('★ 非 Windows：supported == false，消息真打进来也不推', () async {
    FileDrop.platformProbe = () => false;
    expect(FileDrop.supported, isFalse);

    final received = <List<String>>[];
    // 拿流本身不许抛（"通道不通"那类降级都在这条路径上）
    addTearDown(FileDrop.stream.listen(received.add).cancel);

    // 这一条**与用例顺序无关**：若本用例先执行，接收者未安装（回复为 `null`）；
    // 如果别的用例先跑过、接收者已经装上了，`supported` 在每一批里还会再判一次。
    // 两种情况都必须是"什么都不出"，所以这里不去断言回复的形状。
    await _push(<String, Object?>{
      'paths': [r'D:\a.png'],
    });
    await pumpEventQueue();

    expect(received, isEmpty);
  });
}
