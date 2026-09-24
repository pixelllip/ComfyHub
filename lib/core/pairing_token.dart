/// **进程内共享的配对令牌**（`docs/pairing-design.md` §11）。
///
/// # 为什么要有这个文件
///
/// 令牌的真源是手机上的 Android Keystore（[PairingTokenStore]），但"谁需要它"分布在整个
/// App 里：AI 工作台要它取 run-kit、上行同步、取密钥；画廊 / 提示词 / 标签 / NSFW 开关要它
/// 才能读桌面；「AI 模型与凭据」页要它列 provider 与模型。**闸门一开，不带令牌的请求一律 403** ——
/// 而漏一个调用点就是一条"这个页面用不了"，且两端都不报错（界面上只显示 403 的那句人话）。
///
/// 2026-09-24 真机验收就是这么栽的：`ai_provider_settings_page.dart` 自己造了个
/// `AiApiClient(base)`，打开「AI 模型与凭据」直接一条红横幅 `需要配对令牌（X-ComfyHub-Token）`，
/// 而**配对明明是成功的**（同一个 App 里的 AI 工作台那条路带着令牌跑得好好的）。
///
/// 所以令牌放在这里、由两个 HTTP 客户端**默认读取**（见 `ApiClient` / `AiApiClient` 的构造函数）：
/// 新增一个页面/一个客户端时**什么都不用记得**，它天然就带令牌。
///
/// # 三条纪律
///
/// 1. **只在内存**：绝不落 `SharedPreferences`（它和 API Key 同级，落明文偏好文件等于白存密钥库）；
/// 2. **桌面侧恒为 `null`**：桌面上没有任何东西会写它，于是"带上这个头"这件事在桌面
///    一个字节都不会发生（三条既有 e2e 与桌面 App 全靠这条）；
/// 3. 只有 [PairingTokenStore] 能写它 —— 别在别处 `= token`（那会绕过密钥库）。
library;

/// 配对令牌的进程内缓存（真源是 Android Keystore）。
class PairingTokenCache {
  PairingTokenCache._();

  static String? _value;

  /// 当前令牌；没配对 / 已忘记就是 `null`。
  static String? get value => _value;

  /// 给 HTTP 客户端用的取值回调（**每次请求现取**：配对成功那一刻不必重建客户端）。
  static String? read() => _value;

  /// 只有 `PairingTokenStore` 调这个。
  static void set(String? token) {
    _value = (token == null || token.isEmpty) ? null : token;
  }
}
