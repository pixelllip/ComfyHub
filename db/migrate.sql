-- =====================================================================
--  ComfyHub 增量迁移（幂等，可重复执行）
--
--  什么时候需要它？
--    · 后端启动时会自动做同样的事情（见 Migrate.kt），所以正常使用**不需要**手动跑；
--    · 但如果你想手动升级一个已存在的库，或者想看清楚改了哪些结构，用这个文件：
--        pwsh -File scripts\mysql.ps1 migrate
--      或者：
--        mysql -u root -P 3307 -h 127.0.0.1 < db/migrate.sql
--
--  和 schema.sql 的关系：
--    schema.sql 是「全新安装」，migrate.sql 是「老库升级」，两者结果一致。
-- =====================================================================

USE comfy_hub;

DROP PROCEDURE IF EXISTS comfyhub_migrate;

DELIMITER $$
CREATE PROCEDURE comfyhub_migrate()
BEGIN
  -- ---- prompts：来源标记 + 工作流快照 ----
  IF NOT EXISTS (SELECT 1 FROM information_schema.COLUMNS
                  WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'prompts' AND COLUMN_NAME = 'source') THEN
    ALTER TABLE prompts
      ADD COLUMN source VARCHAR(32) NULL DEFAULT 'Manual'
      COMMENT '来源: Manual / ComfyUI / ComfyUI-Import';
  END IF;

  IF NOT EXISTS (SELECT 1 FROM information_schema.COLUMNS
                  WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'prompts' AND COLUMN_NAME = 'source_ref') THEN
    ALTER TABLE prompts
      ADD COLUMN source_ref VARCHAR(128) NULL
      COMMENT '来源侧唯一标识（如 ComfyUI prompt_id）';
  END IF;

  IF NOT EXISTS (SELECT 1 FROM information_schema.COLUMNS
                  WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'prompts' AND COLUMN_NAME = 'workflow_json') THEN
    ALTER TABLE prompts
      ADD COLUMN workflow_json MEDIUMTEXT NULL
      COMMENT 'ComfyUI 界面格式工作流快照';
  END IF;

  IF NOT EXISTS (SELECT 1 FROM information_schema.STATISTICS
                  WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'prompts' AND INDEX_NAME = 'idx_prompts_source_ref') THEN
    ALTER TABLE prompts ADD INDEX idx_prompts_source_ref (source_ref);
  END IF;

  -- ---- media_assets：产物也记住来自哪次生成 ----
  IF NOT EXISTS (SELECT 1 FROM information_schema.COLUMNS
                  WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'media_assets' AND COLUMN_NAME = 'source_ref') THEN
    ALTER TABLE media_assets
      ADD COLUMN source_ref VARCHAR(128) NULL
      COMMENT '来源侧唯一标识（如 ComfyUI prompt_id）';
  END IF;

  IF NOT EXISTS (SELECT 1 FROM information_schema.STATISTICS
                  WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'media_assets' AND INDEX_NAME = 'idx_media_source_ref') THEN
    ALTER TABLE media_assets ADD INDEX idx_media_source_ref (source_ref);
  END IF;

  -- ---- 自动捕获运行记录 ----
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

  -- ---- 运行期设置（自动捕获开关 / 地址 / 目录） ----
  CREATE TABLE IF NOT EXISTS app_settings (
    k          VARCHAR(96)  NOT NULL,
    v          MEDIUMTEXT   NULL,
    updated_at DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3)
                            ON UPDATE CURRENT_TIMESTAMP(3),
    PRIMARY KEY (k)
  ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
END$$

DELIMITER ;

CALL comfyhub_migrate();
DROP PROCEDURE comfyhub_migrate;

SELECT '迁移完成' AS result;

-- ---------------------------------------------------------------------
-- AI 工作台：Provider 与模型目录（M1 / AIH-006 / AIH-007 / AIH-010 / AIH-011）
-- 凭据值不进库：只有 credential_ref 这个名字，值由 DPAPI 加密单独保管（AIH-012/015）
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS ai_providers (
  id             VARCHAR(96)   NOT NULL COMMENT '小写 kebab-case，创建后不可改',
  display_name   VARCHAR(128)  NOT NULL,
  api            VARCHAR(32)   NOT NULL COMMENT 'openai-completions / openai-responses / anthropic-messages',
  base_url       VARCHAR(1024) NOT NULL,
  credential_ref VARCHAR(128)  NULL COMMENT '凭据引用名；值由 DPAPI 单独保管',
  endpoint_trust VARCHAR(32)   NOT NULL DEFAULT 'public',
  headers_json   JSON          NULL COMMENT '高级配置；禁止放密钥',
  compat_json    JSON          NULL,
  enabled        TINYINT(1)    NOT NULL DEFAULT 1,
  revision       BIGINT        NOT NULL DEFAULT 1 COMMENT '乐观锁',
  created_at     DATETIME(3)   NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
  updated_at     DATETIME(3)   NOT NULL DEFAULT CURRENT_TIMESTAMP(3)
                               ON UPDATE CURRENT_TIMESTAMP(3),
  PRIMARY KEY (id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS ai_models (
  provider_id            VARCHAR(96)  NOT NULL,
  model_id               VARCHAR(191) NOT NULL,
  display_name           VARCHAR(191) NOT NULL,
  input_modalities       JSON         NULL COMMENT '能力真源，未知即不支持',
  attachment_transports  JSON         NULL,
  mime_allowlist         JSON         NULL,
  tools                  TINYINT(1)   NOT NULL DEFAULT 0,
  parallel_tools         TINYINT(1)   NOT NULL DEFAULT 0,
  reasoning              TINYINT(1)   NOT NULL DEFAULT 0,
  context_window         INT          NULL,
  max_output_tokens      INT          NULL,
  max_attachment_bytes   BIGINT       NULL,
  max_attachment_count   INT          NULL,
  limits_json            JSON         NULL,
  capability_source      VARCHAR(16)  NOT NULL DEFAULT 'manual'
                                      COMMENT 'builtin / discovered / manual / tested',
  capability_verified_at DATETIME(3)  NULL,
  enabled                TINYINT(1)   NOT NULL DEFAULT 1,
  created_at             DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
  updated_at             DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3)
                                      ON UPDATE CURRENT_TIMESTAMP(3),
  PRIMARY KEY (provider_id, model_id),
  CONSTRAINT fk_ai_models_provider FOREIGN KEY (provider_id)
    REFERENCES ai_providers (id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
