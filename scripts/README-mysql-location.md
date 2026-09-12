# 把本地 MySQL 放到别的盘（存储位置可配置）

便携版 MySQL 的**实例目录**默认是项目里的 `.mysql`，里面有：

| 内容 | 路径 |
| --- | --- |
| 真正的数据文件（InnoDB） | `<实例目录>\data` |
| 配置 | `<实例目录>\my.ini` |
| 错误日志 | `<实例目录>\mysql-error.log` |
| 进程号 | `<实例目录>\mysqld.pid` |

想放到别的盘 / 别的目录（比如 C 盘紧张，想丢到 D 盘），有三种办法。

## 一、解析顺序（四个脚本共用同一套规则）

任何需要实例目录的脚本（`mysql.ps1`、`comfyhub.ps1`、`server.ps1`）都按下面的顺序解析，
先命中先用，最后结果会被规范化成**绝对路径**（相对路径按项目根目录展开）：

1. 命令行参数 `-DataDir <路径>`
2. 环境变量 `COMFYHUB_MYSQL_DIR`
3. 指针文件 `<项目>\.mysql-location.json` 里的 `dataDir`（内容形如 `{"dataDir": "D:\\mysql\\comfyhub"}`）
4. 默认 `<项目>\.mysql`

```powershell
# 只在这一次命令里生效
pwsh -File scripts\mysql.ps1 status -DataDir 'D:\mysql\comfyhub'

# 让当前终端里所有命令都用这个目录
$env:COMFYHUB_MYSQL_DIR = 'D:\mysql\comfyhub'
pwsh -File scripts\comfyhub.ps1 up

# 一劳永逸：写进指针文件（推荐用下面的 move，它会自动写）
```

> 注意：`-DataDir` 是"一次性"的，不会自动记到指针文件里。要让后来所有命令都记住，
> 用 `move` 搬家（会自动写指针文件），或者自己设环境变量。

## 二、搬家：`mysql.ps1 move`

把整个实例目录搬到新位置（**不会删除源目录**）：

```powershell
# 先看看现在用的是哪个目录
pwsh -File scripts\mysql.ps1 status

# 搬家到 D:\mysql\comfyhub
pwsh -File scripts\mysql.ps1 move -DataDir 'D:\mysql\comfyhub'
```

`move` 做的事，按顺序：

1. 停掉 MySQL（**不停库直接复制 InnoDB 文件会复制出一份坏数据**）；
2. 目标目录里已经有 `data\` 就拒绝执行（确认要覆盖请加 `-Force`）；
3. 用 `robocopy /E /COPY:DAT /R:1 /W:1` 复制整个实例目录；
4. 按新位置重写新目录里的 `my.ini`，并写下指针文件 `<项目>\.mysql-location.json`；
5. 从新位置启动 MySQL，等它就绪后打印状态。

验证没问题之后，源目录可以手动删掉（脚本只打印提示，绝不自动删）：

```powershell
Remove-Item -Recurse -Force 'D:\myProject\FlutterProject\viewer\.mysql'
```

想反悔：删掉 `<项目>\.mysql-location.json`（或者改里面的 `dataDir`），下次命令就会用回默认位置。

`comfyhub.ps1` 也接受 `-DataDir`，会原样透传给 `mysql.ps1` / `server.ps1`：

```powershell
pwsh -File scripts\comfyhub.ps1 up     -DataDir 'D:\mysql\comfyhub'
pwsh -File scripts\comfyhub.ps1 down   -DataDir 'D:\mysql\comfyhub'
pwsh -File scripts\comfyhub.ps1 doctor -DataDir 'D:\mysql\comfyhub'   # 会打印解析出来的实例目录
```

## 三、要备份什么 / 注意事项

* **备份**：只备份 `<实例目录>\data`（配置和日志都是可以重新生成的）。
  备份前**必须先停库**：`pwsh -File scripts\mysql.ps1 stop`。
* **复制前一定要停库**：直接复制正在运行的 MySQL 数据目录，拿到的很可能是一份无法启动的数据。
* 新位置别放在会被同步盘（OneDrive 等）或杀软频繁扫描的目录里，InnoDB 对文件锁比较敏感。
* 新位置要有足够空间（默认 `innodb_buffer_pool_size=256M`，实际占用会明显大于数据本身）。
* 别把数据库直接放到项目里再提交到 git：默认位置 `.mysql` 和指针文件 `.mysql-location.json` 都在 `.gitignore` 里。
* 端口仍是 `127.0.0.1:3307`，换存储位置不影响连接方式。
