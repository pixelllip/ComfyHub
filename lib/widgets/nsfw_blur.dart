import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../state/nsfw_store.dart';

/// NSFW 预览图打码：开启后 NSFW 内容的预览图模糊显示。
///
/// ## ⚠ **只打码被标记过的那一条**
///
/// 上一版把 [blurKey] 当作"需要打码的内容"，开关开启后**整页**缩略图全部打码，
/// 包括测试图与音频封面。当时库中没有"该条是否为 NSFW"这一事实，
/// 现在由 `media_assets.nsfw` 提供：
///
///  - [nsfw] = false（默认）→ **原样返回 [child]**，一个像素都不动；
///  - [nsfw] = true 且开启「预览图模糊」→ 打码 + 覆盖"点击查看"；
///  - 点开详情页 → **详情页不打码**（`MediaDetailPage` 中解除该条，
///    返回列表后也不再打码）。
///
/// 三件事说清楚：
///
///  1. **开关的真源在后端**（`NsfwStore` 镜像 `app_settings` 的 `ai.nsfw`），
///     这里只读不算 —— 前端自己再推一遍就会出现"设置页说会打码、画廊其实没打"。
///  2. [revealKey] 是"这一条"的身份（`media:<id>`）：看过之后只解开那一条，
///     不是"一开就到哪儿都看得见"。
///  3. 解除记录只存**内存**（`NsfwStore._revealed`）：关闭 App 后重新打码 ——
///     该选项的语义是默认打码，不是永久解锁某几条。
///
/// 没挂 `NsfwStore`（例如某些单测直接 pump 一个 widget）时**不打码**：
/// 宁可显示原图，也不让用例因为缺少一个 provider 而失败。
class NsfwBlur extends StatelessWidget {
  /// 要打码的内容（图片 / 视频封面…）。
  final Widget child;

  /// 这一条**是不是**成人内容。false → 直接返回 [child]。
  final bool nsfw;

  /// 这一条的身份（同一个 key 只解开一次）。为空时视为"永远需要打码"。
  final String? revealKey;

  /// 圆角，跟外层的格子对齐（模糊层自己会被裁一次，不圆角会露出直角）。
  final BorderRadius? borderRadius;

  const NsfwBlur({
    super.key,
    required this.child,
    this.nsfw = false,
    this.revealKey,
    this.borderRadius,
  });

  @override
  Widget build(BuildContext context) {
    if (!nsfw) return child;
    final store = context.watch<NsfwStore?>();
    final blurred = store?.shouldBlur(revealKey) ?? false;
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
        // 覆盖一层说明。**不拦截点击**（`IgnorePointer`）：点击由外层
        // "打开详情"的 InkWell 处理 —— 一次点击即打开详情，
        // 不需要"第一次解锁、第二次打开"。
        Positioned.fill(
          child: IgnorePointer(
            child: ColoredBox(
              color: Colors.black.withValues(alpha: 0.28),
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
                        shadows: const [
                          Shadow(blurRadius: 3, color: Colors.black87),
                        ],
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
