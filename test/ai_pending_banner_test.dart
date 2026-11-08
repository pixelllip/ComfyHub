import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:viewer/models/ai_models.dart';
import 'package:viewer/widgets/ai_pending_banner.dart';

Widget _host(Widget child) => MaterialApp(home: Scaffold(body: child));

void main() {
  testWidgets('没有待审批：整块不占位', (tester) async {
    await tester.pumpWidget(
      _host(AiPendingBanner(approvals: const [], onOpen: (_) {})),
    );

    expect(find.byKey(const Key('ai-pending-banner')), findsNothing);
    expect(find.text('去处理'), findsNothing);
  });

  testWidgets('有待审批：显示条数与「手机排队」提醒，点「去处理」跳到第一条', (tester) async {
    final approvals = [
      const AiPendingApproval(
        callId: 'submit:q1',
        conversationId: 'c1',
        name: 'comfy_submit',
      ),
      const AiPendingApproval(
        callId: 'call_2',
        conversationId: 'c2',
        name: 'read_file',
      ),
    ];
    AiPendingApproval? opened;

    await tester.pumpWidget(
      _host(AiPendingBanner(approvals: approvals, onOpen: (a) => opened = a)),
    );

    expect(find.byKey(const Key('ai-pending-banner')), findsOneWidget);
    expect(find.textContaining('2 项待审批'), findsOneWidget);
    expect(find.textContaining('1 项是手机排队的提交'), findsOneWidget);

    await tester.tap(find.byKey(const Key('ai-pending-banner-open')));
    expect(opened?.callId, 'submit:q1');
  });
}
