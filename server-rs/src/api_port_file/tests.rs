//! `api_port_file` 的单元测试（主体代码在 [`api_port_file`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn temp_root(label: &str) -> PathBuf {
    crate::testpaths::scratch(&format!("portfile-{label}"))
}

fn state(port: u16, pid: u32) -> ApiPortState {
    ApiPortState {
        port,
        pid,
        host: "127.0.0.1".to_string(),
        url: url_for("127.0.0.1", port),
        started_at: "2026-09-26T01:00:00Z".to_string(),
        requested_port: port,
        fell_back: false,
    }
}

#[test]
fn 形状与_kotlin_逐键一致() {
    let json = state(51000, 4242).to_json();
    let keys: Vec<&str> = json.as_object().unwrap().keys().map(|k| k.as_str()).collect();
    assert_eq!(
        keys,
        vec!["port", "pid", "host", "url", "startedAt", "requestedPort", "fellBack"],
        "键集合与 Kotlin `ApiPortState` 必须逐字一致（脚本与前端按名字读）"
    );
    assert_eq!(json["url"], serde_json::json!("http://127.0.0.1:51000"));
    assert_eq!(json["fellBack"], serde_json::json!(false));
}

#[test]
fn 写与读往返一致_且落在_run_目录下() {
    let root = temp_root("roundtrip");
    let written = write(&root, &state(51234, 999)).expect("要写得下去");
    assert_eq!(written, root.join(RUN_DIR).join(FILE_NAME));
    assert!(written.is_file());
    // `.tmp` 不许留下来（原子替换掉了）
    assert!(!root.join(RUN_DIR).join(format!("{FILE_NAME}.tmp")).exists());
    let read_back = read(&root).expect("读得回来");
    assert_eq!(read_back.port, 51234);
    assert_eq!(read_back.pid, 999);
    assert_eq!(read_back.url, "http://127.0.0.1:51234");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn 只删自己那一份() {
    let root = temp_root("owned");
    write(&root, &state(52000, 111)).unwrap();
    // 别的进程来删：不许删（重启场景下新进程的记录不能被旧进程收尾删掉）
    assert!(!delete_if_owned(&root, 222));
    assert!(path(&root).is_file(), "不属于自己的那份必须留着");
    // 自己来删：删掉
    assert!(delete_if_owned(&root, 111));
    assert!(!path(&root).is_file());
    // 文件本来就不在：也回 false（不是错误）
    assert!(!delete_if_owned(&root, 111));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn 坏文件与缺文件都回_none_而不是崩() {
    let root = temp_root("broken");
    assert!(read(&root).is_none());
    std::fs::create_dir_all(root.join(RUN_DIR)).unwrap();
    std::fs::write(path(&root), "{ 这不是完整 JSON").unwrap();
    assert!(read(&root).is_none(), "半截 JSON 要当读不到");
    std::fs::write(path(&root), r#"{"pid":1}"#).unwrap();
    assert!(read(&root).is_none(), "缺 port 的要当读不到");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn 全部网卡地址要给客户端回落到回环() {
    assert_eq!(url_for("0.0.0.0", 8080), "http://127.0.0.1:8080");
    assert_eq!(url_for("::", 8080), "http://127.0.0.1:8080");
    assert_eq!(url_for("", 8080), "http://127.0.0.1:8080");
    assert_eq!(url_for("127.0.0.1", 8080), "http://127.0.0.1:8080");
    assert_eq!(url_for("192.168.1.9", 8080), "http://192.168.1.9:8080");
    // IPv6 字面量要加方括号，否则拼出来的 URL 连不上
    assert_eq!(url_for("::1", 8080), "http://[::1]:8080");
    assert_eq!(url_for("[::1]", 8080), "http://[::1]:8080");
}

#[test]
fn 回落端口也要如实标出来() {
    let root = temp_root("fellback");
    let mut state = state(51234, 7);
    state.requested_port = 8280;
    state.fell_back = true;
    write(&root, &state).unwrap();
    let read_back = read(&root).unwrap();
    assert_eq!(read_back.requested_port, 8280, "要能看出用户原本要的是哪个端口");
    assert!(read_back.fell_back, "`doctor` 靠它说清\"你给的端口被占了，换成了 X\"");
    let _ = std::fs::remove_dir_all(root);
}
