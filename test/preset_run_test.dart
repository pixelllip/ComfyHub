//! 预设「▶️运行」交给工作台的那段话（2026-10-11）。
//!
//! 这里钉四件事，每一条都是用户明确要求的：
//!  1. **提示词正文逐字带上**（不改写、不截断）；
//!  2. **不带【已存参数】** —— 底模 / 采样器 / 步数 / 尺寸属于"当初那份工作流"，
//!     塞给模型等于替它做决定；正确行为是让它**主动问用户**；
//!  3. `positive_only` 预设**不带负面提示词**，且要明说"别补负面"；
//!  4. **要用户确认后才发**（这一条在弹窗那一侧，见 `showPresetRunDialog`）。

import 'package:flutter_test/flutter_test.dart';
import 'package:viewer/models/models.dart';
import 'package:viewer/pages/preset_run.dart';

Prompt _prompt({
  String title = '海边回眸的黑裙女孩',
  String positive = '(1girl:1.2), (black dress:1.2), ocean',
  String? negative = 'lowres, bad hands',
  String negativeMode = 'keep',
  PromptKind kind = PromptKind.image,
  String? checkpoint = 'krea2Turbo.safetensors',
  String? sampler = 'euler',
  int? steps = 28,
  double? cfg = 4.5,
  int? width = 1024,
  int? height = 1024,
}) => Prompt(
  id: 1,
  title: title,
  kind: kind,
  positivePrompt: positive,
  negativePrompt: negative,
  negativeMode: negativeMode,
  checkpoint: checkpoint,
  sampler: sampler,
  steps: steps,
  cfgScale: cfg,
  width: width,
  height: height,
);

void main() {
  test('提示词正文逐字带上（不改写、不截断）', () {
    final p = _prompt();
    final text = buildPresetRunText(p, PromptKind.image);
    expect(text, contains(p.positivePrompt), reason: '用户存的什么就跑什么');
    expect(text, contains(p.negativePrompt!));
  });

  test('★ 不带【已存参数】—— 让 agent 主动问用户用哪个模型', () {
    final text = buildPresetRunText(_prompt(), PromptKind.image);
    // 底模 / 采样器 / 步数 / CFG / 尺寸，一个都不许出现
    expect(text, isNot(contains('krea2Turbo.safetensors')), reason: '不许替模型挑底模');
    expect(text, isNot(contains('euler')));
    expect(text, isNot(contains('28')));
    expect(text, isNot(contains('4.5')));
    expect(text, isNot(contains('1024')));
    expect(text, isNot(contains('已存参数')), reason: '这一段整个不该存在');
    // 而且必须**明确要求它主动问**
    expect(text, contains('主动问用户'));
    expect(text, contains('没有指定用哪个生图模型'));
  });

  test('positive_only：不带负面，且明说别补负面', () {
    final p = _prompt(negativeMode: 'positive_only');
    final text = buildPresetRunText(p, PromptKind.image);
    expect(text, isNot(contains('lowres')), reason: '只发正面的预设不许把负面带上');
    expect(text, isNot(contains('负面提示词】')));
    expect(text, contains('只发正面提示词'), reason: '要明说原因，否则模型会自己补一段');
  });

  test('keep：负面照原样带上', () {
    final text = buildPresetRunText(_prompt(), PromptKind.image);
    expect(text, contains('负面提示词】'));
    expect(text, contains('lowres, bad hands'));
  });

  test('要出什么写清楚了（生图 / 生视频 / 生音频）', () {
    expect(buildPresetRunText(_prompt(), PromptKind.image), contains('去生图'));
    expect(buildPresetRunText(_prompt(), PromptKind.video), contains('去生视频'));
    expect(buildPresetRunText(_prompt(), PromptKind.audio), contains('去生音频'));
  });

  test('要求先找能跑的工作流，别自己从零搭', () {
    final text = buildPresetRunText(_prompt(), PromptKind.image);
    expect(text, contains('comfy_find_workflow'));
    expect(text, contains('别自己从零搭'));
    expect(text, contains('如实说缺什么'), reason: '缺模型不许假装跑过了');
  });
}
