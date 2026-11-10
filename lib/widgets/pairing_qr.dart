import 'package:flutter/material.dart';
import 'package:qr_flutter/qr_flutter.dart';

import '../core/pairing_payload.dart';

/// 把一次配对画成二维码（`docs/pairing-design.md` §4.5）。
///
/// 单独一个 widget 有两个理由：
/// 1. **二维码要浅底才扫得动** —— 暗色主题下必须铺一层白，不然手机对着一块黑方块扫不出来；
/// 2. 它是**唯一**把 [PairingPayload] 变成像素的地方，用例能直接拿到 `payload` 断言
///    "画出来的确实是那份 ticket + 那些地址"（`QrImageView` 的 data 是私有的，拿不到）。
class PairingQrImage extends StatelessWidget {
  const PairingQrImage({super.key, required this.payload, this.size = 180});

  final PairingPayload payload;
  final double size;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        // 固定白底：扫码靠的是明暗对比，跟着主题走会出现"深色主题下扫不出来"
        color: Colors.white,
        borderRadius: BorderRadius.circular(8),
      ),
      child: QrImageView(
        data: payload.encode(),
        size: size,
        backgroundColor: Colors.white,
        // 语义标签为读屏提供说明（二维码本身无意义）
        semanticsLabel: '配对二维码，手机扫码后仍需输入桌上显示的 6 位码',
      ),
    );
  }
}
