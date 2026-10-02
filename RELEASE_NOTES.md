# AgentMonitor v1.0.0 — 多 Agent 用量监控

本项目的首个独立版本。通过本机会话日志查看多种 Agent 的模型调用次数与 Token 用量，提供 Windows 安装器和 macOS 双架构 DMG，安装后无需 Node.js 或 Rust。

## 下载

| 平台 | 安装包 |
|---|---|
| Windows x64 | [AgentMonitor_1.0.0_x64-setup.exe](https://github.com/YFan945/UsageMonitor/releases/download/v1.0.0/AgentMonitor_1.0.0_x64-setup.exe) |
| macOS Apple Silicon / Intel | [AgentMonitor_1.0.0_universal.dmg](https://github.com/YFan945/UsageMonitor/releases/download/v1.0.0/AgentMonitor_1.0.0_universal.dmg) |

Windows 需要 WebView2；macOS 包未签名或公证。macOS 安装时打开 DMG，将 AgentMonitor 拖入“应用程序”。

## 主要功能

- 本地监测 Kimi Code、Claude Code、Codex、ZCode、WorkBuddy 和 DeepSeek Harness；设置中管理来源、多个目录、模型白名单及主题。
- 按时段、Agent、模型和项目筛选用量，查看趋势柱状图及全年活动热力图。
- 模型、项目和 Agent 完整排行提供 Token 占比图；模型和项目排行跟随 Agent 选择，会话明细保留完整列表。
- 从日志及只读元数据库恢复会话标题，缺少正式标题时明确标注首条提问或子 Agent 名称。
- 模型按厂商固定配色，柱状图、排行和饼图颜色一致；深色主题将 OpenAI 的黑色替换为白色，相邻同色柱块保留分隔线。
- 支持 DSH 拼接压缩帧与恢复事件，处理流式用量、累计快照、重叠目录与继承会话，减少漏计和重复计数。
- 原生窗口和托盘支持后台监测；关闭窗口隐藏应用，托盘或页面“退出”停止应用。

## 接入范围与验证

CodeBuddy 提供兼容日志适配，仍需核对具体版本。Doubao、Trae / Trae CN 目前需要导入标准 usage.record JSONL，尚不支持原生私有日志或账户用量接口。

应用只读取本地日志。Token 合计用于用量参考，不代表费用、套餐消耗或剩余额度；没有落盘的调用无法补算。

本版本通过 Node 18/24 回归检查、Rust 单元测试及 Windows、macOS 双架构构建。构建成功不等同于所有系统版本的安装、窗口和托盘实机验收。

详细设置、目录与统计口径见 [README](https://github.com/YFan945/UsageMonitor#readme)。项目基于 KimiMonitor 扩展，使用 MIT License。
