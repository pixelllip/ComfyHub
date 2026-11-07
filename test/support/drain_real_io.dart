// 在 widget 测试里推完**真的磁盘 IO**。
//
// `testWidgets` 用的是假时钟：`pump` 只推进假时间，`await Directory.list(...)` /
// `File.copy` / `File.delete` 这类真实 IO 的续体永远等不到。只 `runAsync` 一次也不够，
// 因为 IO 事件由**真实事件循环**投递、而续体排在假时钟的微任务队列里 —— 只有 `pump`
// 才 flush 那个队列。
//
// 所以要交替做若干轮：`runAsync`（给真实事件循环转一圈）→ `pump`（收一次微任务）。
// 触发动作之后再调它，然后正常 `pumpAndSettle` 收帧即可。

import 'package:flutter_test/flutter_test.dart';

Future<void> drainRealIo(WidgetTester tester, {int rounds = 30}) async {
  for (var i = 0; i < rounds; i++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 5)),
    );
    await tester.pump(const Duration(milliseconds: 16));
  }
}
