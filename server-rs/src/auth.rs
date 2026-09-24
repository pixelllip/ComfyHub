//! **配对闸门（inbound auth）+ 密钥下行** —— 🔒 `docs/pairing-design.md` §11 的实现。
//!
//! # 这个模块解决两件事
//!
//! 1. **后端认令牌**：在此之前 `X-ComfyHub-Token` 在整个 Rust 侧**一次都没被检查过**
//!    （`/api/sync/*`、`/api/ai/*` 谁都能调）—— 只要 `--host 0.0.0.0` 起在局域网上，
//!    同 Wi-Fi 下任何设备都能读媒体原件、翻会话、取 run-kit。
//! 2. **人怎么把令牌交到手机手里**：手机先举手（`pair/request`）→ 桌面屏幕上出现 6 位码
//!    → 人在手机上敲进去（`pair/claim`）→ 换到**这台设备专属**的长期令牌。
//!    扫码通道本轮**没做**（§11.1 写了取舍），形状给它留着。
//!
//! # 四条纪律（每条都对应一个"错了也不报错"的失败模式）
//!
//! ① **默认关着**（`enabled=false`）：三条既有 e2e 与桌面 App 都不带任何头，
//!    闸门一开就得全绿 —— 这是"不许把默认行为改掉"的机器判据（§7 反向回归）。
//! ② **回环永远放行**：桌面 App、`comfyhub.ps1`、三条 e2e 全走 127.0.0.1；
//!    给它们加一道令牌，等于把"本机自用"这条路堵死。
//! ③ **`requestId` 与 ticket 相互独立**：手机发起通道里若把 ticket 当成 requestId 回给手机，
//!    手机自己就能算出 6 位码 —— "必须看到桌面屏幕"这件事**当场失效**，而流程照样跑得通。
//! ④ **凭据端点永不对匿名开放**（与 `enabled` 无关）：那里是明文 API Key，
//!    判据只能是"回环 或 已配对且被显式授权的那台设备"。
//!
//! # 落在哪
//!
//! 状态在 `app_settings.k = 'auth.pairing'`（**不同步**，手机拿不到它 —— §4.6）；
//! ticket / 滚动码 / 待配对请求**只在内存**（进程重启 = 正在进行的那次配对作废，用户再点一次即可）。

use axum::{
    extract::{ConnectInfo, FromRequestParts, Path as AxumPath, Query, Request, State},
    http::{request::Parts, HeaderMap},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::routes::{ApiError, AppState};

// ---------------------------------------------------------------------------
//  常量（形状见 §11.2 / §11.3，改动要同时改两端）
// ---------------------------------------------------------------------------

/// `app_settings` 里的键（`docs/pairing-design.md` §4.6）。
pub const SETTINGS_KEY: &str = "auth.pairing";

/// 状态形状版本。旧的 `/1` 没有 `devices` —— **读到时按空数组**，不是坏 JSON。
pub const SCHEMA: &str = "comfyhub-inbound-auth/2";

/// 配对会话 TTL（§4.1：**120 秒**，不是 inbound-auth-design 里那个 5 分钟）。
const SESSION_TTL_SECS: u64 = 120;

/// 滚动码的窗口（§4.1）。接受**当前与前一窗口**（时钟边界）。
const CODE_WINDOW_SECS: u64 = 60;

/// 码错几次就把会话作废（§4.1：6 位码只有 10^6 种，不限次等于没有）。
const MAX_ATTEMPTS: u32 = 5;

/// 同时最多留几个待配对会话（再点一次 = 挤掉最老的那个）。
const MAX_PENDING: usize = 8;

/// 未配对设备每分钟能举几次手（按来源地址）。
const RATE_MAX_PER_MIN: usize = 12;

/// 一次能下行的 ref 数上限（挡住"把整库 ref 一次要走"）。
const MAX_CREDENTIAL_REFS: usize = 32;

const PAIR_REQUEST_PATH: &str = "/api/auth/pair/request";
const PAIR_CLAIM_PATH: &str = "/api/auth/pair/claim";
const CREDENTIALS_PATH: &str = "/api/ai/credentials";

const HEADER_TOKEN: &str = "x-comfyhub-token";

// ---------------------------------------------------------------------------
//  持久状态（`app_settings` 里那一行 JSON）
// ---------------------------------------------------------------------------

/// 一台已配对的设备（§4.6 / §11.3）。**令牌只在这里与 claim 的响应里出现**。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeviceRecord {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub token: String,
    #[serde(default, rename = "pairedAt")]
    pub paired_at: Option<String>,
    #[serde(default, rename = "lastSeenAt")]
    pub last_seen_at: Option<String>,
    #[serde(default, rename = "lastIp")]
    pub last_ip: Option<String>,
    /// **密钥下行的唯一开关**，默认 `false`：配对成功 ≠ 能拿 API Key（§11.3）。
    #[serde(default, rename = "allowCredentials")]
    pub allow_credentials: bool,
}

fn default_schema() -> String {
    SCHEMA.to_string()
}

/// `auth.pairing` 那一行。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthFile {
    #[serde(default = "default_schema")]
    pub schema: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default, rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(default, rename = "rotatedAt")]
    pub rotated_at: Option<String>,
    #[serde(default)]
    pub devices: Vec<DeviceRecord>,
}

impl Default for AuthFile {
    fn default() -> Self {
        Self {
            schema: default_schema(),
            enabled: false,
            token: None,
            created_at: None,
            rotated_at: None,
            devices: Vec::new(),
        }
    }
}

/// 读那一行。**读不到 / 读坏了一律按默认值**（= 闸门关着、没有设备）——
/// 那不是"库坏了"，是"还没配过"。
pub fn load(conn: &Connection) -> AuthFile {
    let raw: Option<String> = conn
        .query_row("SELECT v FROM app_settings WHERE k = ?1", params![SETTINGS_KEY], |row| {
            row.get::<_, Option<String>>(0)
        })
        .ok()
        .flatten();
    raw.and_then(|text| serde_json::from_str::<AuthFile>(&text).ok())
        .unwrap_or_default()
}

