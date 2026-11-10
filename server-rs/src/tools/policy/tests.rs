//! `policy` 的单元测试（主体代码在 [`policy`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn policy(json: &str) -> ToolPolicy {
    let config: ToolPolicyConfig = serde_json::from_str(json).unwrap();
    ToolPolicy::new(PathBuf::from(r"D:\proj"), config)
}

#[test]
fn 出厂白名单与_kotlin_一致() {
    let p = policy("{}");
    assert_eq!(p.write_roots, vec![PathBuf::from(r"D:\proj\comfyui")]);
    assert_eq!(
        p.read_roots,
        vec![
            PathBuf::from(r"D:\proj\comfyui"),
            PathBuf::from(r"D:\proj\storage")
        ]
    );
    assert_eq!(p.permission_mode, PERMISSION_ASK);
    assert!(!p.full_permission());
}

#[test]
fn 白名单展开认相对与绝对两种写法() {
    let p = policy(
        r#"{"writeRoots":["comfyui","D:\\other\\out","a/../b"],"readRoots":["  storage  "],"permissionMode":"full"}"#,
    );
    assert_eq!(
        p.write_roots,
        vec![
            PathBuf::from(r"D:\proj\comfyui"),
            PathBuf::from(r"D:\other\out"),
            PathBuf::from(r"D:\proj\b"),
        ]
    );
    assert_eq!(p.read_roots, vec![PathBuf::from(r"D:\proj\storage")]);
    assert!(p.full_permission());
}

