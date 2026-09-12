-- =====================================================================
--  ComfyHub - ComfyUI 提示词 / 生成产物 管理数据库
--  MySQL 8.0+ / utf8mb4
--  执行方式: mysql -u root -P 3307 -h 127.0.0.1 < db/schema.sql
-- =====================================================================

CREATE DATABASE IF NOT EXISTS comfy_hub
  DEFAULT CHARACTER SET utf8mb4
  DEFAULT COLLATE utf8mb4_unicode_ci;

USE comfy_hub;

-- ---------------------------------------------------------------------
-- 提示词主表
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS prompts (
  id                BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  title             VARCHAR(255)    NOT NULL DEFAULT '',
  kind              ENUM('IMAGE','VIDEO','AUDIO','MIXED') NOT NULL DEFAULT 'IMAGE'
                    COMMENT '该提示词面向的生成类型',
  positive_prompt   MEDIUMTEXT      NOT NULL,
  negative_prompt   MEDIUMTEXT      NULL,

  -- ComfyUI / 生成参数
  checkpoint        VARCHAR(255)    NULL COMMENT '模型 / checkpoint 名称',
  loras             JSON            NULL COMMENT 'LoRA 列表 [{name, weight}]',
  sampler           VARCHAR(128)    NULL,
  scheduler         VARCHAR(128)    NULL,
  steps             INT             NULL,
  cfg_scale         DECIMAL(6,2)    NULL,
  seed              BIGINT          NULL,
  width             INT             NULL,
  height            INT             NULL,
  batch_size        INT             NULL,
  extra_params      JSON            NULL COMMENT '其它任意参数 k/v',

  notes             TEXT            NULL,
  favorite          TINYINT(1)      NOT NULL DEFAULT 0,

  -- 来源追踪：自动捕获（ComfyUI 轮询 / 自定义节点推送 / 历史导入）会写这几个字段
  source            VARCHAR(32)     NULL DEFAULT 'Manual'
                    COMMENT '来源: Manual / ComfyUI / ComfyUI-Import',
  source_ref        VARCHAR(128)    NULL COMMENT '来源侧唯一标识（如 ComfyUI prompt_id）',
  workflow_json     MEDIUMTEXT      NULL COMMENT 'ComfyUI 界面格式工作流快照',

  created_at        DATETIME(3)     NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
  updated_at        DATETIME(3)     NOT NULL DEFAULT CURRENT_TIMESTAMP(3)
                                    ON UPDATE CURRENT_TIMESTAMP(3),

  PRIMARY KEY (id),
  KEY idx_prompts_kind (kind),
  KEY idx_prompts_created (created_at),
  KEY idx_prompts_favorite (favorite),
  KEY idx_prompts_source_ref (source_ref),
  FULLTEXT KEY ft_prompts (title, positive_prompt, negative_prompt, notes)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- ---------------------------------------------------------------------
-- 标签表（全局词表）
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS tags (
  id          BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  name        VARCHAR(96)     NOT NULL COMMENT '展示名',
  normalized  VARCHAR(96)     NOT NULL COMMENT '小写去空格的唯一键',
  category    VARCHAR(64)     NULL COMMENT '分组: 风格/角色/画质/负面/其它',
  color       VARCHAR(16)     NULL COMMENT '#RRGGBB',
  description VARCHAR(512)    NULL,
  use_count   INT             NOT NULL DEFAULT 0 COMMENT '冗余计数，便于热度排序',
  created_at  DATETIME(3)     NOT NULL DEFAULT CURRENT_TIMESTAMP(3),

  PRIMARY KEY (id),
  UNIQUE KEY uk_tags_normalized (normalized),
  KEY idx_tags_category (category),
  KEY idx_tags_use_count (use_count)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- ---------------------------------------------------------------------
-- 提示词 <-> 标签
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS prompt_tags (
  prompt_id BIGINT UNSIGNED NOT NULL,
  tag_id    BIGINT UNSIGNED NOT NULL,
  PRIMARY KEY (prompt_id, tag_id),
  KEY idx_pt_tag (tag_id),
  CONSTRAINT fk_pt_prompt FOREIGN KEY (prompt_id) REFERENCES prompts (id) ON DELETE CASCADE,
  CONSTRAINT fk_pt_tag    FOREIGN KEY (tag_id)    REFERENCES tags (id)    ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- ---------------------------------------------------------------------
-- 生成产物（图片 / 视频 / 音频）
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS media_assets (
  id             BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  prompt_id      BIGINT UNSIGNED NULL COMMENT '关联提示词，可空',
  kind           ENUM('IMAGE','VIDEO','AUDIO') NOT NULL,
  title          VARCHAR(255)    NOT NULL DEFAULT '',
  original_name  VARCHAR(512)    NOT NULL,
  stored_name    VARCHAR(255)    NOT NULL COMMENT 'storage 目录下的文件名',
  mime_type      VARCHAR(128)    NULL,
  size_bytes     BIGINT UNSIGNED NOT NULL DEFAULT 0,
  width          INT             NULL,
  height         INT             NULL,
  duration_ms    BIGINT          NULL,
  sha256         CHAR(64)        NULL,
  source         VARCHAR(128)    NULL DEFAULT 'ComfyUI',
  source_ref     VARCHAR(128)    NULL COMMENT '来源侧唯一标识（如 ComfyUI prompt_id）',
  workflow_json  JSON            NULL COMMENT 'ComfyUI workflow / API 参数快照',
  notes          TEXT            NULL,
  favorite       TINYINT(1)      NOT NULL DEFAULT 0,
  created_at     DATETIME(3)     NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
  updated_at     DATETIME(3)     NOT NULL DEFAULT CURRENT_TIMESTAMP(3)
                                 ON UPDATE CURRENT_TIMESTAMP(3),

  PRIMARY KEY (id),
  KEY idx_media_prompt (prompt_id),
  KEY idx_media_kind (kind),
  KEY idx_media_created (created_at),
  KEY idx_media_sha256 (sha256),
  KEY idx_media_source_ref (source_ref),
  CONSTRAINT fk_media_prompt FOREIGN KEY (prompt_id) REFERENCES prompts (id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- ---------------------------------------------------------------------
-- 生成产物 <-> 标签（产物自己也可以打标签）
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS media_tags (
  media_id BIGINT UNSIGNED NOT NULL,
  tag_id   BIGINT UNSIGNED NOT NULL,
  PRIMARY KEY (media_id, tag_id),
  KEY idx_mt_tag (tag_id),
  CONSTRAINT fk_mt_media FOREIGN KEY (media_id) REFERENCES media_assets (id) ON DELETE CASCADE,
  CONSTRAINT fk_mt_tag   FOREIGN KEY (tag_id)   REFERENCES tags (id)        ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- ---------------------------------------------------------------------
-- 自动捕获：每一次 ComfyUI 运行的记录（run_key 唯一 => 同一次生成只入库一次）
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS capture_runs (
  id          BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  run_key     VARCHAR(191)    NOT NULL COMMENT '来源侧唯一键（ComfyUI prompt_id / import:<sha256>）',
  source      VARCHAR(32)     NOT NULL DEFAULT 'ComfyUI',
  prompt_id   BIGINT UNSIGNED NULL,
  status      VARCHAR(32)     NOT NULL DEFAULT 'success' COMMENT 'success / error / running',
  media_count INT             NOT NULL DEFAULT 0,
  title       VARCHAR(255)    NULL,
  error       TEXT            NULL,
  raw         JSON            NULL COMMENT '原始 history 片段（便于排查）',
  created_at  DATETIME(3)     NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
  PRIMARY KEY (id),
  UNIQUE KEY uk_capture_runs_key (run_key),
  KEY idx_capture_runs_created (created_at),
  KEY idx_capture_runs_prompt (prompt_id),
  CONSTRAINT fk_capture_runs_prompt FOREIGN KEY (prompt_id) REFERENCES prompts (id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- ---------------------------------------------------------------------
-- 运行期设置（自动捕获开关 / ComfyUI 地址 / 输出目录等，App 与后端共用）
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS app_settings (
  k          VARCHAR(96)  NOT NULL,
  v          MEDIUMTEXT   NULL,
  updated_at DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3)
                          ON UPDATE CURRENT_TIMESTAMP(3),
  PRIMARY KEY (k)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- ---------------------------------------------------------------------
-- 视图：产物 + 关联提示词 + 聚合标签（供列表页一次查出）
-- ---------------------------------------------------------------------
CREATE OR REPLACE VIEW v_media_full AS
SELECT
  m.id,
  m.prompt_id,
  m.kind,
  m.title,
  m.original_name,
  m.stored_name,
  m.mime_type,
  m.size_bytes,
  m.width,
  m.height,
  m.duration_ms,
  m.sha256,
  m.source,
  m.favorite,
  m.notes,
  m.created_at,
  m.updated_at,
  p.title            AS prompt_title,
  p.positive_prompt  AS prompt_positive,
  p.negative_prompt  AS prompt_negative,
  p.checkpoint,
  p.seed,
  (
    SELECT COALESCE(GROUP_CONCAT(t.name ORDER BY t.name SEPARATOR ','), '')
    FROM prompt_tags pt JOIN tags t ON t.id = pt.tag_id
    WHERE pt.prompt_id = m.prompt_id
  ) AS prompt_tag_names
FROM media_assets m
LEFT JOIN prompts p ON p.id = m.prompt_id;
