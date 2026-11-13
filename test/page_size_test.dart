import 'dart:ui' show Size;

import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:viewer/core/page_size.dart';
import 'package:viewer/core/settings_store.dart';

void main() {
  test('自动每页数量：窗口越大排得越多，且夹在 8~120', () {
    final small = autoPageSizeFor(const Size(1000, 800));
    final large = autoPageSizeFor(const Size(2200, 1400));
    expect(large, greaterThan(small));
    for (final value in [small, large]) {
      expect(value, inInclusiveRange(8, 120));
    }
  });

  test('自动每页数量：窄窗口不扣侧边栏，极小窗口兜底 8 条', () {
    // 800 宽在 900 以下，是底部导航形态，内容区不扣 80 的侧栏。
    // 高度只够放 3 整排（第 4 排会被窗口底部切掉），所以是 4×3=12。
    expect(autoPageSizeFor(const Size(800, 800)), 12);
    expect(autoPageSizeFor(const Size(360, 360)), 8);
  });

  test('每页数量模式默认自动，认不出来的值也回自动', () async {
    SharedPreferences.setMockInitialValues({});
    final settings = SettingsStore();
    await settings.load();
    expect(settings.pageSizeMode, PageSizeMode.auto);
    expect(PageSizeMode.parse('bogus'), PageSizeMode.auto);
    expect(PageSizeMode.parse('fixed'), PageSizeMode.fixed);
  });

  test('每页数量模式写进偏好，重启后仍是固定', () async {
    SharedPreferences.setMockInitialValues({
      'comfyhub.pageSizeMode': 'fixed',
      'comfyhub.pageSize': 40,
    });
    final settings = SettingsStore();
    await settings.load();
    expect(settings.pageSizeMode, PageSizeMode.fixed);
    expect(settings.pageSize, 40);
  });
}
