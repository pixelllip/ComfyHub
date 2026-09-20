import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../state/nsfw_store.dart';

/// NSFW 预览图打码（2026-09-20 用户要求：「再加一个 NSFW 模糊选项，
/// 开启后 NSFW 内容的预览图模糊显示」）。
///
/// ## ⚠ 2026-09-20 晚重写：**只糊被标记过的那一条**
///
/// 用户的 bug 原话：「NSFW内容模糊——怎么丝毫不区分正常内容全部模糊呢？」。
/// 上一版把 [blurKey] 当"要糊的东西"，于是开关一开**整页**缩略图全糊了 ——
/// 连测试图和音频封面都不放过。原因很简单：库里当时根本没有"这条是不是 NSFW"
/// 这个事实。现在有了（`media_assets.nsfw`）：
///
///  - [nsfw] = false（默认）→ **原样返回 [child]**，一个像素都不动；
///  - [nsfw] = true 且用户开着「预览图模糊」→ 糊 + 盖一层"点击查看"；
///  - 点开详情页 = 用户主动要看 → **详情页不打码**（`MediaDetailPage` 里解开这一条，
///    所以回到列表时这条也不再糊）——用户原话：「打开详情之后就没必要模糊了」。
///
/// 三件事说清楚：
///
///  1. **开关的真源在后端**（`NsfwStore` 镜像 `app_settings` 的 `ai.nsfw`），
///     这里只读不算 —— 前端自己再推一遍就会出现"设置页说会打码、画廊其实没打"。
///  2. [revealKey] 是"这一条"的身份（`media:<id>`）：看过之后只解开那一条，
///     不是"一开就到哪儿都看得见"。
///  3. 解开只记在**内存**里（`NsfwStore._revealed`）：关掉 App 再打开重新打码 ——
///     用户开这个选项就是不想一进来被糊一脸，而不是要永久解锁某几条。
///
/// 没挂 `NsfwStore`（例如某些单测直接 pump 一个 widget）时**不打码**：
/// 宁可显示原图，也不要让用例因为少一个 provider 就崩。
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
        // 盖一层说明。**不吃点击**（`IgnorePointer`）：点下去走的是外层那个
        // "打开详情"的 InkWell —— 用户要的是"点开详情就能看"，而不是
        // "第一下只解锁、第二下才打开"。
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
