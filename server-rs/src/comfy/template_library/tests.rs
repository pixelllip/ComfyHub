//! `template_library` 的用例。
//!
//! 全部用 [`crate::infra::testpaths::scratch`] 现造目录树（AGENTS §1：别用 `temp_dir()`）——
//! 不依赖这台机器上装没装 ComfyUI，也不会因为某个人的安装布局不同而红。

use std::path::{Path, PathBuf};

use super::*;

/// 造一棵"Desktop 那种两层"的安装树，返回 `(安装根, ComfyUI 根)`。
///
/// 形状照真机：`<安装根>\ComfyUI`（有 main.py / comfy/ / output/ / models/）
/// 与 `<安装根>\standalone-env\Lib\site-packages`（模板库在这儿）。
fn build_tree(label: &str, manifest: &str) -> (PathBuf, PathBuf) {
    let root = crate::infra::testpaths::scratch(label);
    let home = root.join("install").join("ComfyUI");
    std::fs::create_dir_all(home.join("comfy")).unwrap();
    std::fs::write(home.join("main.py"), "print('x')").unwrap();
    std::fs::create_dir_all(home.join("output")).unwrap();
    std::fs::create_dir_all(home.join("models")).unwrap();

    let site = root
        .join("install")
        .join("standalone-env")
        .join("Lib")
        .join("site-packages");
    let core = site.join(CORE_PACKAGE);
    std::fs::create_dir_all(&core).unwrap();
    std::fs::write(core.join("manifest.json"), manifest).unwrap();
    for (_, pkg) in BUNDLE_PACKAGES {
        std::fs::create_dir_all(site.join(pkg).join("templates")).unwrap();
    }
    (root, home)
}

/// 在 `media-image` 包里放一份模板正文。
fn put_template(root: &Path, filename: &str, body: &str) {
    let dir = root
        .join("install")
        .join("standalone-env")
        .join("Lib")
        .join("site-packages")
        .join("comfyui_workflow_templates_media_image")
        .join("templates");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(filename), body).unwrap();
}

const MANIFEST: &str = r#"{
  "manifest_version": 1,
  "bundles": { "media-image": { "version": "0.3.132" } },
  "templates": [
    {
      "id": "qwen_image_edit",
      "bundle": "media-image",
      "version": "0.3.132",
      "assets": [
        { "filename": "qwen_image_edit.json", "sha256": "aa" },
        { "filename": "qwen_image_edit-1.webp", "sha256": "bb" }
      ]
    },
    {
      "id": "flux_dev_checkpoint",
      "bundle": "media-image",
      "version": "0.3.132",
      "assets": [{ "filename": "flux_dev_checkpoint.json", "sha256": "cc" }]
    },
    {
      "id": "gone_template",
      "bundle": "media-image",
      "version": "0.3.132",
      "assets": [{ "filename": "gone_template.json", "sha256": "dd" }]
    }
  ]
}"#;

const UI_WORKFLOW: &str = r#"{"nodes": [], "links": [], "version": 0.4}"#;

#[test]
fn 从安装目录的上一级找到模板库() {
    let (root, home) = build_tree("tpl-discover", MANIFEST);
    put_template(&root, "qwen_image_edit.json", UI_WORKFLOW);

    let libs = libraries(&[home]);
    assert_eq!(
        libs.len(),
        1,
        "应当从 ComfyUI 根的上一级找到 standalone-env"
    );
    assert!(libs[0].manifest.ends_with("manifest.json"));
    assert!(libs[0].site_packages.ends_with("site-packages"));
    crate::infra::testpaths::cleanup(&root);
}

#[test]
fn 解析索引得到模板与预览图() {
    let (root, home) = build_tree("tpl-load", MANIFEST);
    put_template(&root, "qwen_image_edit.json", UI_WORKFLOW);
    put_template(&root, "qwen_image_edit-1.webp", "webp");
    put_template(&root, "flux_dev_checkpoint.json", UI_WORKFLOW);

    let libs = libraries(&[home]);
    let loaded = load(&libs[0]).expect("索引应当能读");

    assert_eq!(loaded.templates.len(), 2, "少了一份文件的那条不算");
    assert_eq!(loaded.missing, 1, "缺件必须如实计数，不许假装有");
    assert_eq!(loaded.unknown_bundles, 0);

    let qwen = loaded
        .templates
        .iter()
        .find(|t| t.id == "qwen_image_edit")
        .unwrap();
    assert_eq!(qwen.bundle, "media-image");
    assert_eq!(qwen.version, "0.3.132");
    assert!(
        qwen.file.is_file(),
        "file 必须是能直接喂给 comfy_load_workflow 的路径"
    );
    assert_eq!(qwen.previews.len(), 1);

    let detail = detail(qwen).expect("工作流能读出来");
    assert_eq!(detail.format, "ui", "模板是界面格式（nodes/links）");
    assert_eq!(detail.sha256.len(), 64);

    crate::infra::testpaths::cleanup(&root);
}