/// 写那一行（`ON CONFLICT DO UPDATE`：红线 R3 —— `app_settings` 现在虽不在同步范围内，
/// 但写法统一，将来谁把它加进去也不会点亮假的 delete）。
pub fn save(conn: &Connection, file: &AuthFile) -> rusqlite::Result<()> {
    let raw = serde_json::to_string(file).unwrap_or_else(|_| "{}".to_string());
    conn.execute(
        "INSERT INTO app_settings (k, v) VALUES (?1, ?2)
         ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        params![SETTINGS_KEY, raw],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
//  纯函数层：令牌比对 + 判定（**闸门的全部逻辑都在这里**，端点只做 I/O）
// ---------------------------------------------------------------------------

/// 一次请求在闸门眼里的归属。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Grant {
    /// 本机回环（桌面 App / 脚本 / e2e）。
    Loopback,
    /// 全局令牌（`rotate` 作用在它身上）。
    Global,
    /// 某台已配对设备（带上它是否被允许下行密钥）。
    Device { id: String, allow_credentials: bool },
}

/// 闸门的判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow(Grant),
    /// `403`：**没带**令牌（或这台设备没被授权做这件事）。
    Forbidden(String),
    /// `401`：带了令牌但**不对 / 已被吊销**。
    Unauthorized(String),
}

/// 这个 IP 算不算"本机"。
///
/// `Ipv6Addr::is_loopback` 只认 `::1`，而双栈监听时 `127.0.0.1` 会以
/// `::ffff:127.0.0.1` 的形状出现 —— 少认这一种，桌面 App 就会被自己的闸门关在门外。
pub fn is_loopback(remote: Option<IpAddr>) -> bool {
    match remote {
        Some(ip) => match ip {
            IpAddr::V4(v4) => v4.is_loopback(),
            IpAddr::V6(v6) => {
                v6.is_loopback() || v6.to_ipv4_mapped().map(|v4| v4.is_loopback()).unwrap_or(false)
            }
        },
        // **拿不到来源地址 = 不当成本机**（保守）。三条既有 e2e 走的是 `enabled=false`
        // 那条放行分支，所以它们不受影响；而"忘了装 ConnectInfo"必须是安全的那一侧。
        None => false,
    }
}

/// 从请求头取令牌（空串 = 没带）。
pub fn header_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(HEADER_TOKEN)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// 常数时间比较（D9）。长度不同直接 false —— 长度本身不是秘密（令牌恒 43 字符）。
pub fn tokens_equal(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// 令牌 → 归属（`None` = 不认识）。
pub fn grant_of(file: &AuthFile, token: &str) -> Option<Grant> {
    if let Some(global) = file.token.as_deref() {
        if !global.is_empty() && tokens_equal(global, token) {
            return Some(Grant::Global);
        }
    }
    file.devices
        .iter()
        .find(|d| !d.token.is_empty() && tokens_equal(&d.token, token))
        .map(|d| Grant::Device { id: d.id.clone(), allow_credentials: d.allow_credentials })
}

/// **闸门的判定**（§11.4 的判定顺序，一行一条，顺序不能换）。
///
/// * `path`：请求路径
/// * `remote`：来源地址（`None` = 拿不到，按非回环处理）
/// * `token`：请求头里的令牌
pub fn decide(path: &str, remote: Option<IpAddr>, token: Option<&str>, file: &AuthFile) -> Verdict {
    // ① 两条匿名端点：它们本来就是给未配对设备用的（内部限速 + 5 次作废）
    if path == PAIR_REQUEST_PATH || path == PAIR_CLAIM_PATH {
        return Verdict::Allow(Grant::Loopback);
    }
    // ② 回环放行
    if is_loopback(remote) {
        return Verdict::Allow(Grant::Loopback);
    }
    // ③ 凭据端点：**只认"已配对 + 已授权"的设备**，与 enabled 无关（纪律 ④）
    if path == CREDENTIALS_PATH {
        return match token.and_then(|t| grant_of(file, t)) {
            Some(Grant::Device { id, allow_credentials: true }) => {
                Verdict::Allow(Grant::Device { id, allow_credentials: true })
            }
            Some(_) => Verdict::Forbidden("这台设备没有被允许读取密钥".into()),
            None => match token {
                Some(_) => Verdict::Unauthorized("令牌无效或已被吊销".into()),
                None => Verdict::Forbidden("需要配对令牌（X-ComfyHub-Token）".into()),
            },
        };
    }
    // ④ 闸门关着 = 今天的行为（默认值，三条 e2e 靠它）
    if !file.enabled {
        return Verdict::Allow(Grant::Loopback);
    }
    // ⑤ 开着：认令牌
    match token {
        Some(t) => match grant_of(file, t) {
            Some(grant) => Verdict::Allow(grant),
            None => Verdict::Unauthorized("令牌无效或已被吊销".into()),
        },
        None => Verdict::Forbidden("需要配对令牌（X-ComfyHub-Token）".into()),
    }
}

// ---------------------------------------------------------------------------
//  内存态：待配对会话 + 限速 + 最近活跃
// ---------------------------------------------------------------------------

/// 一次正在进行的配对。`secret` 就是 ticket：**它永远不回给发起方**（手机发起通道）。
struct Session {
    id: String,
    secret: String,
    device_id: String,
    device_name: String,
    ip: Option<String>,
    expires: u64,
    attempts: u32,
    used: bool,
}

#[derive(Default)]
struct Sessions {
    list: Vec<Session>,
    /// 来源地址 → 最近一分钟内的举手时刻（限速用）。
    hits: HashMap<String, Vec<u64>>,
    /// deviceId → (lastSeenAt, lastIp)：**攒在内存里**，由 `GET /api/auth` 顺手落库
    /// （D3 的纪律：别每个请求写一次库）。
    seen: HashMap<String, (String, String)>,
    last_flush: u64,
}

fn sessions() -> &'static Mutex<Sessions> {
    static STATE: OnceLock<Mutex<Sessions>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(Sessions::default()))
}

/// 清掉**已过期**的会话。
///
/// ⚠ 刻意**不按 `used` 清**：用过的 / 被码错作废的会话要留到 TTL 到期，
/// 这样"同一个 ticket 再兑一次"永远得到 `410`（"一次性"这个词的含义）。
/// 一用过就删掉的话，重放会掉进"找不到会话"那条分支，回的是 `400 配对失败` ——
/// 手机上那句话会变成"码错了，再试试"，而正确处置是"回电脑上重新发起"。
fn prune(list: &mut Vec<Session>, unix: u64) {
    list.retain(|s| s.expires > unix);
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn iso_now() -> String {
    crate::iso_time::from_system_time(SystemTime::now())
}

fn iso_at(unix: u64) -> String {
    crate::iso_time::from_system_time(UNIX_EPOCH + std::time::Duration::from_secs(unix))
}

// ---------------------------------------------------------------------------
//  随机 + 编码 + HMAC（不引新依赖：发布包与离线构建都不该被这条链拖住）
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn os_random(buf: &mut [u8]) -> Result<(), String> {
    #[link(name = "bcrypt")]
    extern "system" {
        fn BCryptGenRandom(
            alg: *mut core::ffi::c_void,
            buffer: *mut u8,
            len: u32,
            flags: u32,
        ) -> i32;
    }
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(format!("BCryptGenRandom 返回 {status}"))
    }
}

