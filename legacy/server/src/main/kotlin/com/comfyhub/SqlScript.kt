package com.comfyhub

/**
 * 把一份 SQL 脚本切成**可逐句执行**的语句列表。
 *
 * 为什么需要它（而不是 `sql.split(";")`）：`db/schema.sqlite.sql` 里的
 * `CREATE TRIGGER … BEGIN … ; END;` **体内就有分号** —— 按分号硬切会把一个触发器
 * 切成两半，报出来的是"near \"END\": syntax error"，非常难往触发器上想。
 *
 * 处理三件事：
 *  · 单引号字符串里的分号 / 注释符不算标记；
 *  · 行注释（两个减号起）与块注释（斜杠星号起）整段忽略；
 *  · 触发器语句一直攒到 `END;` 才算一句。
 */
internal object SqlScript {

    fun split(sql: String): List<String> {
        val out = mutableListOf<String>()
        val cur = StringBuilder()
        var i = 0
        val n = sql.length
        while (i < n) {
            val c = sql[i]

            // --- 行注释：整行跳过（保留换行，别把两句粘一起）---
            if (c == '-' && i + 1 < n && sql[i + 1] == '-') {
                while (i < n && sql[i] != '\n') i++
                cur.append('\n')
                continue
            }
            // --- 块注释 ---
            if (c == '/' && i + 1 < n && sql[i + 1] == '*') {
                i += 2
                while (i + 1 < n && !(sql[i] == '*' && sql[i + 1] == '/')) i++
                i += 2
                continue
            }
            // --- 单引号字符串：原样抄进去，里面的分号不当分隔符 ---
            if (c == '\'') {
                cur.append(c)
                i++
                while (i < n) {
                    cur.append(sql[i])
                    if (sql[i] == '\'') {
                        // SQL 里 '' 是转义的单引号，不是字符串结束
                        if (i + 1 < n && sql[i + 1] == '\'') {
                            cur.append(sql[i + 1])
                            i += 2
                            continue
                        }
                        i++
                        break
                    }
                    i++
                }
                continue
            }

            if (c == ';') {
                val text = cur.toString()
                if (isCompleteTrigger(text)) {
                    out += text.trim()
                    cur.setLength(0)
                } else {
                    // 触发器体内 / 或还没到 END：继续攒
                    cur.append(c)
                }
                i++
                continue
            }

            cur.append(c)
            i++
        }
        val tail = cur.toString().trim()
        if (tail.isNotEmpty()) out += tail
        return out.filter { it.isNotBlank() }
    }

    /** 触发器要攒到 `END;` 才算一句；其余语句见到分号就结束。 */
    private fun isCompleteTrigger(text: String): Boolean {
        if (!text.contains(Regex("""(?i)CREATE\s+TRIGGER"""))) return true
        return text.trimEnd().endsWith("END", ignoreCase = true)
    }
}
