# AgentMonitor v1.4.0 — OpenCode 接入与用量历史保存

新增 OpenCode 原生用量监测，并为所有来源保存已采集的历史。源日志被清理或应用重启后，已成功落盘的 Token 记录仍可查看。

## 下载

| 平台 | 安装包 |
|---|---|
| Windows x64 | [AgentMonitor_1.4.0_x64-setup.exe](https://github.com/YFan945/UsageMonitor/releases/download/v1.4.0/AgentMonitor_1.4.0_x64-setup.exe) |
| macOS Apple Silicon / Intel | [AgentMonitor_1.4.0_universal.dmg](https://github.com/YFan945/UsageMonitor/releases/download/v1.4.0/AgentMonitor_1.4.0_universal.dmg) |

安装后无需 Node.js 或 Rust。Windows 需要 WebView2；macOS 包未签名或公证，打开 DMG 后将应用拖入“应用程序”。

## 新增与修复

- **OpenCode**：设置和首次引导中可选择 OpenCode，默认读取 `~/.local/share/opencode`，遵循 `XDG_DATA_HOME`。只读支持新旧 SQLite 消息表（`message`、`session_message`）及旧版 JSON 会话存储，识别模型、子 Agent、缓存读取和写入，不叠加 `part` 或会话汇总；个别损坏消息不影响其他可读记录。
- **用量历史**：启动后自动采集，此后每 30 秒后台扫描；成功扫描的新增或更新记录立即写入配置目录的 `usage-history.json`。关闭窗口隐藏到后台后继续采集，完全退出后停止。
- **恢复与去重**：日志被删除或轮换后保留已保存的记录，重启自动恢复。累计快照先归档再求差，避免 Codex 重复累计；同一消息的流式用量更新合并到原记录。
- **可靠保存**：临时文件写入并同步磁盘后替换主文件，保留 `.bak` 备份；主文件损坏时尝试恢复备份，保存失败直接显示原因并在后续扫描重试。
- **筛选**：模型白名单仅筛选显示，历史完整采集。禁用来源或移除目录只隐藏对应历史，重新启用原目录可恢复显示。

## 升级与数据

沿用原配置目录和设置，无需手动迁移。历史保存从升级后的首次成功采集开始，无法补回此前已删除或未记录用量的调用。备份时请同时保存 `usage-history.json` 及 `.bak`，不要只保存 `config.json`。

Node 源码服务建议使用 Node.js 24，OpenCode SQLite 读取需要内置 `node:sqlite`；桌面版内置数据库支持。Node/SEA 维护版安装器独立管理版本，本次发行提供 Rust/Tauri 桌面安装包。

## 验证与范围

本地 Node 31 项、Rust 17 项测试通过，覆盖新旧 OpenCode、只读数据库、后台定时采集、删除日志与重启恢复、模型及来源筛选、备份恢复和保存失败重试；本机 OpenCode 数据也通过只读解析核验。Windows 和 macOS 双架构安装包由发布流水线构建。构建成功不等同于所有系统版本的安装、窗口和托盘实机验收。

CodeBuddy 仍为兼容日志适配；Doubao、Trae / Trae CN 仍需标准 `usage.record` JSONL 导入。Token 统计用于本地用量参考，不代表费用、套餐消耗或剩余额度。

详细说明见 [README](https://github.com/YFan945/UsageMonitor#readme)。AgentMonitor 与 KimiMonitor 均由 YFan945 开发，分别管理版本与发行，使用 MIT License。