#[cfg(not(windows))]
fn os_random(buf: &mut [u8]) -> Result<(), String> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(buf))
        .map_err(|e| format!("/dev/urandom 读取失败：{e}"))
}

/// 系统 CSPRNG 取随机字节。
///
/// ⚠ 取不到**就报错**，不退化成一个"时间戳 + pid"拼出来的假随机数：
/// 那会让令牌看起来正常、实际可预测，而且没有任何测试会红。
fn random_bytes(len: usize) -> Result<Vec<u8>, ApiError> {
    let mut buf = vec![0u8; len];
    os_random(&mut buf).map_err(|e| {
        ApiError::internal(format!("系统的随机数发生器不可用（{e}）：拒绝生成可预测的令牌"))
    })?;
    Ok(buf)
}

/// 32 字节 → base64url（43 字符，无填充）—— 长期令牌与 ticket 都是这个形状（D11）。
pub fn random_token() -> Result<String, ApiError> {
    Ok(base64url(&random_bytes(32)?))
}

/// 16 字节 → 32 位十六进制（配对会话 id；**它不是 ticket**，纪律 ③）。
fn random_hex_id() -> Result<String, ApiError> {
    Ok(random_bytes(16)?.iter().map(|b| format!("{b:02x}")).collect())
}

const B64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const B64STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_with(alphabet: &[u8; 64], bytes: &[u8], pad: bool) -> String {
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(alphabet[(n >> 18) as usize & 63] as char);
        out.push(alphabet[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(alphabet[(n >> 6) as usize & 63] as char);
        } else if pad {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(alphabet[n as usize & 63] as char);
        } else if pad {
            out.push('=');
        }
    }
    out
}

/// base64url（无填充）：令牌、ticket 的编码形状。
pub fn base64url(bytes: &[u8]) -> String {
    base64_with(B64URL, bytes, false)
}

/// base64 **解码**（标准与 url-safe 都认，容忍填充）—— 读 Kotlin 写的 DPAPI 密文用它。
pub fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for ch in text.chars() {
        if ch == '=' || ch == '\n' || ch == '\r' || ch == ' ' {
            continue;
        }
        let value = B64STD
            .iter()
            .position(|c| *c as char == ch)
            .or_else(|| B64URL.iter().position(|c| *c as char == ch))
            .ok_or_else(|| format!("不是合法的 base64 字符：{ch:?}"))?;
        acc = (acc << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

/// HMAC-SHA256（RFC 2104）。用它而不是引 `hmac` crate：这条链只有一处用途，
/// 而多一个依赖就多一次"离线构建拿不到包"的风险。
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    const BLOCK: usize = 64;
    let mut key_block = [0u8; BLOCK];
    if key.len() > BLOCK {
        key_block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(msg);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner);
    outer.finalize().into()
}

/// 滚动 6 位码（§4.1）：
/// `code = HMAC-SHA256(key = ticket, msg = str(floor(unix/60))) mod 10^6`。
///
/// **以 ticket 为密钥**（§9 第 2 条判据）：不另存 secret —— 于是"作废 ticket"自动作废码。
pub fn code_for(secret: &str, unix: u64) -> String {
    let window = unix / CODE_WINDOW_SECS;
    let mac = hmac_sha256(secret.as_bytes(), window.to_string().as_bytes());
    let n = u32::from_be_bytes([mac[0], mac[1], mac[2], mac[3]]) % 1_000_000;
    format!("{n:06}")
}

/// 当前窗口的结束时刻（界面据此滚动重画）。
fn code_expires_at(unix: u64) -> u64 {
    (unix / CODE_WINDOW_SECS + 1) * CODE_WINDOW_SECS
}

// ---------------------------------------------------------------------------
//  DPAPI 解密（**只 Windows**）：把托管凭据文件里的密文还原成明文
// ---------------------------------------------------------------------------

/// 托管凭据文件里的值由 Kotlin 用 `ProtectedData.Protect(bytes, null, CurrentUser)` 加密
/// （`CredentialService.DpapiSecretBox`，**熵为 null**）—— 对应 `CryptUnprotectData` 的空熵。
#[cfg(windows)]
mod dpapi {
    use std::ffi::c_void;

    #[repr(C)]
    struct DataBlob {
        cb_data: u32,
        pb_data: *mut u8,
    }

    #[link(name = "crypt32")]
    extern "system" {
        fn CryptUnprotectData(
            p_data_in: *const DataBlob,
            ppsz_data_descr: *mut *mut u16,
            p_optional_entropy: *const DataBlob,
            pv_reserved: *mut c_void,
            p_prompt_struct: *mut c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;
        fn CryptProtectData(
            p_data_in: *const DataBlob,
            sz_data_descr: *const u16,
            p_optional_entropy: *const DataBlob,
            pv_reserved: *mut c_void,
            p_prompt_struct: *mut c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;
        fn LocalFree(h_mem: *mut c_void) -> *mut c_void;
    }

    /// `CRYPTPROTECT_UI_FORBIDDEN` —— 与 .NET 的 `ProtectedData` 同一档
    /// （服务/无人值守进程里弹不了 UI，必须禁掉）。
    const UI_FORBIDDEN: u32 = 0x1;

    pub fn unprotect(sealed: &[u8]) -> Result<Vec<u8>, String> {
        let mut buf = sealed.to_vec();
        let input = DataBlob { cb_data: buf.len() as u32, pb_data: buf.as_mut_ptr() };
        let mut out = DataBlob { cb_data: 0, pb_data: std::ptr::null_mut() };
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Err(
                "DPAPI 解密失败（换过 Windows 用户 / 凭据文件被改写 / 后端与写入时不是同一个用户）"
                    .to_string(),
            );
        }
        let plain = unsafe { std::slice::from_raw_parts(out.pb_data, out.cb_data as usize).to_vec() };
        unsafe { LocalFree(out.pb_data as *mut c_void) };
        Ok(plain)
    }

    /// 只给用例用：**从 Rust 加密一次**，证明我们的 `unprotect` 与 Kotlin 那份是同一套
    /// （自己写的 FFI 若把熵/scope 弄错，只有"能解开 Kotlin 写的东西"才算证明）。
    #[allow(dead_code)]
    pub fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
        let mut buf = plain.to_vec();
        let input = DataBlob { cb_data: buf.len() as u32, pb_data: buf.as_mut_ptr() };
        let mut out = DataBlob { cb_data: 0, pb_data: std::ptr::null_mut() };
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Err("DPAPI 加密失败".to_string());
        }
        let sealed = unsafe { std::slice::from_raw_parts(out.pb_data, out.cb_data as usize).to_vec() };
        unsafe { LocalFree(out.pb_data as *mut c_void) };
        Ok(sealed)
    }
}

