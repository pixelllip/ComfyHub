import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:viewer/core/media_cache.dart';
import 'package:viewer/widgets/media_cache_card.dart';

/// 设置页「产物缓存」那张卡。
///
/// 为什么单独测它：那一节**只在手机上渲染**（`!isWindowsDesktop` 且非 Web），
/// 而 `flutter test` 跑在 Windows 上 —— 留在 `settings_page.dart` 里就一条用例
/// 都覆盖不到，"320dp 会不会溢出"就只能等真机发现。
///
/// 形态按 AGENTS §6.0：384（OnePlus 8T 实测）与 320（窗口最窄那档）两个宽度都要过。
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const stats = MediaCacheStats(
    thumbBytes: 100 * 1024 * 1024,
    origBytes: 900 * 1024 * 1024,
    thumbCount: 1200,
    origCount: 34,
    thumbBudget: 150 * 1024 * 1024,
    origBudget: 1536 * 1024 * 1024,
  );

  Future<void> pumpCard(
    WidgetTester tester, {
    required Size logical,
    MediaCacheStats? data = stats,
    int budgetMb = 1536,
    bool busy = false,
    ValueChanged<int>? onBudgetChanged,
    VoidCallback? onClear,
  }) async {
    tester.view.physicalSize = Size(logical.width * 2, logical.height * 2);
    tester.view.devicePixelRatio = 2;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            padding: const EdgeInsets.all(16),
            child: MediaCacheCard(
              stats: data,
              budgetMb: budgetMb,
              busy: busy,
              onBudgetChanged: onBudgetChanged ?? (_) {},
              onClear: onClear ?? () {},
            ),
          ),
        ),
      ),
    );
    // 不用 `pumpAndSettle`：`busy` / "正在统计" 那两态里有**无限循环**的进度圈，
    // settle 会一直等下去（用例超时）。这里只推两帧就够渲染。
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));
  }

  testWidgets('384dp：占用 / 上限 / 分项都写出来（溢出会直接抛，不用自己断言）', (tester) async {
    await pumpCard(tester, logical: const Size(384, 853));

    expect(find.text('已用 1000 MB / 上限 1.6 GB'), findsOneWidget);
    expect(find.textContaining('缩略图 100 MB（1200 张'), findsOneWidget);
    expect(find.textContaining('原件 900 MB（34 条）'), findsOneWidget);
    expect(find.text('原件上限'), findsOneWidget);
    expect(find.text('清理缓存'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('320dp：最窄那档也不溢出（下拉与按钮折行）', (tester) async {
    await pumpCard(tester, logical: const Size(320, 853));

    expect(find.text('清理缓存'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('还没统计完：只显示"正在统计"，清理按钮禁用', (tester) async {
    await pumpCard(tester, logical: const Size(384, 853), data: null);

    expect(find.text('正在统计…'), findsOneWidget);
    final button = tester.widget<OutlinedButton>(
      find.ancestor(
        of: find.text('清理缓存'),
        matching: find.byType(OutlinedButton),
      ),
    );
    expect(button.onPressed, isNull, reason: '还不知道占用多少就不该让人点清理');
  });

  testWidgets('换档位会把 MB 值交出去（512 / 1 GB / 1.5 GB / 2 GB / 4 GB）', (
    tester,
  ) async {
    int? picked;
    await pumpCard(
      tester,
      logical: const Size(384, 853),
      onBudgetChanged: (v) => picked = v,
    );

    await tester.tap(find.byType(DropdownButton<int>));
    await tester.pumpAndSettle();
    expect(find.text('512 MB'), findsWidgets);
    expect(find.text('4 GB'), findsOneWidget);
    await tester.tap(find.text('2 GB').last);
    await tester.pumpAndSettle();

    expect(picked, 2048);
  });

  testWidgets('忙的时候下拉与清理都不能点（避免两次操作交叉）', (tester) async {
    await pumpCard(tester, logical: const Size(384, 853), busy: true);

    final button = tester.widget<OutlinedButton>(
      find.ancestor(
        of: find.text('清理缓存'),
        matching: find.byType(OutlinedButton),
      ),
    );
    expect(button.onPressed, isNull);
    final dropdown = tester.widget<DropdownButton<int>>(
      find.byType(DropdownButton<int>),
    );
    expect(dropdown.onChanged, isNull);
  });
}
