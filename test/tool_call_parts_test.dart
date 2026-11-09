// 工具卡的**块形状**回归（2026-11-09 真机）：
//
// 手机上跑一轮里同时调两个工具时，harness 曾把**全部调用**塞进**同一个**
// `tool_call` 块：`[{id,name}, …]`。共享界面按 `payload['name']` 取名字、
// 按 `toolCallId` 配结果 —— 于是那一轮的卡片全是「未知工具 · 失败」的空卡
// （参数也丢了：数组里根本没写 arguments）。
//
// 这里钉住两件事：
//   ① 旧形状（数组块）仍要能展开成多张卡 —— 已经同步下来的历史行不重跑就得能看；
//   ② 新形状（一次调用一个块，`{name, arguments}`）按 id 与结果配对。
//
// 写端（Kotlin `HarnessRunner`）已改成一次一个块；读端留兼容是为了历史数据。

import 'package:flutter_test/flutter_test.dart';
import 'package:viewer/models/ai_models.dart';

AiMessagePart _call(String id, String jsonPayload) =>
    AiMessagePart.fromJson({
      'type': 'tool_call',
      'toolCallId': id,
      'jsonPayload': jsonPayload,
    });

AiMessagePart _result(String id, {required bool ok, String? name}) =>
    AiMessagePart.fromJson({
      'type': 'tool_result',
      'toolCallId': id,
      'text': ok ? '跑完了' : '失败了',
      'jsonPayload': {
        'ok': ok,
        'name': ?name,
        if (!ok) 'error': '缺少参数 name',
      },
    });

void main() {
  test('旧形状：一个块里装着整轮调用 → 展开成多张卡，结果按 id 配对', () {
    final parts = [
      _call('call_1,call_2', '[{"id":"call_1","name":"load_skill"},'
          '{"id":"call_2","name":"list_skills"}]'),
      _result('call_1', ok: true, name: 'load_skill'),
      _result('call_2', ok: false, name: 'list_skills'),
    ];

    final calls = AiToolCallState.listFromParts(parts);

    expect(calls.length, 2, reason: '一块里的两个调用要各出一张卡');
    expect(calls[0].name, 'load_skill');
    expect(calls[0].status, AiToolCallStatus.ok);
    expect(calls[1].name, 'list_skills');
    expect(calls[1].status, AiToolCallStatus.failed);
    expect(calls[1].error, contains('缺少参数 name'), reason: '失败原因要原样给出来');
  });

  test('新形状：一次调用一个块（name + arguments），结果按 id 配对', () {
    final parts = [
      _call('call_9', '{"name":"comfy_queue_submit","arguments":"{\\"task\\":\\"t2i\\"}"}'),
      _result('call_9', ok: true),
    ];

    final calls = AiToolCallState.listFromParts(parts);

    expect(calls.length, 1);
    expect(calls[0].name, 'comfy_queue_submit');
    expect(calls[0].arguments, contains('t2i'), reason: '参数要留在卡上（旧形状里根本没有）');
    expect(calls[0].status, AiToolCallStatus.ok);
  });

  test('数组块里没有名字的坏元素：跳过，不造空卡', () {
    final parts = [
      _call('x', '[{"id":"call_1","name":"remember"},{"name":"没有 id 的"}]'),
    ];

    final calls = AiToolCallState.listFromParts(parts);

    expect(calls.length, 1);
    expect(calls.single.name, 'remember');
  });

  test('旧数据的结果块没有 ok 键：按失败前缀判，不许一律画成失败', () {
    // 手机 harness 以前只落工具自己的结果 JSON（没有 `ok`）。真机上那会让
    // "明明成功的 load_skill" 显示「失败」—— 判据只有"失败正文以 [工具失败: 开头"。
    final parts = [
      AiMessagePart.fromJson({
        'type': 'tool_call',
        'toolCallId': 'call_ok',
        'jsonPayload': '{"name":"load_skill","arguments":"{}"}',
      }),
      AiMessagePart.fromJson({
        'type': 'tool_result',
        'toolCallId': 'call_ok',
        'text': '技能正文',
        'jsonPayload': '{"name":"load_skill"}',
      }),
      AiMessagePart.fromJson({
        'type': 'tool_call',
        'toolCallId': 'call_bad',
        'jsonPayload': '{"name":"load_skill","arguments":"{}"}',
      }),
      AiMessagePart.fromJson({
        'type': 'tool_result',
        'toolCallId': 'call_bad',
        'text': '[工具失败:NO_SKILL] 没有这个技能',
        'jsonPayload': 'null',
      }),
    ];

    final calls = AiToolCallState.listFromParts(parts);

    expect(calls[0].status, AiToolCallStatus.ok);
    expect(calls[1].status, AiToolCallStatus.failed);
  });
}