#[cfg(not(windows))]
mod dpapi {
    pub fn unprotect(_sealed: &[u8]) -> Result<Vec<u8>, String> {
        Err("DPAPI 只在 Windows 上可用（托管凭据库不解密）".to_string())
    }
}

/// 解析一个 ref 的明文：**环境变量优先**（与 Kotlin `resolve` 同顺序），再托管文件。
///
/// 环境变量认两个名字：`<ref>` 与 `COMFYHUB_CRED_<ref>`（前缀常量在 `ai.rs`，
/// 写错就是"永远读不到环境变量里那把 key"）。
fn resolve_credential(state: &AppState, reference: &str) -> Result<String, ApiError> {
    let from_env = std::env::var(reference)
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var(format!("COMFYHUB_CRED_{reference}")).ok().filter(|v| !v.is_empty()));
    if let Some(value) = from_env {
        return Ok(value);
    }

    let file = crate::ai::credential_store_path(state);
    let text = std::fs::read_to_string(&file)
        .map_err(|_| ApiError::not_found(format!("桌面上没有配置凭据 {reference}")))?;
    let parsed: Value = serde_json::from_str(&text)
        .map_err(|_| ApiError::internal("托管凭据文件不是合法 JSON（没有下行任何值）"))?;
    let sealed = parsed
        .get(reference)
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::not_found(format!("桌面上没有配置凭据 {reference}")))?;

    let blob = base64_decode(sealed).map_err(|e| ApiError::internal(format!("凭据密文不是合法 base64：{e}")))?;
    let plain = dpapi::unprotect(&blob).map_err(|e| {
        ApiError::ai("MISSING_CREDENTIAL", format!("凭据 {reference} {e}"))
    })?;
    String::from_utf8(plain).map_err(|_| ApiError::internal(format!("凭据 {reference} 解密后不是 UTF-8")))
}

// ---------------------------------------------------------------------------
//  请求提取器
// ---------------------------------------------------------------------------

/// 来源地址（拿不到就是 `None`，按非回环处理）。
pub struct RemoteAddr(pub Option<IpAddr>);

impl<S> FromRequestParts<S> for RemoteAddr
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(RemoteAddr(remote_ip(parts)))
    }
}

fn remote_ip(parts: &Parts) -> Option<IpAddr> {
    parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip())
}

/// 仅回环（桌面 UI 那几个端点）。
fn require_loopback(remote: Option<IpAddr>, what: &str) -> Result<(), ApiError> {
    if is_loopback(remote) {
        Ok(())
    } else {
        Err(ApiError::forbidden(format!("{what} 只允许在本机（127.0.0.1）调用")))
    }
}

// ---------------------------------------------------------------------------
//  闸门中间件
// ---------------------------------------------------------------------------

/// 装在整个 router 上的闸门。
///
/// 每一步判定都在 [`decide`]（纯函数、可单测）；这里只负责"取来源 / 取令牌 / 读状态"。
pub async fn gate(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let remote = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|info| info.0.ip());
    let token = header_token(req.headers());

    // 快路径：两条匿名端点与回环请求**根本不读库**（闸门关着时所有请求都走这里）
    let needs_store = path != PAIR_REQUEST_PATH && path != PAIR_CLAIM_PATH && !is_loopback(remote);
    if !needs_store {
        return next.run(req).await;
    }

    let verdict = state
        .with(|conn| Ok(decide(&path, remote, token.as_deref(), &load(conn))))
        // 读状态失败（库锁中毒等）：按**最保守**的一侧处理 —— 401 而不是放行
        .unwrap_or_else(|e| Verdict::Unauthorized(format!("读取配对状态失败：{}", e.human())));
    match verdict {
        Verdict::Allow(grant) => {
            note_seen(&grant, remote);
            next.run(req).await
        }
        Verdict::Forbidden(message) => ApiError::forbidden(message).into_response(),
        Verdict::Unauthorized(message) => ApiError::unauthorized(message).into_response(),
    }
}

/// 记一笔"这台设备刚刚活跃过"（**只在内存**，由 `GET /api/auth` 顺手落库）。
fn note_seen(grant: &Grant, remote: Option<IpAddr>) {
    let Grant::Device { id, .. } = grant else { return };
    let Some(ip) = remote else { return };
    if let Ok(mut state) = sessions().lock() {
        state.seen.insert(id.clone(), (iso_now(), ip.to_string()));
    }
}

// ---------------------------------------------------------------------------
//  路由
// ---------------------------------------------------------------------------

pub fn router() -> axum::Router<AppState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/auth", get(get_auth).put(put_auth))
        .route("/api/auth/rotate", post(rotate))
        .route("/api/auth/devices", get(list_devices))
        .route("/api/auth/devices/{id}/revoke", post(revoke_device))
        .route("/api/auth/devices/{id}/credentials", post(set_device_credentials))
        .route("/api/auth/pair/start", post(pair_start))
        .route("/api/auth/pair/request", post(pair_request))
        .route("/api/auth/pair/pending", get(pair_pending))
        .route("/api/auth/pair/claim", post(pair_claim))
        // 密钥下行（**永不对匿名非回环请求开放**）
        .route(CREDENTIALS_PATH, get(get_credentials))
}

// ---------------------------------------------------------------------------
//  桌面侧端点
// ---------------------------------------------------------------------------

/// 设备 → 响应对象。**没有 `token` 字段**（连前缀都不回，§5.3）。
fn device_json(device: &DeviceRecord) -> Value {
    let mut obj = Map::new();
    obj.insert("id".into(), Value::String(device.id.clone()));
    obj.insert("name".into(), Value::String(device.name.clone()));
    if let Some(v) = device.paired_at.as_ref() {
        obj.insert("pairedAt".into(), Value::String(v.clone()));
    }
    if let Some(v) = device.last_seen_at.as_ref() {
        obj.insert("lastSeenAt".into(), Value::String(v.clone()));
    }
    if let Some(v) = device.last_ip.as_ref() {
        obj.insert("lastIp".into(), Value::String(v.clone()));
    }
    obj.insert("allowCredentials".into(), Value::Bool(device.allow_credentials));
    Value::Object(obj)
}

/// 待配对会话 → 响应对象（**6 位码在这里出现，也只在回环响应里出现**）。
fn pending_json(session: &Session, unix: u64) -> Value {
    let mut obj = Map::new();
    obj.insert("requestId".into(), Value::String(session.id.clone()));
    obj.insert("code".into(), Value::String(code_for(&session.secret, unix)));
    obj.insert(
        "codeExpiresAt".into(),
        Value::String(iso_at(code_expires_at(unix))),
    );
    obj.insert("expiresAt".into(), Value::String(iso_at(session.expires)));
    if let Some(ip) = session.ip.as_ref() {
        obj.insert("ip".into(), Value::String(ip.clone()));
    }
    if !session.device_name.is_empty() {
        obj.insert("deviceName".into(), Value::String(session.device_name.clone()));
    }
    Value::Object(obj)
}

