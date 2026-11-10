//! `comfy_dirs` 的单元测试（主体代码在 [`comfy_dirs`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn tmpdir(tag: &str) -> PathBuf {
    crate::infra::testpaths::scratch(&format!("comfy-dirs-{tag}"))
}

/// 造一个"源码形态"的 ComfyUI 根。
fn make_home(root: &Path) -> PathBuf {
    let home = root.join("ComfyUI");
    std::fs::create_dir_all(home.join("comfy")).unwrap();
    std::fs::write(home.join("main.py"), "print(1)").unwrap();
    home
}

fn write_workflow(dir: &Path, name: &str, body: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(name), body).unwrap();
}

#[test]
fn 固定盘列表只包含真的存在的盘根_而且不只认某一个盘() {
    // 判据是"这台机器上有什么"，不是"开发机上是 D:\"：
    // · 每一项都必须真的存在（网络盘 / 光驱被 GetDriveTypeW 过滤掉了）；
    // · 至少包含系统盘（`SystemDrive`，任何 Windows 都有）—— 这样"ComfyUI 装在 C:"
    //   的机器不会因为写死 D: 而找不到。
    let roots = fixed_drive_roots();
    for root in &roots {
        assert!(root.is_dir(), "盘根不存在：{}", root.display());
        assert_eq!(
            root.to_string_lossy().chars().count(),
            3,
            "应当是 `X:\\`：{}",
            root.display()
        );
    }
    if let Ok(system) = std::env::var("SystemDrive") {
        let expected = format!("{system}\\");
        assert!(
            roots.iter().any(|r| r.to_string_lossy().eq_ignore_ascii_case(&expected)),
            "固定盘里必须有系统盘 {expected}，实际：{roots:?}"
        );
    }
}

#[test]
fn 每个固定盘都会贡献_comfyui_候选位置() {
    // 候选是"每个固定盘 × 两种常见布局"，盘符来自机器本身 —— 换台机器也是同一套规则。
    let candidates = drive_install_candidates();
    assert_eq!(candidates.len(), fixed_drive_roots().len() * 2);
    for (path, kind) in &candidates {
        assert!(path.is_absolute(), "{}", path.display());
        assert!(kind.contains("ComfyUI"), "说明要能让人看懂：{kind}");
    }
}

#[test]
fn looks_like_comfy_两种形态都认_只有_output_不认() {
    let root = tmpdir("looks");
    let src = make_home(&root);
    assert!(looks_like_comfy(&src).0);

    let any = root.join("portable");
    std::fs::create_dir_all(any.join("output")).unwrap();
    std::fs::create_dir_all(any.join("models")).unwrap();
    assert!(
        looks_like_comfy(&any).0,
        "output/ + models/ 是任何形态都有的"
    );

    let bogus = root.join("随便建的");
    std::fs::create_dir_all(bogus.join("output")).unwrap();
    let (ok, _reason) = looks_like_comfy(&bogus);
    assert!(
        !ok,
        "只有一个空 output/ 不算 ComfyUI（误判的代价是捕获一条也收不到）"
    );
}

