import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../state/nsfw_store.dart';

/// NSFW 预览图打码（2026-09-20 用户要求：「再加一个 NSFW 模糊选项，
/// 开启后 NSFW 内容的预览图模糊显示」）。
///
/// 三件事说清楚：
///
///  1. **开关的真源在后端**（`NsfwStore` 镜像 `app_settings` 的 `ai.nsfw`），
///     这里只读不算 —— 前端自己再推一遍就会出现"设置页说会打码、画廊其实没打"。
///  2. [blurKey] 是"这一张图"的身份（通常用文件 URL）：点开看过之后只解开那一张，
///     不是"一开就到哪儿都看得见"。
///  3. 解开只记在**内存**里（`NsfwStore._revealed`）：关掉 App 再打开重新打码 ——
///     用户开这个选项就是不想一进来被糊一脸，而不是要永久解锁某几张。
///
/// 没挂 `NsfwStore`（例如某些单测直接 pump 一个 widget）时**不打码**：
/// 宁可显示原图，也不要让用例因为少一个 provider 就崩。
class NsfwBlur extends StatelessWidget {
  /// 要打码的内容（图片 / 视频封面…）。
  final Widget child;

  /// 这张图的身份（同一个 key 只解开一次）。为空时视为"永远需要打码"。
  final String? blurKey;

  /// 圆角，跟外层的格子对齐（模糊层自己会被裁一次，不圆角会露出直角）。
  final BorderRadius? borderRadius;

  const NsfwBlur({
    super.key,
    required this.child,
    required this.blurKey,
    this.borderRadius,
  });

  @override
  Widget build(BuildContext context) {
    final store = context.watch<NsfwStore?>();
    final blurred = store?.shouldBlur(blurKey) ?? false;
    if (!blurred) return child;

    return Stack(
      fit: StackFit.expand,
      children: [
        // 模糊层与内容层用同一份 child：`ImageFiltered` 只影响绘制，不影响布局
        ClipRRect(
          borderRadius: borderRadius ?? BorderRadius.zero,
          child: ImageFiltered(
            imageFilter: ui.ImageFilter.blur(sigmaX: 18, sigmaY: 18),
            child: child,
          ),
        ),
        // 点一下解开这一张（只对本次会话有效）。用 Positioned.fill 盖住整块，
        // 这样缩略图上的"点开详情"仍然要点第二下 —— 第一下先解锁，符合预期。
        Positioned.fill(
          child: Material(
            color: Colors.black.withValues(alpha: 0.28),
            child: InkWell(
              onTap: () => store?.reveal(blurKey),
              child: Center(
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(
                      Icons.visibility_off_outlined,
                      size: 26,
                      color: Colors.white.withValues(alpha: 0.92),
                    ),
                    const SizedBox(height: 4),
                    Text(
                      '已模糊 · 点击查看',
                      style: TextStyle(
                        color: Colors.white.withValues(alpha: 0.92),
                        fontSize: 11.5,
                        fontWeight: FontWeight.w600,
                        shadows: const [Shadow(blurRadius: 3, color: Colors.black87)],
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}