/// 把内存里攒的 `lastSeen` 落库（最多每 30 秒一次；`GET /api/auth` 顺手调）。
fn flush_seen(file: &mut AuthFile) {
    let drained = {
        let Ok(mut state) = sessions().lock() else { return };
        let now = now_unix();
        if now.saturating_sub(state.last_flush) < 30 {
            return;
        }
        state.last_flush = now;
        std::mem::take(&mut state.seen)
    };
    for (id, (at, ip)) in drained {
        if let Some(device) = file.devices.iter_mut().find(|d| d.id == id) {
            device.last_seen_at = Some(at);
            device.last_ip = Some(ip);
        }
    }
}

/// `GET /api/auth`（**仅回环**）。
async fn get_auth(
    State(state): State<AppState>,
    RemoteAddr(remote): RemoteAddr,
) -> Result<Json<Value>, ApiError> {
    require_loopback(remote, "配对管理")?;
    let unix = now_unix();
    let body = state.with(|conn| {
        let mut file = load(conn);
        flush_seen(&mut file);
        save(conn, &file)?;
        Ok(status_json(&file, unix))
    })?;
    Ok(Json(body))
}

/// `PUT /api/auth` `{enabled}`（**仅回环**）。
async fn put_auth(
    State(state): State<AppState>,
    RemoteAddr(remote): RemoteAddr,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    require_loopback(remote, "配对管理")?;
    let enabled = payload
        .get("enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| ApiError::bad_request("enabled 必须是 true / false"))?;

    let body = state.with(|conn| {
        let mut file = load(conn);
        file.enabled = enabled;
        if enabled && file.token.as_deref().unwrap_or("").is_empty() {
            // 打开开关时顺手生成一个长期令牌（D10 的既有语义）
            file.token = Some(random_token().map_err(to_sql_err)?);
            file.created_at = Some(iso_now());
        }
        save(conn, &file)?;
        Ok(status_json(&file, now_unix()))
    })?;
    Ok(Json(body))
}

/// `POST /api/auth/rotate`（**仅回环**）—— 轮换全局令牌。
///
/// **所有设备立刻 401、没有宽限期**（RSK-E）：所以这里把 `devices` 一起清空，
/// 界面上必须写明"手机上要重新配对"。
async fn rotate(
    State(state): State<AppState>,
    RemoteAddr(remote): RemoteAddr,
) -> Result<Json<Value>, ApiError> {
    require_loopback(remote, "轮换令牌")?;
    let body = state.with(|conn| {
        let mut file = load(conn);
        file.token = Some(random_token().map_err(to_sql_err)?);
        file.rotated_at = Some(iso_now());
        if file.created_at.is_none() {
            file.created_at = Some(iso_now());
        }
        let revoked = file.devices.len();
        file.devices.clear();
        save(conn, &file)?;
        Ok((file.token.is_some(), revoked))
    })?;
    if let Ok(mut state) = sessions().lock() {
        state.seen.clear();
    }
    Ok(Json(serde_json::json!({
        "ok": true,
        "hasToken": body.0,
        "revokedDevices": body.1,
    })))
}

/// `GET /api/auth/devices`（回环 或 带令牌）。
async fn list_devices(
    State(state): State<AppState>,
    RemoteAddr(remote): RemoteAddr,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let token = header_token(&headers);
    let file = state.with(|conn| Ok(load(conn)))?;
    // 非回环**必须已带令牌**（这张表本身是"这台机器上谁有钥匙"的情报）
    if !is_loopback(remote) {
        match token.as_deref().and_then(|t| grant_of(&file, t)) {
            Some(_) => {}
            None => return Err(ApiError::forbidden("需要配对令牌（X-ComfyHub-Token）")),
        }
    }
    Ok(Json(serde_json::json!({
        "devices": file.devices.iter().map(device_json).collect::<Vec<_>>(),
    })))
}

/// `POST /api/auth/devices/{id}/revoke`（**仅回环**）—— 立刻生效。
async fn revoke_device(
    State(state): State<AppState>,
    RemoteAddr(remote): RemoteAddr,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Value>, ApiError> {
    require_loopback(remote, "吊销设备")?;
    let removed = state.with(|conn| {
        let mut file = load(conn);
        let before = file.devices.len();
        file.devices.retain(|d| d.id != id);
        let removed = before != file.devices.len();
        if removed {
            save(conn, &file)?;
        }
        Ok(removed)
    })?;
    if !removed {
        return Err(ApiError::not_found(format!("没有这台设备：{id}")));
    }
    Ok(Json(serde_json::json!({ "ok": true, "revoked": id })))
}

/// `POST /api/auth/devices/{id}/credentials` `{allow}`（**仅回环**）。
///
/// 这是"把 API Key 发给手机"的唯一开关（§11.3）。默认关着，用户显式勾选才开。
async fn set_device_credentials(
    State(state): State<AppState>,
    RemoteAddr(remote): RemoteAddr,
    AxumPath(id): AxumPath<String>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    require_loopback(remote, "调整设备授权")?;
    let allow = payload
        .get("allow")
        .and_then(Value::as_bool)
        .ok_or_else(|| ApiError::bad_request("allow 必须是 true / false"))?;
    let device = state.with(|conn| {
        let mut file = load(conn);
        let Some(device) = file.devices.iter_mut().find(|d| d.id == id) else {
            return Ok(None);
        };
        device.allow_credentials = allow;
        let snapshot = device.clone();
        save(conn, &file)?;
        Ok(Some(snapshot))
    })?;
    match device {
        Some(device) => Ok(Json(device_json(&device))),
        None => Err(ApiError::not_found(format!("没有这台设备：{id}"))),
    }
}

/// `POST /api/auth/pair/start` —— **扫码通道，本轮没做**（§11.1）。
///
/// 刻意**显式 501**而不是让它掉进"接口不存在"：这两件事对用户是不同的
/// （一个是"这条路还没修"，一个是"你路径写错了"）。
async fn pair_start() -> Result<Json<Value>, ApiError> {
    Err(ApiError::not_implemented(
        "扫码配对还没做（本轮只做「手机发起 + 桌面显示 6 位码」，见 docs/pairing-design.md §11.1）",
    ))
}

// ---------------------------------------------------------------------------
//  手机侧端点（匿名 + 限速）
// ---------------------------------------------------------------------------

/// 限速：同一个来源地址一分钟内最多举手 [RATE_MAX_PER_MIN] 次。
fn check_rate(ip: Option<IpAddr>, unix: u64) -> Result<(), ApiError> {
    let key = ip.map(|v| v.to_string()).unwrap_or_else(|| "unknown".to_string());
    let Ok(mut state) = sessions().lock() else {
        return Err(ApiError::internal("配对状态锁已中毒"));
    };
    let hits = state.hits.entry(key).or_default();
    hits.retain(|t| unix.saturating_sub(*t) < CODE_WINDOW_SECS);
    if hits.len() >= RATE_MAX_PER_MIN {
        return Err(ApiError::new_rate_limited("配对请求太频繁，请稍后再试"));
    }
    hits.push(unix);
    Ok(())
}

/// `POST /api/auth/pair/request` —— 手机举手。
///
/// 响应里**只有 `requestId`，没有 `ticket`**（纪律 ③）：手机拿不到算码的密钥，
/// 于是"必须看到桌面屏幕"这件事才成立。
async fn pair_request(
    RemoteAddr(remote): RemoteAddr,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let unix = now_unix();
    check_rate(remote, unix)?;

    let device_id = payload.get("deviceId").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if device_id.is_empty() {
        return Err(ApiError::bad_request("deviceId 不能为空"));
    }
    let device_name = payload
        .get("deviceName")
        .and_then(Value::as_str)
        .unwrap_or("Android 设备")
        .trim()
        .to_string();

    let id = random_hex_id()?;
    let secret = random_token()?;
    let expires = unix + SESSION_TTL_SECS;

    {
        let Ok(mut state) = sessions().lock() else {
            return Err(ApiError::internal("配对状态锁已中毒"));
        };
        // 只留还没过期的（再点一次 = 挤掉最老的那个）
        prune(&mut state.list, unix);
        while state.list.len() >= MAX_PENDING {
            state.list.remove(0);
        }
        state.list.push(Session {
            id: id.clone(),
            secret: secret.clone(),
            device_id,
            device_name,
            ip: remote.map(|v| v.to_string()),
            expires,
            attempts: 0,
            used: false,
        });
    }

    let mut obj = Map::new();
    obj.insert("requestId".into(), Value::String(id));
    obj.insert("expiresAt".into(), Value::String(iso_at(expires)));
    obj.insert("codeExpiresAt".into(), Value::String(iso_at(code_expires_at(unix))));
    obj.insert("serverTime".into(), Value::String(iso_now()));
    Ok(Json(Value::Object(obj)))
}

/// `GET /api/auth/pair/pending`（**仅回环**）—— 桌面上那张"待配对"列表。
async fn pair_pending(RemoteAddr(remote): RemoteAddr) -> Result<Json<Value>, ApiError> {
    require_loopback(remote, "配对列表")?;
    let unix = now_unix();
    let Ok(mut state) = sessions().lock() else {
        return Err(ApiError::internal("配对状态锁已中毒"));
    };
    prune(&mut state.list, unix);
    let pending: Vec<Value> = state
        .list
        .iter()
        .filter(|s| !s.used)
        .map(|s| pending_json(s, unix))
        .collect();
    Ok(Json(serde_json::json!({ "pending": pending })))
}

/// `POST /api/auth/pair/claim` —— 交人敲进来的 6 位码，换这台设备专属的长期令牌。
///
/// 失败一律 **`400 {"error":"配对失败"}`**，**不区分**是 ticket 不对还是码不对
/// （区分等于告诉攻击者"ticket 猜对了"，§5.2）；会话过期 / 已用过 / 试错 5 次 → `410`。
async fn pair_claim(
    State(state): State<AppState>,
    RemoteAddr(remote): RemoteAddr,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let unix = now_unix();
    let request_id = payload.get("requestId").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let ticket = payload.get("ticket").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let code = payload.get("code").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if request_id.is_empty() && ticket.is_empty() {
        return Err(ApiError::bad_request("缺少 requestId 或 ticket"));
    }

    // 校验（**在锁内**：会话状态只有这里会改）
    let outcome = {
        let Ok(mut state) = sessions().lock() else {
            return Err(ApiError::internal("配对状态锁已中毒"));
        };
        let idx = if !request_id.is_empty() {
            state.list.iter().position(|s| s.id == request_id)
        } else {
            state.list.iter().position(|s| tokens_equal(&s.secret, &ticket))
        };
        let Some(idx) = idx else {
            return Err(ApiError::bad_request("配对失败"));
        };
        let expires = state.list[idx].expires;
        let used = state.list[idx].used;
        if used || unix > expires {
            return Err(ApiError::gone("配对会话已失效，请在电脑上重新发起"));
        }
        let secret = state.list[idx].secret.clone();
        let ok = tokens_equal(&code, &code_for(&secret, unix))
            || tokens_equal(&code, &code_for(&secret, unix.saturating_sub(CODE_WINDOW_SECS)));
        if ok {
            state.list[idx].used = true;
            let session = &state.list[idx];
            (session.device_id.clone(), session.device_name.clone(), session.ip.clone())
        } else {
            state.list[idx].attempts += 1;
            if state.list[idx].attempts >= MAX_ATTEMPTS {
                // 5 次错 → **当场作废**（不是"锁 5 分钟"：作废更简单、判据更硬）
                state.list[idx].used = true;
                return Err(ApiError::gone("配对码错误次数过多，本次配对已作废"));
            }
            return Err(ApiError::bad_request("配对失败"));
        }
    };
    let (session_device_id, session_device_name, session_ip) = outcome;

    // 设备身份以**请求里的**为准（会话里那份是举手时带的，可能还没填设备名）
    let device_id = payload
        .get("deviceId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(&session_device_id)
        .to_string();
    let device_name = payload
        .get("deviceName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(&session_device_name)
        .to_string();
    // 设备 id 与会话不一致 = 举手的那台与兑码的那台不是同一个 → 拒绝（如实、不静默）
    if !session_device_id.is_empty() && session_device_id != device_id {
        return Err(ApiError::bad_request("配对失败"));
    }

    let token = random_token()?;
    let schema_version = state.with(|conn| Ok(crate::db::schema_version(conn)?))?;
    let ip = session_ip.or_else(|| remote.map(|v| v.to_string()));
    state.with(|conn| {
        let mut file = load(conn);
        let now = iso_now();
        match file.devices.iter_mut().find(|d| d.id == device_id) {
            // 同一台设备再配一次 = **换令牌**（不是新增一行：那是"同一台手机两台设备"）
            Some(existing) => {
                existing.token = token.clone();
                existing.name = device_name.clone();
                existing.paired_at = Some(now.clone());
                existing.last_ip = ip.clone();
                existing.allow_credentials = existing.allow_credentials && true;
            }
            None => file.devices.push(DeviceRecord {
                id: device_id.clone(),
                name: device_name.clone(),
                token: token.clone(),
                paired_at: Some(now),
                last_seen_at: None,
                last_ip: ip,
                allow_credentials: false,
            }),
        }
        if file.created_at.is_none() {
            file.created_at = Some(iso_now());
        }
        save(conn, &file)?;
        Ok(())
    })?;

    let mut obj = Map::new();
    obj.insert("deviceId".into(), Value::String(device_id));
    // ⚠ 令牌**只在这一处**出现（D4/D5）；此后任何接口都不再回它
    obj.insert("token".into(), Value::String(token));
    obj.insert("schemaVersion".into(), Value::from(schema_version));
    obj.insert("serverTime".into(), Value::String(iso_now()));
    Ok(Json(Value::Object(obj)))
}

// ---------------------------------------------------------------------------
//  密钥下行
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CredentialsQuery {
    #[serde(default)]
    pub refs: Option<String>,
}

/// `GET /api/ai/credentials?refs=a,b` —— **把桌面上的 API Key 发给手机**。
///
/// 准入只有两条（与 `enabled` 无关）：本机回环，或**已配对且 `allowCredentials=true`** 的设备。
/// 未授权的 ref **不出现在结果里、也不报错**（调用方据此如实说"哪几把没拿到"）。
async fn get_credentials(
    State(state): State<AppState>,
    Query(query): Query<CredentialsQuery>,
) -> Result<Json<Value>, ApiError> {
    let wanted: Vec<String> = query
        .refs
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty() && v.len() <= 128)
        .take(MAX_CREDENTIAL_REFS)
        .map(str::to_string)
        .collect();

    let mut out = Map::new();
    let mut failures: Vec<String> = Vec::new();
    for reference in &wanted {
        match resolve_credential(&state, reference) {
            Ok(value) => {
                out.insert(reference.clone(), Value::String(value));
            }
            Err(e) => failures.push(format!("{reference}: {}", e.human())),
        }
    }

    let mut body = Map::new();
    body.insert("credentials".into(), Value::Object(out));
    if !failures.is_empty() {
        // 如实说哪几把没拿到（**不含值**）：手机那边要能告诉用户"这一把还得自己填"
        body.insert("missing".into(), Value::Array(failures.into_iter().map(Value::String).collect()));
    }
    Ok(Json(Value::Object(body)))
}

// ---------------------------------------------------------------------------
//  状态响应
// ---------------------------------------------------------------------------

fn status_json(file: &AuthFile, unix: u64) -> Value {
    let pending: Vec<Value> = sessions()
        .lock()
        .map(|state| {
            state
                .list
                .iter()
                .filter(|s| !s.used && s.expires > unix)
                .map(|s| pending_json(s, unix))
                .collect()
        })
        .unwrap_or_default();
    let mut obj = Map::new();
    obj.insert("enabled".into(), Value::Bool(file.enabled));
    obj.insert(
        "hasToken".into(),
        Value::Bool(file.token.as_deref().map(|t| !t.is_empty()).unwrap_or(false)),
    );
    obj.insert("schema".into(), Value::String(file.schema.clone()));
    obj.insert(
        "devices".into(),
        Value::Array(file.devices.iter().map(device_json).collect()),
    );
    obj.insert("pending".into(), Value::Array(pending));
    Value::Object(obj)
}

/// 把 [`ApiError`] 的 `status` 借来当 `rusqlite::Error` 的载体 —— `AppState::with`
/// 的闭包签名要求 `rusqlite::Result`，而随机数失败是"内部的、非 SQL"的错误。
fn to_sql_err(error: ApiError) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(error.human().to_string())))
}