#[test]
fn 三个目录来源都要认_少搬一条就会静默少结果() {
    let root = tmpdir("sources");
    // ① 配置里的输出目录（父目录 = ComfyUI 根）
    let configured_home = make_home(&root.join("from-config"));
    let output = configured_home.join("output");
    std::fs::create_dir_all(&output).unwrap();
    let cfg_dir = configured_home
        .join("user")
        .join("default")
        .join("workflows");
    write_workflow(&cfg_dir, "a.json", r#"{"nodes":[]}"#);

    // ② 项目内的 comfyui（探测那一档）
    let project_root = root.join("project");
    let project_home = make_home(&project_root);
    let proj_dir = project_home.join("user").join("default").join("workflows");
    write_workflow(&proj_dir, "b.json", r#"{"nodes":[]}"#);

    let dirs = workflow_dirs(&project_root, Some(&output.to_string_lossy()));
    let names: Vec<String> = dirs
        .iter()
        .map(|d| d.to_string_lossy().to_string())
        .collect();
    assert!(
        names.iter().any(
            |n| n.ends_with("from-config\\ComfyUI\\user\\default\\workflows")
                || n.ends_with("from-config/ComfyUI/user/default/workflows")
        ),
        "① 配置的输出目录那一档：{names:?}"
    );
    assert!(
        names.iter().any(|n| n.contains("project")),
        "② 项目内 comfyui 那一档（ComfyLocator 探测）：{names:?}"
    );
}

#[test]
fn 只列真的能用的_坏_json_空文件_非_json_与未知格式都跳过() {
    let root = tmpdir("scan");
    let dir = root.join("workflows");
    write_workflow(&dir, "ui.json", r#"{"nodes":[],"links":[]}"#);
    write_workflow(
        &dir,
        "api.json",
        r#"{"1":{"class_type":"KSampler","inputs":{}}}"#,
    );
    write_workflow(&dir, "broken.json", "{ 这不是 json");
    write_workflow(&dir, "weird.json", r#"{"hello":"world"}"#);
    write_workflow(&dir, "notes.txt", r#"{"nodes":[]}"#);
    write_workflow(&dir, ".hidden.json", r#"{"nodes":[]}"#);
    std::fs::write(dir.join("empty.json"), "").unwrap();

    let files = scan(std::slice::from_ref(&dir), None, MAX_FILES);
    let mut names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    names.sort();
    assert_eq!(
        names,
        vec!["api.json", "ui.json"],
        "只列 ui/api 两种真能跑的"
    );
    assert_eq!(
        files.iter().find(|f| f.name == "ui.json").unwrap().format,
        "ui"
    );
    assert_eq!(
        files.iter().find(|f| f.name == "api.json").unwrap().format,
        "api"
    );
    // sha256 是内容寻址的键（`comfy_load_workflow` 用 file:<sha256>），必须真的是 64 位 hex
    let sha = &files.iter().find(|f| f.name == "ui.json").unwrap().sha256;
    assert_eq!(sha.len(), 64);
    assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn query_是按文件名的不区分大小写子串() {
    let root = tmpdir("query");
    let dir = root.join("workflows");
    write_workflow(&dir, "krea2_hd.json", r#"{"nodes":[]}"#);
    write_workflow(&dir, "anima_t2i.json", r#"{"nodes":[]}"#);

    let hit = scan(std::slice::from_ref(&dir), Some("KREA2"), MAX_FILES);
    assert_eq!(hit.len(), 1);
    assert_eq!(hit[0].name, "krea2_hd.json");
    assert!(scan(&[dir], Some("没有这个"), MAX_FILES).is_empty());
}

#[test]
fn 清单的键集合与_是否入库如实反映() {
    let root = tmpdir("json");
    let dir = root.join("workflows");
    write_workflow(&dir, "a.json", r#"{"nodes":[]}"#);

    let json = list_json(std::slice::from_ref(&dir), None, 50, |sha| {
        if sha.len() == 64 {
            Some(310)
        } else {
            None
        }
    });
    let obj = json.as_object().unwrap();
    let mut keys: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["count", "dirs", "files", "hint", "total"],
        "少一个键 = 界面上少一块内容"
    );
    let file = &obj["files"][0];
    assert_eq!(file["inLibrary"], true);
    assert_eq!(file["promptId"], 310);

    // 没入库的那份不给 promptId（别编一个 0 出来）
    let json = list_json(&[dir], None, 50, |_| None);
    let file = &json["files"][0];
    assert_eq!(file["inLibrary"], false);
    assert!(file.get("promptId").is_none());
}

#[test]
fn 没有目录时给出可操作的那句话_而不是空清单() {
    let json = list_json(&[], None, 50, |_| None);
    assert_eq!(json["count"], 0);
    let message = json["message"].as_str().unwrap();
    assert!(message.contains("没找到本机 ComfyUI"), "{message}");
}

#[test]
fn 返回的路径不带_windows_逐字前缀() {
    // `canonicalize()` 在 Windows 上给 `\\?\D:\…`，而 Kotlin `toRealPath()` 给 `D:\…`。
    // 这个前缀会**原样出现在接口里**（`dirs` 与每个文件的 `path`），用户 / AI 都会看到，
    // 还会被贴进对话当参数 —— 实测真库副本上第一版就是 `\\?\D:\Comfy-Desktop\…`。
    let root = tmpdir("verbatim");
    let home = make_home(&root);
    let dir = home.join("user").join("default").join("workflows");
    write_workflow(&dir, "a.json", r#"{"nodes":[]}"#);

    let dirs = dirs_from_homes(&[home]);
    assert_eq!(dirs.len(), 1);
    let text = dirs[0].to_string_lossy().to_string();
    assert!(!text.starts_with(r"\\?\"), "不能带逐字前缀：{text}");

    let files = scan(&dirs, None, 10);
    let path = files[0].path.clone();
    assert!(!path.starts_with(r"\\?\"), "文件 path 也不能带：{path}");
}

#[test]
fn 一小时前的文件时间戳是_iso8601_utc() {
    let root = tmpdir("time");
    let dir = root.join("workflows");
    write_workflow(&dir, "a.json", r#"{"nodes":[]}"#);
    let files = scan(&[dir], None, 10);
    let ts = files[0].modified_at.as_deref().unwrap();
    // 形状：2026-09-24T10:00:00.000Z —— 前端 DateTime.parse 能吃
    assert!(ts.ends_with('Z'), "{ts}");
    assert_eq!(&ts[4..5], "-");
    assert_eq!(&ts[10..11], "T");
    // 小数位：0 / 3 / 6 / 9 位（Java `Instant.toString()` 的规则，Kotlin 侧逐个比过）
    let frac = ts.trim_end_matches('Z').split('.').nth(1).unwrap_or("");
    assert!(
        matches!(frac.len(), 0 | 3 | 6 | 9),
        "小数位只能是 0/3/6/9：{ts}"
    );
}

/// 小数位规则（三位一组去尾零）现在住在 [`crate::infra::iso_time`] —— 那里有用例专门钉它。
/// 这里只留"文件时间戳真的走了那条路"的接线检查。
#[test]
fn 文件时间戳走的是共享的_iso_时间格式化() {
    use std::time::{Duration, UNIX_EPOCH};
    let at = |nanos: u64| {
        crate::infra::iso_time::from_system_time(
            UNIX_EPOCH + Duration::new(1_774_512_000, nanos as u32),
        )
    };
    // 整秒：**没有小数点**
    assert_eq!(at(0), "2026-03-26T08:00:00Z");
    assert_eq!(at(1_000_000), "2026-03-26T08:00:00.001Z", "1ms → 3 位");
    assert_eq!(at(1_000), "2026-03-26T08:00:00.000001Z", "1µs → 6 位");
    assert_eq!(at(200), "2026-03-26T08:00:00.000000200Z", "200ns → 9 位");
    // Windows 的 FileTime 精度是 100ns：实测真机上就是这个形状
    assert_eq!(at(917_303_200), "2026-03-26T08:00:00.917303200Z");
}
