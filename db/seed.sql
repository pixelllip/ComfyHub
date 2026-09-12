-- =====================================================================
--  ComfyHub 演示数据（可重复执行）
-- =====================================================================
USE comfy_hub;

INSERT INTO tags (name, normalized, category, color) VALUES
  ('风景',      '风景',     '风格', '#4CAF50'),
  ('人像',      '人像',     '风格', '#E91E63'),
  ('赛博朋克',  '赛博朋克', '风格', '#9C27B0'),
  ('写实',      '写实',     '风格', '#607D8B'),
  ('水墨',      '水墨',     '风格', '#795548'),
  ('8K',        '8k',       '画质', '#FF9800'),
  ('高细节',    '高细节',   '画质', '#FF9800'),
  ('电影感',    '电影感',   '画质', '#3F51B5'),
  ('负面',      '负面',     '负面', '#F44336'),
  ('视频',      '视频',     '类型', '#00BCD4'),
  ('音乐',      '音乐',     '类型', '#8BC34A'),
  ('LoRA',      'lora',     '其它', '#009688')
ON DUPLICATE KEY UPDATE name = VALUES(name), category = VALUES(category), color = VALUES(color);

INSERT INTO prompts
  (title, kind, positive_prompt, negative_prompt, checkpoint, sampler, scheduler,
   steps, cfg_scale, seed, width, height, batch_size, notes, favorite)
VALUES
  ('雨夜霓虹街道 · 赛博朋克',
   'IMAGE',
   'cyberpunk city street at night, heavy rain, neon signs reflecting on wet asphalt, a lone figure with a translucent umbrella, volumetric fog, cinematic lighting, ultra detailed, 8k, photorealistic, shot on 35mm lens',
   'lowres, blurry, watermark, text, extra fingers, deformed',
   'sd_xl_base_1.0.safetensors',
   'dpmpp_2m', 'karras', 32, 7.5, 884213771, 1216, 832, 4,
   '测试用：雨夜氛围关键词组合效果最好的是 neon reflection + volumetric fog。', 1),

  ('古风少女 · 水墨',
   'IMAGE',
   'traditional chinese ink painting, a young woman in hanfu standing by a lotus pond, xieyi brush strokes, negative space, soft rice paper texture, subtle red seal stamp, masterpiece',
   'photo, 3d render, harsh lines, oversaturated',
   'flux1-dev.safetensors',
   'euler', 'simple', 28, 3.5, 1203948576, 1024, 1024, 1,
   '水墨风格建议 cfg 控制在 3~4。', 0),

  ('产品广告 · 缓慢环绕运镜',
   'VIDEO',
   'a matte black wireless earbud case slowly rotating on a reflective black pedestal, studio softbox lighting, shallow depth of field, slow orbit camera movement, dust particles in the light beam, 4k product commercial',
   'shake, flicker, text overlay, low quality',
   'svd_xt_1_1.safetensors',
   'euler', 'normal', 25, 6.0, 556677889, 1024, 576, 1,
   '视频用：运镜描述写在提示词末尾，效果更稳定。', 1),

  ('Lo-fi 氛围音乐 · 雨声',
   'AUDIO',
   'lofi hiphop instrumental, warm vinyl crackle, mellow rhodes piano chords, soft boom bap drums at 78 bpm, rain ambience layered underneath, nostalgic and calm, no vocals',
   NULL,
   'ace_step_v1.safetensors',
   NULL, NULL, 40, 5.0, 20250815, NULL, NULL, NULL,
   '音频用：bpm 与乐器要显式写出。', 0)
ON DUPLICATE KEY UPDATE title = VALUES(title);

-- 关联标签
UPDATE tags SET use_count = (
  SELECT COUNT(*) FROM prompt_tags pt WHERE pt.tag_id = tags.id
);

-- 建立演示关联：按标题匹配
INSERT IGNORE INTO prompt_tags (prompt_id, tag_id)
SELECT p.id, t.id FROM prompts p JOIN tags t
WHERE (p.title LIKE '雨夜霓虹%' AND t.normalized IN ('赛博朋克','夜景','写实','8k','高细节','电影感','负面'))
   OR (p.title LIKE '古风少女%' AND t.normalized IN ('水墨','人像','负面'))
   OR (p.title LIKE '产品广告%' AND t.normalized IN ('视频','电影感','高细节'))
   OR (p.title LIKE 'Lo-fi%'    AND t.normalized IN ('音乐','负面','lora'))
   OR (p.title LIKE 'Lo-fi%'    AND t.name = 'LoRA');

UPDATE tags SET use_count = (
  SELECT COUNT(*) FROM prompt_tags pt WHERE pt.tag_id = tags.id
);
