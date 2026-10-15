# AgentMonitor

AgentMonitor 是 YFan945 开发的多 Agent 本地用量仪表盘。读取本机会话日志，查看模型调用次数、Token 趋势、活动热力图，以及模型、项目和 Agent 排行。

AgentMonitor 与 [KimiMonitor](https://github.com/YFan945/kimi-usage-monitor) 均由同一作者开发。KimiMonitor 聚焦 Kimi Code 用量统计；AgentMonitor 延续其仪表盘设计和本地统计能力，面向多种 Agent，使用独立的应用名称、配置目录和版本号。

当前版本 **v1.4.0**，本项目独立版本从 **v1.0.0** 开始。桌面应用使用 Rust / Tauri，Windows 和 macOS 安装包均不需要另行安装 Node.js 或 Rust；开发者也可以运行共享界面的 Node 服务。

## 下载与安装

| 平台 | 下载 | 安装方式 |
|---|---|---|
| Windows x64 | [安装包](https://github.com/YFan945/UsageMonitor/releases/download/v1.4.0/AgentMonitor_1.4.0_x64-setup.exe) | 运行安装器；缺少 WebView2 时按提示安装 |
| macOS Apple Silicon / Intel | [双架构 DMG](https://github.com/YFan945/UsageMonitor/releases/download/v1.4.0/AgentMonitor_1.4.0_universal.dmg) | 打开 DMG，将应用拖入“应用程序” |

完整版本说明见 [v1.4.0 Release](https://github.com/YFan945/UsageMonitor/releases/tag/v1.4.0)。macOS 包未签名或公证，首次打开可能受到系统拦截。应用安装名称为 AgentMonitor，macOS 应用为 AgentMonitor.app。

## 开始使用

1. 启动应用，在首次引导中勾选需要监测的 Agent，确认日志目录并选择主题。
2. 打开“设置”，可增减监测目录、禁用来源、选择模型或添加完整模型 ID。
3. 选择时段，并使用 Agent、模型、项目多选筛选查看用量。多选为空表示全部。
4. 点击排行中的“查看全部”查看完整列表；点击排行横条可切换对应筛选。

“自动监测所有模型”包含未来出现的模型；关闭后仅统计已勾选的模型。移除目录或禁用来源不会删除原始日志。所有来源关闭后，重启不会自动恢复默认监测。

## 支持的 Agent

`~` 表示当前用户主目录。路径可在设置中修改，同一个 Agent 可以配置多个目录。

| Agent | 日志支持 | 默认目录 |
|---|---|---|
| Kimi Code | 原生 usage.record，包含子 Agent | ~/.kimi-code/sessions |
| Claude Code | 原生 message.usage，合并同一响应的流式记录 | ~/.claude/projects |
| OpenCode | 只读 opencode.db 的 message / session_message；兼容旧版 storage/session、storage/message JSON | ~/.local/share/opencode |
| Codex | 原生 token_count，累计快照按差分统计 | ~/.codex/sessions |
| ZCode | 优先只读 db.sqlite 的 model_usage；兼容 model_io JSONL | ~/.zcode/cli/db |
| WorkBuddy | message.usage 与 function_call，按响应去重 | ~/.workbuddy/projects |
| CodeBuddy | 兼容 Claude / WorkBuddy 风格 JSONL，需按实际版本核对 | ~/.codebuddy/projects |
| DeepSeek Harness | v2–v4 会话事件，支持拼接的 .jsonl.zstd 压缩日志 | ~/.dsh/sessions |
| Doubao、Trae / Trae CN | 标准 usage.record JSONL 导入，尚不支持原生私有日志或账户用量接口 | 手动指定导入目录 |

DSH 设置了 DSH_HOME 时，默认读取其下的 sessions。普通恢复标记保留历史用量；明确标记为继承的父会话记录不重复计入子会话，重试分别统计。DSH v0/v1 历史格式没有完整兼容保证。

ZCode 建议使用数据库入口，不要同时启用同源数据库和 rollout 目录。数据库只统计 model_usage，不叠加其他表中的 Token。

## 用量历史保存

启动后自动采集，此后每 30 秒扫描一次；页面刷新也会触发扫描。每次扫描后的新增或更新记录立即保存到配置目录下的 `usage-history.json`，先写临时文件并同步磁盘，再替换主文件，同时保留 `.bak` 备份。关窗隐藏到后台时仍会采集，完全退出后停止采集。

已成功保存的用量在源日志、会话文件或数据库记录被清理后仍会保留，重启后自动恢复。事件按来源和标识合并，重复扫描不累加；Codex 保存累计快照，合并后再计算增量。模型白名单仅筛选显示，不影响历史采集。禁用来源或移除目录后隐藏对应历史，重新启用原目录可恢复显示。

备份 `usage-history.json` 和 `.bak` 可保留已采集的用量；不要只备份原始日志或配置文件。若保存失败，页面会显示原因并在后续扫描重试；只有内存中的未落盘记录无法保证在退出后恢复。采集前已删除的日志无法恢复。

OpenCode 默认目录遵循 `XDG_DATA_HOME/opencode`，未设置时为 `~/.local/share/opencode`。设置中选择包含 `opencode.db` 的目录或旧版 `storage` 的上级目录；旧 JSON 也可直接选择 `storage`。只统计助手消息的用量，不叠加会话汇总或 `part` 表；输入字段已排除缓存，直接保留缓存读取和写入值。SQLite 读取需要 Node.js 内置 SQLite（建议 Node.js 24），桌面版内置支持。数据格式依据 [OpenCode 会话存储源码](https://github.com/anomalyco/opencode/blob/dev/packages/core/src/session/sql.ts)。

## 统计与筛选

- **模型调用次数**：可识别的模型用量记录数，包含子 Agent。流式响应按来源的事件或响应标识合并，因此不等同于用户发送消息的次数。
- **Token 合计**：未缓存输入、输出、缓存读取和缓存创建之和。
- **趋势图**：按类型、模型或 Agent 着色；“图表计入缓存读取”影响趋势及排行，不改变总量卡片和会话明细。
- **活动热力图**：展示最近约一年的已启用来源数据，不受页面时段和多选筛选影响。
- **模型、项目排行**：展示当前时段的数据，跟随 Agent 筛选；不受模型和项目多选影响。Agent 排行保留当前时段全部来源，便于比较。
- **完整排行**：默认前 8 项，详情保留所有条目。占比图将低于 2% 或超出前 7 项的用量合并为“其他”。
- **会话明细**：首页显示调用最多的前 50 个会话，完整列表按 Token 合计排序，遵循页面筛选且始终包含缓存读取；没有饼图。

模型按厂商固定配色，参考 [Artificial Analysis 厂商图例](https://artificialanalysis.ai/evaluations/artificial-analysis-intelligence-index)。柱状图、模型排行和饼图使用相同颜色；OpenAI 在浅色主题为黑色、深色主题为白色，相邻同色柱块有对比色分隔线。未识别模型使用灰色，参考页面未列出的厂商使用补充色。

统计只反映本地日志，不代表账单、套餐消耗或剩余额度。未落盘或缺少用量字段的调用无法补算。读取失败会显示来源和具体文件，其他可读文件继续统计。

## 会话标题

Codex 从日志目录附近最新的 state_<版本>.sqlite 读取 threads.title；WorkBuddy 从 workbuddy.db 读取自定义标题或会话标题。Claude 和 DSH 可从日志标题事件读取名称。

没有正式标题时，使用明确标注的“首条提问：”或“子 Agent：”作为显示名称；仍无信息的会话显示“无标题”。元数据库只补充已扫描会话的名称，不导入其他会话或叠加用量。

## 窗口与设置保存

关闭桌面窗口的行为可在设置“关闭应用时”中选择：直接关闭应用、后台运行（默认，隐藏到托盘）或每次询问。通过托盘恢复窗口，托盘菜单“退出”始终彻底退出应用；桌面版页面不设退出键。Node 源码版通过页面“退出”停止服务。服务仅监听 127.0.0.1，默认端口为 43110；桌面版端口被占用时自动选择空闲端口。

监测目录、模型设置和引导状态保存在 config.json，保存时保留上一份有效备份。主题及页面筛选保存在当前浏览器 / WebView 的 localStorage，不跨设备同步。

| 运行方式 | 配置目录 |
|---|---|
| Windows 桌面版 | %APPDATA%/AgentMonitor |
| macOS 桌面版 | ~/Library/Application Support/AgentMonitor |
| Node 源码版（Windows） | 项目目录 |
| Node 源码版（macOS） | ~/Library/Application Support/AgentMonitor |

Node 可通过 AGENT_MONITOR_CONFIG_DIR 指定独立配置目录，兼容旧的 KIMI_MONITOR_CONFIG_DIR。桌面版与 Node 版配置及浏览器设置不自动同步。升级前先退出旧应用；卸载可能保留配置。

首次使用新目录时，桌面版会从同级旧目录 com.yfan945.agentmonitor（旧应用标识目录）或 com.yfan945.kimimonitor 复制有效配置，macOS Node 默认目录会从旧 KimiMonitor 目录复制。主配置损坏时尝试旧备份；新目录已有配置或备份时不覆盖，旧文件保留。应用标识改变后 WebView 使用独立数据目录，主题和页面筛选可能需要重新选择。各 Agent 原始日志目录不受应用改名影响。

## 标准用量导入

将真实调用用量逐行写入 .jsonl 文件，在设置中选择来源并配置文件所在目录。每次调用一行，time 为 Unix 毫秒或 ISO 日期，eventId 在同一会话中唯一；inputOther 不包含缓存用量。

以下仅为格式示例，不是实际数据：

```json
{"type":"usage.record","eventId":"call-001","sessionId":"session-001","time":"2026-10-03T00:00:00+08:00","model":"model-id","cwd":"D:/project","usage":{"inputOther":100,"output":30,"inputCacheRead":200,"inputCacheCreation":0}}
```

## 源码运行与构建

### Node 服务

建议 Node.js 24。基础 JSONL 支持 Node.js 18+；数据库读取需要 node:sqlite，DSH 压缩日志需要 Zstandard API。旧 Node 会回退到 ZCode rollout 目录，无法解析的压缩日志会报告原因。

```powershell
node server.js
# 已有服务占用端口时
node server.js --port 43111
```

Node 服务无需安装 npm 依赖。访问终端显示的本机地址；停止服务可使用页面“退出”。

### 桌面应用

需要 Rust 和平台构建工具，Windows 还需 MSVC。先安装 npm lockfile 中的 Tauri CLI：

```powershell
npm ci
npm run desktop:dev
# Windows 安装器
npm run desktop -- --bundles nsis
```

Windows 成品位于 src-tauri/target/release/bundle/nsis/。macOS 在 Mac 上构建：

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
npm ci
npm run desktop -- --target universal-apple-darwin --bundles app,dmg
```

DMG 位于 src-tauri/target/universal-apple-darwin/release/bundle/dmg/。图标源文件为 assets/app-icon.png。

### 测试与发布

```powershell
npm test
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

测试覆盖多来源解析、流式合并、累计差分、重复入口、标题恢复、配置保存及异常请求。自动化测试和构建成功不等同于所有系统版本上的安装、窗口与托盘实机验收。

GitHub Actions 的 Regression tests 校验 Node 18/24 和 Rust。Build desktop apps 支持手动构建；推送版本标签时构建 Windows 与 macOS 双架构安装包并生成 Release 草稿，核对后再正式发布。版本号须同步 package.json、package-lock.json、Cargo.toml、Cargo.lock 和 tauri.conf.json。

build.js、sea-config.json 和 packaging/ 为保留的 Node / SEA 打包工具，与当前 Tauri 安装包分开。AgentMonitor 与 KimiMonitor 分别管理版本和下载入口；辅助发布命令仅在明确授权时运行。

## 常见问题

**为什么显示零条记录？** 检查来源、日志目录、模型白名单和时段。目录存在不代表含有可识别用量；Doubao 和 Trae 需要标准日志导入。

**排行为何与卡片总量不同？** 模型及项目排行跟随 Agent 筛选，但不受模型、项目多选影响；图表缓存开关也可能改变排行总量。

**为什么部分会话没有标题？** 原日志或元数据库可能已缺少标题信息。应用不会据此编造正式标题。

**为什么关闭窗口后仍在运行？** 关闭窗口会隐藏到后台，请使用托盘或页面“退出”。

**为什么浏览器与桌面版数据不同？** 检查各自的监测配置、模型设置、时段和筛选条件；它们使用独立配置和 localStorage。

## 作者与协议

作者：[YFan945](https://github.com/YFan945)。相关项目：[KimiMonitor](https://github.com/YFan945/kimi-usage-monitor)。本项目采用 [MIT License](LICENSE)。