#[test]
fn 索引坏掉时如实报错而不是崩() {
    let (root, home) = build_tree("tpl-broken", "{ 这不是 JSON");
    let libs = libraries(&[home]);
    let err = load(&libs[0]).expect_err("坏索引必须报错");
    assert!(err.contains("不是合法 JSON"), "错误要说人话：{err}");
    crate::infra::testpaths::cleanup(&root);
}

#[test]
fn 按模型名搜索且短id优先() {
    let (root, home) = build_tree("tpl-search", MANIFEST);
    put_template(&root, "qwen_image_edit.json", UI_WORKFLOW);
    put_template(&root, "flux_dev_checkpoint.json", UI_WORKFLOW);
    let libs = libraries(&[home]);
    let loaded = load(&libs[0]).unwrap();

    let hits = search(&loaded.templates, Some("qwen"), 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "qwen_image_edit");

    // 两个词都要命中；顺序无所谓
    let hits = search(&loaded.templates, Some("image qwen"), 10);
    assert_eq!(hits.len(), 1);

    let hits = search(&loaded.templates, Some("sdxl"), 10);
    assert!(hits.is_empty(), "没有就别编一条出来");

    let all = search(&loaded.templates, None, 10);
    assert_eq!(all.len(), 2, "不带关键词就是全部");

    crate::infra::testpaths::cleanup(&root);
}

#[test]
fn 列出来的每一份都带着能不能用的结论() {
    let (root, home) = build_tree("tpl-json", MANIFEST);
    put_template(&root, "qwen_image_edit.json", UI_WORKFLOW);
    put_template(&root, "flux_dev_checkpoint.json", UI_WORKFLOW);
    let libs = libraries(&[home]);

    // 收下第二条的 sha，模拟"这份已经在库里了"
    let loaded = load(&libs[0]).unwrap();
    let flux = loaded
        .templates
        .iter()
        .find(|t| t.id == "flux_dev_checkpoint")
        .unwrap();
    let flux_sha = detail(flux).unwrap().sha256;

    let json = list_json(&libs, Some("flux"), 10, |sha| {
        (sha == flux_sha).then_some(7)
    });
    assert_eq!(json["total"], 2);
    assert_eq!(json["count"], 1);
    let entry = &json["templates"][0];
    assert_eq!(entry["id"], "flux_dev_checkpoint");
    assert_eq!(entry["format"], "ui");
    assert_eq!(entry["loadable"], true);
    assert_eq!(entry["inLibrary"], true);
    assert_eq!(entry["promptId"], 7);
    assert!(json["hint"]
        .as_str()
        .unwrap()
        .contains("comfy_load_workflow"));

    crate::infra::testpaths::cleanup(&root);
}

#[test]
fn 一份索引都没有时给一句人话() {
    let json = list_json(&[], None, 10, |_| None);
    assert_eq!(json["total"], 0);
    assert_eq!(json["ok"], true);
    let message = json["message"].as_str().unwrap();
    assert!(
        message.contains("comfy_list_workflows"),
        "要说下一步能做什么：{message}"
    );
}

#[test]
fn 认不出的bundle只计数不报错() {
    let manifest = r#"{"templates":[
      {"id":"x","bundle":"media-future","assets":[{"filename":"x.json"}]}
    ]}"#;
    let (root, home) = build_tree("tpl-future-bundle", manifest);
    let libs = libraries(&[home]);
    let loaded = load(&libs[0]).unwrap();
    assert!(loaded.templates.is_empty());
    assert_eq!(loaded.unknown_bundles, 1);
    crate::infra::testpaths::cleanup(&root);
}
