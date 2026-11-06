/// 本地 LoRA 训练引擎（anima-lora-next）的状态。
///
/// 真源只有后端 `GET /api/train/status`：引擎目录、守护进程 / WebUI 的端口与
/// 在线状态、项目数、问题清单。**前端不猜端口、不猜路径** —— WebUI 是
/// `pythonw -m anima_lora_webui.server --port 0` 之外由后端挑的空闲端口，
/// 只有这里拿到的那一个是对的。
class TrainEngineStatus {
  final bool installed;
  final String? engineDir;
  final String? python;
  final String? webuiSource;
  final TrainEndpoint daemon;
  final TrainEndpoint webui;
  final int projects;
  final List<String> problems;

  const TrainEngineStatus({
    required this.installed,
    required this.engineDir,
    required this.python,
    required this.webuiSource,
    required this.daemon,
    required this.webui,
    required this.projects,
    required this.problems,
  });

  factory TrainEngineStatus.fromJson(Map<String, dynamic> json) {
    return TrainEngineStatus(
      installed: json['installed'] == true,
      engineDir: json['engineDir'] as String?,
      python: json['python'] as String?,
      webuiSource: json['webuiSource'] as String?,
      daemon: TrainEndpoint.fromJson(
        (json['daemon'] as Map?)?.cast<String, dynamic>() ?? const {},
      ),
      webui: TrainEndpoint.fromJson(
        (json['webui'] as Map?)?.cast<String, dynamic>() ?? const {},
      ),
      projects: (json['projects'] as num?)?.toInt() ?? 0,
      problems:
          (json['problems'] as List?)
              ?.map((e) => e.toString())
              .toList(growable: false) ??
          const [],
    );
  }

  bool get ready => installed && webui.up && webui.url != null;
}

class TrainEndpoint {
  final bool up;
  final int? port;
  final String? url;

  /// 训练台的可选访问令牌（Rust 起 WebUI 时生成；手跑实例没有）。
  /// 首跳 `/?token=…` 会种 HttpOnly Cookie，之后资源/API/SSE 自动带上。
  final String? token;

  const TrainEndpoint({
    required this.up,
    required this.port,
    required this.url,
    this.token,
  });

  factory TrainEndpoint.fromJson(Map<String, dynamic> json) {
    final rawPort = json['port'];
    return TrainEndpoint(
      up: json['up'] == true,
      // 后端给的是数字；但旧构建 / 手写 JSON 可能给字符串 —— 两种都认，
      // 认不出来就是 null（界面显示“端口未知”，不抛）。
      port: rawPort is num
          ? rawPort.toInt()
          : int.tryParse(rawPort?.toString() ?? ''),
      url: json['url'] as String?,
      token: json['token'] as String?,
    );
  }
}
