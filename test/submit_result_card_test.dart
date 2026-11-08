import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:viewer/models/ai_models.dart';
import 'package:viewer/widgets/submit_result_card.dart';

AiMessage _reply({
  required bool ok,
  required String text,
  List<int> mediaIds = const [],
  String? code,
}) => AiMessage(
  id: 'submit-reply-q1',
  conversationId: 'c1',
  seq: 1,
  role: 'assistant',
  status: 'complete',
  text: text,
  parts: [
    AiMessagePart(
      type: 'tool_result',
      payload: {
        'name': 'comfy_submit',
        'ok': ok,
        'code': ?code,
        'mediaIds': mediaIds,
        'queueId': 'q1',
      },
    ),
  ],
);

Widget _host(Widget child) => MaterialApp(
  home: Scaffold(body: SingleChildScrollView(child: child)),
);

void main() {
  testWidgets('失败回执：红卡 + 原因原文 + 错误码，一眼可见', (tester) async {
    final msg = _reply(
      ok: false,
      text: '提交失败：HTTP 400 prompt_outputs_failed_validation（node 107 KeyError）',
      code: 'COMFY_SUBMIT_FAILED',
    );

    await tester.pumpWidget(
      _host(
        SubmitResultCard(
          message: msg,
          mediaThumbUrlOf: (id) => 'http://127.0.0.1:1/thumb/$id',
        ),
      ),
    );

    expect(find.text('提交失败'), findsOneWidget);
    expect(find.byIcon(Icons.error_outline), findsOneWidget);
    // 原因原文必须露出来（不许吞成一句"失败"）
    expect(
      find.textContaining('prompt_outputs_failed_validation'),
      findsOneWidget,
    );
    expect(find.text('COMFY_SUBMIT_FAILED'), findsOneWidget);
  });

  testWidgets('成功回执：绿卡 + 产物入口，点开拿到 mediaId', (tester) async {
    final msg = _reply(
      ok: true,
      text: '手机上排队的那次提交已经跑完了，产物已入库（1 个）',
      mediaIds: const [7],
    );
    var opened = -1;

    await tester.pumpWidget(
      _host(
        SubmitResultCard(
          message: msg,
          mediaThumbUrlOf: (id) => 'http://127.0.0.1:1/thumb/$id',
          onOpenMedia: (id) => opened = id,
        ),
      ),
    );

    expect(find.text('提交完成'), findsOneWidget);
    expect(find.byIcon(Icons.check_circle_outline), findsOneWidget);
    expect(find.text('查看产物'), findsOneWidget);

    await tester.tap(find.text('查看产物'));
    expect(opened, 7);
  });

  testWidgets('没有结果标记的旧回执：不猜成功还是失败', (tester) async {
    final msg = AiMessage(
      id: 'submit-reply-old',
      conversationId: 'c1',
      seq: 1,
      role: 'assistant',
      status: 'complete',
      text: '旧回执',
    );

    await tester.pumpWidget(
      _host(
        SubmitResultCard(
          message: msg,
          mediaThumbUrlOf: (id) => 'http://127.0.0.1:1/thumb/$id',
        ),
      ),
    );

    expect(find.text('提交结果'), findsOneWidget);
    expect(find.textContaining('没有结果标记'), findsOneWidget);
    // 不许把未知当成成功或失败
    expect(find.text('提交失败'), findsNothing);
    expect(find.text('提交完成'), findsNothing);
  });
}