#[test]
fn 权限档认不出来就回落询问() {
    // 配置写坏（比如拼成 "FULL "、或者老库里塞了别的词）不能放开权限
    assert_eq!(
        policy(r#"{"permissionMode":"FULL "}"#).permission_mode,
        PERMISSION_ASK
    );
    assert_eq!(
        policy(r#"{"permissionMode":"whatever"}"#).permission_mode,
        PERMISSION_ASK
    );
    assert!(policy(r#"{"permissionMode":"full"}"#).full_permission());
}

#[test]
fn 生效权限按权限档放宽但不动_deny() {
    let ask = policy("{}");
    assert_eq!(ask.access_for("web_search", "ASK"), ToolAccess::Ask);
    assert_eq!(ask.access_for("list_skills", "ALLOW"), ToolAccess::Allow);

    let full = policy(r#"{"permissionMode":"full"}"#);
    assert_eq!(
        full.access_for("web_search", "ASK"),
        ToolAccess::Allow,
        "全权限档下 ask 变 allow"
    );
    assert_eq!(
        full.access_for("x", "DENY"),
        ToolAccess::Deny,
        "deny 是禁用，不放宽"
    );
    assert_eq!(
        full.access_for("x", "???"),
        ToolAccess::Allow,
        "认不出的出厂档当 ask 处理"
    );
}

#[test]
fn 逐工具覆盖优先于出厂档() {
    let p = policy(r#"{"overrides":{"web_search":"deny","list_skills":"ask"}}"#);
    assert_eq!(p.access_for("web_search", "ASK"), ToolAccess::Deny);
    assert_eq!(p.access_for("list_skills", "ALLOW"), ToolAccess::Ask);
    // 覆盖值写坏了 → 回落出厂档（不是"随便给一个"）
    let broken = policy(r#"{"overrides":{"web_search":"nope"}}"#);
    assert_eq!(broken.access_for("web_search", "ASK"), ToolAccess::Ask);
}

/// 两条预算与 Kotlin 的出厂值一致，而且**老配置（没这两个键）走默认**。
#[test]
fn 工具预算的出厂值与_kotlin_一致() {
    let p = policy("{}");
    assert_eq!(p.config.max_tool_steps, 25);
    assert_eq!(p.config.max_calls_per_run, 48);
    // 老库里冻着的配置没有这两个键 → 必须落到出厂值，不是 0
    let legacy = policy(r#"{"writeRoots":["comfyui"],"permissionMode":"full"}"#);
    assert_eq!(legacy.config.max_tool_steps, DEFAULT_MAX_TOOL_STEPS);
    assert_eq!(legacy.config.max_calls_per_run, DEFAULT_MAX_CALLS_PER_RUN);
    // 手动调整过的值需生效
    let custom = policy(r#"{"maxToolSteps":8,"maxCallsPerRun":16}"#);
    assert_eq!(custom.config.max_tool_steps, 8);
    assert_eq!(custom.config.max_calls_per_run, 16);
}

/// 库中那份"16"是**旧默认值写入**的，不升级它则默认值调整不生效。
#[test]
fn 旧出厂值_deprecated_16_要升到新默认而不是永远锁着() {
    let mut old = ToolPolicyConfig {
        max_tool_steps: LEGACY_DEFAULT_MAX_TOOL_STEPS,
        ..Default::default()
    };
    migrate_legacy_max_tool_steps(&mut old);
    assert_eq!(old.max_tool_steps, DEFAULT_MAX_TOOL_STEPS);

    // 手动选择的值不得改动（设置页中 8 / 12 / 40 均为可选值）
    for chosen in [8, 12, 20, 40] {
        let mut custom = ToolPolicyConfig {
            max_tool_steps: chosen,
            ..Default::default()
        };
        migrate_legacy_max_tool_steps(&mut custom);
        assert_eq!(custom.max_tool_steps, chosen);
    }
}

/// 端到端那一半：真的从库里读一次 —— 旧值升上去、**写回库**、而且只升一次
/// （第二次读到的 16 是特意选择的，不得再改）。
#[test]
fn 升级会写回库并且只做一次() {
    let conn = crate::infra::db::open_memory().unwrap();
    let schema = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("db")
            .join("schema.sqlite.sql"),
    )
    .unwrap();
    conn.execute_batch(&schema).unwrap();
    conn.execute(
        "INSERT INTO app_settings (k, v) VALUES (?1, ?2)",
        rusqlite::params![
            KEY,
            r#"{"writeRoots":[],"readRoots":[],"overrides":{},"permissionMode":"full","maxToolSteps":16,"maxCallsPerRun":48,"maxReadBytes":2097152,"maxWriteBytes":4194304}"#
        ],
    )
    .unwrap();
    let state = AppState::new(conn, ":memory:", crate::infra::testpaths::scratch("media"));

    let first = ToolPolicy::load(&state).unwrap();
    assert_eq!(
        first.config.max_tool_steps, DEFAULT_MAX_TOOL_STEPS,
        "旧出厂值要当没设过"
    );
    // 写回库：别的键（我们这一版不认识的 maxReadBytes 等）一个都不能丢
    let stored: String = state
        .with(|conn| {
            conn.query_row("SELECT v FROM app_settings WHERE k = ?1", [KEY], |r| {
                r.get(0)
            })
        })
        .unwrap();
    assert!(stored.contains("\"maxToolSteps\":25"), "{stored}");
    assert!(
        stored.contains("maxReadBytes"),
        "合并写回，不许把别的键删掉：{stored}"
    );

    // 已明确选择 16（标记已写入）→ 第二次读取**不得**再改
    state
        .write(|tx| {
            tx.execute(
                "UPDATE app_settings SET v = json_set(v, '$.maxToolSteps', 16) WHERE k = ?1",
                [KEY],
            )?;
            Ok(())
        })
        .unwrap();
    let second = ToolPolicy::load(&state).unwrap();
    assert_eq!(
        second.config.max_tool_steps, 16,
        "标记落过之后，用户挑的值就是最终答案"
    );
}

// ------------------------------------------------------------------
//  路径准入（P4 第 40 步）
// ------------------------------------------------------------------

/// 每个用例一个**真实存在**的临时根（路径准入要解真实路径，光有字符串不够）。
fn temp_root(tag: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = crate::infra::testpaths::scratch(&format!("policy-{tag}-{seq}"));
    std::fs::create_dir_all(root.join("comfyui")).unwrap();
    std::fs::create_dir_all(root.join("storage")).unwrap();
    root
}

fn policy_at(root: &Path, json: &str) -> ToolPolicy {
    ToolPolicy::new(root.to_path_buf(), serde_json::from_str(json).unwrap())
}

#[test]
fn 写只能在写白名单里读只能在读白名单里() {
    let root = temp_root("roots");
    let p = policy_at(&root, "{}");
    // 写：出厂只有 comfyui
    let inside = p.resolve_write("comfyui/a/b.txt").unwrap();
    assert!(
        inside.starts_with(root.join("comfyui")),
        "{}",
        inside.display()
    );
    // 不存在也照样解析（write_file 的目标通常还不存在）
    assert!(inside.ends_with("b.txt"));
    // storage 是**读**白名单，不是写白名单
    let denied = p.resolve_write("storage/x.txt").unwrap_err();
    assert_eq!(denied.code, "PATH_DENIED");
    assert!(denied.message.contains("只允许在"), "{}", denied.message);
    // 读：comfyui 与 storage 都行
    assert!(p.resolve_read("storage/x.txt").is_ok());
    assert!(p.resolve_read("comfyui/x.txt").is_ok());
    // 项目根之外
    assert_eq!(
        p.resolve_read("../outside.txt").unwrap_err().code,
        "PATH_DENIED"
    );
    // 空路径是参数错，不是越界
    assert_eq!(p.resolve_write("   ").unwrap_err().code, "INVALID_ARGUMENT");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn 禁段与用户白名单无关() {
    let root = temp_root("forbidden");
    // 白名单放宽到**项目根**（最宽的写法）
    let p = policy_at(&root, r#"{"writeRoots":["."],"readRoots":["."]}"#);
    assert!(p.resolve_write("comfyui/ok.txt").is_ok());
    for bad in [
        ".git/config",
        ".run/x.log",
        "node_modules/a/b.js",
        ".mysql/data/x",
    ] {
        let err = p.resolve_write(bad).unwrap_err();
        assert_eq!(err.code, "PATH_DENIED", "{bad} 竟然放行了");
        assert!(
            err.message.contains("运行期数据 / 依赖目录"),
            "{}",
            err.message
        );
        // 读也一样拒（措辞要跟 verb 对上，别让"读被拒"看起来像"能读不能改"）
        let read_err = p.resolve_read(bad).unwrap_err();
        assert!(read_err.message.contains("拒绝读"), "{}", read_err.message);
    }
    // 大小写不敏感（Windows 上 `.GIT` 一样要拒）
    assert_eq!(
        p.resolve_write(".GIT/config").unwrap_err().code,
        "PATH_DENIED"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn 目录根本身不能当写入目标() {
    let root = temp_root("dirroot");
    let p = policy_at(&root, "{}");
    let err = p.resolve_write("comfyui").unwrap_err();
    assert_eq!(err.code, "PATH_DENIED");
    assert!(err.message.contains("目录根本身"), "{}", err.message);
    // 带尾斜杠写法归一到同一个目录，照样拒
    assert!(p.resolve_write("comfyui/").is_err());
    // **读**目录根本身是允许的（list_dir 的默认路径就是它）
    assert!(p.resolve_read("comfyui").is_ok());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn 自动发现的目录只放宽读() {
    let root = temp_root("auto");
    let outside = root.join("ComfyUI-Installs").join("ComfyUI");
    std::fs::create_dir_all(outside.join("user").join("default").join("workflows")).unwrap();
    let p = policy_at(&root, "{}").with_auto_read_roots(vec![outside.clone()]);
    // 读：自动发现的目录可以
    assert!(p
        .resolve_read(
            &outside
                .join("user")
                .join("default")
                .join("workflows")
                .join("a.json")
                .display()
                .to_string()
        )
        .is_ok());
    // 写：**一点都没放宽**（那只放宽读）
    let err = p
        .resolve_write(&outside.join("x.json").display().to_string())
        .unwrap_err();
    assert_eq!(err.code, "PATH_DENIED");
    // 去重 + 保序（Kotlin `.distinct()`）
    let dup =
        policy_at(&root, "{}").with_auto_read_roots(vec![root.join("storage"), outside.clone()]);
    assert_eq!(
        dup.effective_read_roots(),
        vec![root.join("comfyui"), root.join("storage"), outside]
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn 真实路径解掉符号链接与逐字前缀() {
    let root = temp_root("real");
    let p = policy_at(&root, "{}");
    std::fs::create_dir_all(root.join("comfyui").join("sub")).unwrap();
    std::fs::write(root.join("comfyui").join("sub").join("a.txt"), b"x").unwrap();
    let resolved = p.resolve_write("comfyui/sub/a.txt").unwrap();
    // Windows 的 canonicalize 会给 `\\?\D:\…`，闸门内部必须已经去掉那个前缀
    assert!(
        !resolved.to_string_lossy().starts_with(r"\\?\"),
        "{}",
        resolved.display()
    );
    // 词法上的 `..` 要被消掉（`comfyui/../comfyui/a.txt` 仍然合法）
    assert!(p.resolve_write("comfyui/../comfyui/b.txt").is_ok());
    // 但消完落在白名单外就要拒
    assert!(p.resolve_write("comfyui/../storage/x.txt").is_err());
    std::fs::remove_dir_all(&root).ok();
}

/// 非法字符这一档：Java 的 `Paths.get` 会抛 `InvalidPathException`，
/// Rust 的 `PathBuf` **什么都收** —— 不自己挡就会一路走到 `create_dir_all` 才报一个
/// 看不懂的 IO 错误（而 Kotlin 那边回的是 `PATH_DENIED`）。
#[test]
fn 非法字符按无法解析拒绝() {
    let root = temp_root("badchars");
    let p = policy_at(&root, "{}");
    let err = p.resolve_write("comfyui/a\u{0}b.txt").unwrap_err();
    assert_eq!(err.code, "PATH_DENIED");
    assert!(p.resolve_write("comfyui/a?b.txt").is_err());
    assert!(p.resolve_write("comfyui/a|b.txt").is_err());
    std::fs::remove_dir_all(&root).ok();
}