// ---------------------------------------------------------------------------
//  用例（纯函数层）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn file_enabled() -> AuthFile {
        AuthFile {
            enabled: true,
            token: Some("global-token".into()),
            devices: vec![DeviceRecord {
                id: "phone-1".into(),
                name: "OnePlus".into(),
                token: "device-token".into(),
                allow_credentials: true,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn hmac_matches_rfc4231_vector() {
        // RFC 4231 Test Case 2
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            mac.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn base64url_and_decode_round_trip() {
        // 32 字节 → 43 字符、无填充（D11 的令牌形状）
        let token = base64url(&[0u8; 32]);
        assert_eq!(token.len(), 43);
        assert!(!token.contains('='));
        assert_eq!(base64_decode(&token).unwrap(), vec![0u8; 32]);
        // Kotlin 写的是**标准** base64（带 + / 与填充），解码器两种都要认
        assert_eq!(base64_decode("+/8=").unwrap(), vec![0xfb, 0xff]);
    }

    #[test]
    fn rolling_code_is_six_digits_and_window_scoped() {
        let code = code_for("ticket", 1_700_000_000);
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
        // 同一个 60 秒窗口内恒定、跨窗口必变（滚动的唯一含义）。
        // ⚠ 窗口是按 `unix / 60` 切的，不是"从第一次请求起算 60 秒"：
        //    1_700_000_000 落在窗口的第 20 秒，所以 +39 还在同一格、+40 就换格了。
        assert_eq!(code_for("ticket", 1_700_000_000), code_for("ticket", 1_700_000_039));
        assert_ne!(code_for("ticket", 1_700_000_000), code_for("ticket", 1_700_000_040));
        assert_ne!(code_for("ticket", 1_700_000_000), code_for("ticket", 1_700_000_060));
        // 换 ticket 就换码（"作废 ticket 自动作废码"）
        assert_ne!(code_for("ticket", 1_700_000_000), code_for("other", 1_700_000_000));
    }

    #[test]
    fn gate_lets_anonymous_pairing_paths_through() {
        // 闸门开着、没有令牌：这两条路径必须放行（它们就是给未配对设备用的）
        for path in [PAIR_REQUEST_PATH, PAIR_CLAIM_PATH] {
            assert!(matches!(
                decide(path, Some("10.0.0.9".parse().unwrap()), None, &file_enabled()),
                Verdict::Allow(_)
            ));
        }
    }

    #[test]
    fn gate_forbids_and_unauthorizes_differently() {
        let remote = Some("10.0.0.9".parse().unwrap());
        // 没带令牌 → 403（不是 401）
        match decide("/api/prompts", remote, None, &file_enabled()) {
            Verdict::Forbidden(_) => {}
            other => panic!("期望 403，得到 {other:?}"),
        }
        // 带了错的 → 401
        match decide("/api/prompts", remote, Some("nope"), &file_enabled()) {
            Verdict::Unauthorized(_) => {}
            other => panic!("期望 401，得到 {other:?}"),
        }
        // 对的 → 放行
        assert_eq!(
            decide("/api/prompts", remote, Some("device-token"), &file_enabled()),
            Verdict::Allow(Grant::Device { id: "phone-1".into(), allow_credentials: true })
        );
        // 全局令牌也算
        assert_eq!(
            decide("/api/prompts", remote, Some("global-token"), &file_enabled()),
            Verdict::Allow(Grant::Global)
        );
    }

    #[test]
    fn gate_off_keeps_todays_behaviour() {
        // **不许把默认行为改掉**：关着的时候，非回环不带令牌也照样通
        let file = AuthFile::default();
        assert!(!file.enabled);
        assert!(matches!(
            decide("/api/prompts", Some("10.0.0.9".parse().unwrap()), None, &file),
            Verdict::Allow(_)
        ));
        // 三条既有 e2e / 桌面 App 走的是回环那条
        assert!(matches!(
            decide("/api/prompts", Some("127.0.0.1".parse().unwrap()), None, &file),
            Verdict::Allow(Grant::Loopback)
        ));
    }

    #[test]
    fn loopback_is_recognised_including_ipv4_mapped() {
        assert!(is_loopback(Some("127.0.0.1".parse().unwrap())));
        assert!(is_loopback(Some("::1".parse().unwrap())));
        assert!(is_loopback(Some("::ffff:127.0.0.1".parse().unwrap())));
        assert!(!is_loopback(Some("10.44.33.71".parse().unwrap())));
        // ⚠ 拿不到来源地址 = **不是**本机（保守的那一侧）
        assert!(!is_loopback(None));
    }

    #[test]
    fn credentials_endpoint_is_never_anonymous() {
        // 纪律 ④：与 enabled 无关 —— 关着闸门也照样要"已配对 + 已授权"
        let off = AuthFile {
            enabled: false,
            token: Some("global-token".into()),
            devices: vec![
                DeviceRecord {
                    id: "phone-1".into(),
                    token: "granted".into(),
                    allow_credentials: true,
                    ..Default::default()
                },
                DeviceRecord {
                    id: "phone-2".into(),
                    token: "not-granted".into(),
                    allow_credentials: false,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let remote = Some("10.0.0.9".parse().unwrap());
        // 匿名 → 403（**不是**放行）
        match decide(CREDENTIALS_PATH, remote, None, &off) {
            Verdict::Forbidden(_) => {}
            other => panic!("匿名请求必须被拒，得到 {other:?}"),
        }
        // 全局令牌也不行：那是"配对管理"的钥匙，不是"读密钥"的授权
        match decide(CREDENTIALS_PATH, remote, Some("global-token"), &off) {
            Verdict::Forbidden(_) => {}
            other => panic!("全局令牌不该能读密钥，得到 {other:?}"),
        }
        // 配对了但没勾"允许密钥" → 403
        match decide(CREDENTIALS_PATH, remote, Some("not-granted"), &off) {
            Verdict::Forbidden(_) => {}
            other => panic!("未授权设备不该能读密钥，得到 {other:?}"),
        }
        // 勾了 → 放行
        assert!(matches!(
            decide(CREDENTIALS_PATH, remote, Some("granted"), &off),
            Verdict::Allow(Grant::Device { allow_credentials: true, .. })
        ));
        // 本机回环 → 放行（桌面 App 自己要用）
        assert!(matches!(
            decide(CREDENTIALS_PATH, Some("127.0.0.1".parse().unwrap()), None, &off),
            Verdict::Allow(Grant::Loopback)
        ));
    }

    #[test]
    fn auth_file_tolerates_missing_keys() {
        // 旧的 `/1` 形状（没有 devices）与手工改过的 JSON 都要能读
        let old: AuthFile = serde_json::from_str(r#"{"schema":"comfyhub-inbound-auth/1","enabled":true}"#).unwrap();
        assert!(old.devices.is_empty());
        assert_eq!(old.token, None);
        let partial: AuthFile = serde_json::from_str(r#"{"enabled":true,"devices":[{"id":"p","token":"t"}]}"#).unwrap();
        assert_eq!(partial.devices.len(), 1);
        assert!(!partial.devices[0].allow_credentials);
        assert_eq!(partial.schema, SCHEMA);
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_round_trip_works() {
        // 证明我们这套 FFI（空熵 + CurrentUser）真的能加解密 ——
        // 它是"钥匙下行"能不能用的唯一技术前提
        let plain = b"sk-test-not-a-real-key";
        let sealed = dpapi::protect(plain).expect("DPAPI 加密失败");
        assert_ne!(sealed, plain);
        assert_eq!(dpapi::unprotect(&sealed).expect("DPAPI 解密失败"), plain);
        // 密文按 Kotlin 的形状落盘：base64 文本
        let text = base64_with(B64STD, &sealed, true);
        assert_eq!(dpapi::unprotect(&base64_decode(&text).unwrap()).unwrap(), plain);
    }

    #[cfg(windows)]
    #[test]
    fn resolve_credential_reads_the_managed_store_env_first() {
        // 托管文件那份是 **Kotlin 写的密文**（标准 base64 + DPAPI/CurrentUser/空熵）。
        // 这条用例把"Rust 能不能解开 Kotlin 写的东西"钉死 —— 自己写的 FFI
        // 参数（熵 / scope）错一位，表现就是"下行的密钥全是乱码或直接失败"。
        let tmp = std::env::temp_dir().join(format!("comfyhub-auth-cred-{}", std::process::id()));
        let media = tmp.join("media");
        let ai = tmp.join("ai");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::create_dir_all(&ai).unwrap();
        let plain = "sk-from-managed-store";
        let sealed = dpapi::protect(plain.as_bytes()).unwrap();
        let stored = serde_json::json!({ "fixture.ref": base64_with(B64STD, &sealed, true) });
        std::fs::write(
            ai.join("credentials.dpapi.json"),
            serde_json::to_string_pretty(&stored).unwrap(),
        )
        .unwrap();

        let state = AppState::new(crate::db::open_memory().unwrap(), ":memory:", media.clone());
        assert_eq!(resolve_credential(&state, "fixture.ref").unwrap(), plain);

        // 环境变量优先（与 Kotlin `CredentialService.resolve` 同顺序）
        std::env::set_var("fixture.ref", "from-env");
        assert_eq!(resolve_credential(&state, "fixture.ref").unwrap(), "from-env");
        std::env::remove_var("fixture.ref");

        // 桌面上没有的 ref → 404 形状的人话（不是 500、也不是空串）
        let error = resolve_credential(&state, "nope").unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::NOT_FOUND);
        assert!(error.human().contains("nope"));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
