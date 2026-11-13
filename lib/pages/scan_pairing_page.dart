import 'package:material_ui/material_ui.dart';
import 'package:mobile_scanner/mobile_scanner.dart';

import '../core/pairing_payload.dart';

/// 扫桌面上的配对二维码（`docs/pairing-design.md` §3.2 通道 A 的手机那一半）。
///
/// 它只做两件事：**扫**、**解析**。解析出来的东西交给调用方（配对卡）去决定：
/// 地址填哪个、ticket 怎么带着去 claim。这样"格式"只有一份实现
/// （[PairingPayload]），这个页面里没有第二条解析逻辑。
///
/// 三条纪律：
/// 1. **扫到不是 ComfyHub 的码** → 说清是"扫到了别的码"，**继续扫**（不弹走页面）；
/// 2. **相机不可用不算失败**：安卓上常见（没授权 / 被别的 App 占着）——
///    如实说一句 + 给「手输地址」那条退路，**绝不让扫码的可用性决定配对整条的可用性**；
/// 3. 命中一次就 `pop`（防连扫十次把同一件事提交十遍）。
class ScanPairingPage extends StatefulWidget {
  const ScanPairingPage({super.key});

  @override
  State<ScanPairingPage> createState() => _ScanPairingPageState();
}

class _ScanPairingPageState extends State<ScanPairingPage> {
  final _controller = MobileScannerController();

  /// 上一帧的问题（"这不是 ComfyHub 的码"之类）——显示成一条横幅，**不挡住继续扫**。
  String? _problem;
  bool _handled = false;

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  void _onDetect(BarcodeCapture capture) {
    if (_handled) return;
    final raw = capture.barcodes
        .map((b) => b.rawValue)
        .whereType<String>()
        .map((v) => v.trim())
        .firstWhere((v) => v.isNotEmpty, orElse: () => '');
    if (raw.isEmpty) return;

    final parsed = PairingPayload.parse(raw);
    if (!parsed.ok) {
      // 继续扫描：二维码可能未对准，或画面上存在其他码
      if (mounted && parsed.error != _problem) {
        setState(() => _problem = parsed.error);
      }
      return;
    }
    _handled = true;
    Navigator.of(context).pop(parsed.payload);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      appBar: AppBar(
        title: const Text('扫桌面上的二维码'),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('手输地址'),
          ),
        ],
      ),
      body: Column(
        children: [
          Expanded(
            child: Stack(
              fit: StackFit.expand,
              children: [
                MobileScanner(
                  controller: _controller,
                  onDetect: _onDetect,
                  errorBuilder: (context, error) => Center(
                    child: Padding(
                      padding: const EdgeInsets.all(20),
                      child: Column(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          const Icon(Icons.no_photography_outlined, size: 42),
                          const SizedBox(height: 10),
                          Text(
                            '相机用不了，${error.errorCode.name}',
                            textAlign: TextAlign.center,
                          ),
                          const SizedBox(height: 6),
                          const Text(
                            '可以用右上角「手输地址」走那条路（桌面上的 6 位码照样要敲进来）。',
                            textAlign: TextAlign.center,
                            style: TextStyle(fontSize: 12),
                          ),
                        ],
                      ),
                    ),
                  ),
                ),
                // 取景框：提供明确的对准目标
                IgnorePointer(
                  child: Center(
                    child: Container(
                      width: 220,
                      height: 220,
                      decoration: BoxDecoration(
                        border: Border.all(color: Colors.white70, width: 2),
                        borderRadius: BorderRadius.circular(12),
                      ),
                    ),
                  ),
                ),
              ],
            ),
          ),
          Padding(
            padding: const EdgeInsets.all(12),
            child: Text(
              _problem ?? '把桌面上的二维码放进框里。扫到之后，还要把桌面上显示的那 6 位码敲进手机。',
              style: theme.textTheme.bodySmall?.copyWith(
                color: _problem == null
                    ? theme.colorScheme.outline
                    : theme.colorScheme.error,
              ),
            ),
          ),
        ],
      ),
    );
  }
}
