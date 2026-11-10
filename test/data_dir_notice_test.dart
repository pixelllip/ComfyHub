// 「数据目录提醒」横幅（`lib\widgets\data_dir_notice_banner.dart`）的回归测试。
//
// 它存在的理由：`resolveDataDir` 会把数据目录**校正**过来（指针指在 `…\storage`
// 的上一级时按实际那份执行），但"更换了库"这一事实必须可见 ——
// 那次事故里，磁盘上什么都没丢，界面却是"对话不见了、角色卡是旧版"。
//
// 三条钉住：正常时不占地方 / 有异常时把话原样说出来 / 关掉的是"这一次的问题"。

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:viewer/widgets/data_dir_notice_banner.dart';

void main() {
  Widget host({String? notice, VoidCallback? onOpenSettings}) => MaterialApp(
    home: Scaffold(
      body: DataDirNoticeBanner(notice: notice, onOpenSettings: onOpenSettings),
    ),
  );

  testWidgets('没有异常时一个像素都不占', (tester) async {
    await tester.pumpWidget(host());
    expect(find.text('数据目录提醒'), findsNothing);
    expect(
      tester.getSize(find.byType(DataDirNoticeBanner)),
      Size.zero,
      reason: '正常机器上不该有这条横幅的任何空间',
    );
  });

  testWidgets('有异常时原样显示，并且能去设置', (tester) async {
    var opened = 0;
    await tester.pumpWidget(
      host(
        notice: '库实际在 D:\\data\\storage。D:\\data 那一级的那份是旧的。',
        onOpenSettings: () => opened++,
      ),
    );

    expect(find.text('数据目录提醒'), findsOneWidget);
    expect(
      find.text('库实际在 D:\\data\\storage。D:\\data 那一级的那份是旧的。'),
      findsOneWidget,
    );

    await tester.tap(find.text('去设置'));
    expect(opened, 1);
  });

  testWidgets('关掉之后不再出现；换成另一个问题会重新出现', (tester) async {
    await tester.pumpWidget(host(notice: '这个目录里只有库，没有产物文件夹 storage。'));
    expect(find.text('数据目录提醒'), findsOneWidget);

    await tester.tap(find.byTooltip('知道了'));
    await tester.pump();
    expect(find.text('数据目录提醒'), findsNothing);

    // 同一个问题再渲染一次：仍然保持关闭
    await tester.pumpWidget(host(notice: '这个目录里只有库，没有产物文件夹 storage。'));
    expect(find.text('数据目录提醒'), findsNothing);

    // 换了一个问题（比如指针失效）：重新出现
    await tester.pumpWidget(host(notice: '上次的数据目录 D:\\gone 不在了，这次用程序目录里的数据。'));
    expect(find.text('数据目录提醒'), findsOneWidget);
    expect(find.textContaining('不在了'), findsOneWidget);
  });
}
