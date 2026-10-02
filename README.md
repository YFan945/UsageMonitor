# Agent 用量监控（基于 KimiMonitor）

基于 [YFan945/kimi-usage-monitor](https://github.com/YFan945/kimi-usage-monitor) 扩展，沿用原有仪表盘、趋势柱状图和活动热力图。增加 Agent 多选、按 Agent 着色、首次来源引导，以及主题 / 监测来源 / 模型设置。Node 与 Rust/Tauri 共用界面，并分别实现相同的日志适配器。下方原项目 Release 下载链接是历史发行包，**不包含本次扩展**；本次改动请从源码运行或自行构建。

## 多 Agent 监测

### 桌面安装包构建

应用图标源文件为 `assets/app-icon.png`，Windows ICO、macOS ICNS、托盘及网页图标均由该图片生成。桌面应用沿用 `KimiMonitor` 的安装名称和配置路径以兼容原项目。

Windows 本机构建：先运行 `npm ci`，再运行 `npm run desktop -- --bundles nsis`，安装器位于 `src-tauri/target/release/bundle/nsis/`。

macOS 构建需要 Mac：运行 `npm ci`，安装 Rust 的 `aarch64-apple-darwin` 与 `x86_64-apple-darwin` 目标，再运行 `npm run desktop -- --target universal-apple-darwin --bundles app,dmg`。DMG 位于 `src-tauri/target/universal-apple-darwin/release/bundle/dmg/`，支持 Apple Silicon 和 Intel。

新仓库 [YFan945/UsageMonitor](https://github.com/YFan945/UsageMonitor) 的 Actions → Build desktop apps 支持手动构建两种安装包，产物在成功任务的 Artifacts 中下载。手动构建不创建 Release。安装包未配置代码签名和 macOS 公证；打包成功不等于已在所有目标系统完成安装、托盘和运行验收。

```powershell
node server.js --port 43111
```

访问 `http://127.0.0.1:43111`。默认端口仍为 `43110`；已有监控占用时可指定其他端口。Node 服务无需安装 npm 依赖，基础功能支持 Node.js 18+，**DSH 压缩日志需要 Node.js 22.15+ 的 Zstandard API，建议 Node.js 24**。Rust 桌面版自带 Zstandard 解码。

| Agent | 接入方式 | 默认日志目录 |
|---|---|---|
| Kimi Code | 原生 `usage.record`，包含子 agent | `~/.kimi-code/sessions` |
| Claude Code | 原生 `message.usage`，按响应 ID 去重流式记录 | `~/.claude/projects` |
| Codex | 原生 `token_count`，累计值差分，跳过重复限额更新 | `~/.codex/sessions` |
| ZCode | 优先只读 CLI `db.sqlite` 的 `model_usage` 与 `session` 表；兼容 `model_io` JSONL | `~/.zcode/cli/db`；旧 Node 使用 `~/.zcode/cli/rollout` |
| WorkBuddy | `message.usage`，支持 `function_call`，按模型响应 ID 去重 | `~/.workbuddy/projects` |
| CodeBuddy | 兼容 Claude / WorkBuddy 风格 JSONL；未做独立实机验收 | `~/.codebuddy/projects`（按实际版本调整） |
| DeepSeek Harness | v2–v4 的 `assistant/message` / `assistant/attempt`；支持 `.jsonl.zstd`，只读最高格式代 | `~/.dsh/sessions` |
| Doubao、Trae | 标准用量 JSONL 接入；**尚未实现原生日志 / 云端账户接口** | 在设置中指定标准日志目录 |

首次引导会列出所有来源，勾选已发现原生目录的来源；可调整路径、勾选状态及主题，也可以暂不启用任何来源。路径存在但未记录用量时显示零条记录。设置可以给同一个 Agent 添加多个目录、禁用来源、添加完整模型 ID；“自动监测所有模型”会包含后来出现的新模型，否则只统计白名单中的模型。

ZCode 数据库读取需要 Node.js 22.13+ 的 `node:sqlite`，Rust 桌面版自带 SQLite。推荐数据库入口以避免读取包含完整请求正文的大型 `model_io` 日志；同一来源不要同时启用数据库和 rollout 入口，它们可能是同源数据。只读取 `model_usage`，不叠加 `turn_usage` 或 `message` 表的 token，防止重复累计。某个来源数据库读取失败时，其他来源仍可统计，设置页显示原因。

柱状图旁的 Agent / 模型 / 项目多选共同影响统计卡片、趋势与会话明细，选择为空表示全部。按 Agent 着色的柱状图可比较不同来源；会话明细显示来源标签。活动热力图仍显示所有已启用来源及模型设置下的全年数据，不受趋势筛选影响。

模型、项目和 Agent 排行默认显示当前时段用量最高的前 8 项，点击“查看全部”可打开完整排行；横条仍支持加入 / 移出筛选。项目 / 会话明细默认显示调用最多的前 50 个会话，也可查看全部并按 Token 合计排序。会话详情仅显示完整明细表；三个排行榜的详情表旁提供 Token 占比饼图，低于 2% 或超出前 7 项的用量合并为“其他”，完整表格不截断。窄屏下饼图与表格上下排列。排行榜不受页面多选筛选影响，遵循“图表计入缓存读取”开关；会话详情遵循页面多选筛选，合计始终包含缓存读取，详情内注明统计口径。

标准用量接入：把真实调用用量逐行写入 `.jsonl` 文件，选择 Doubao 或 Trae 等来源并配置文件所在目录。每次调用一行，`time` 为 Unix 毫秒或 ISO 日期；`inputOther` 不包含缓存读取 / 创建，`eventId` 在同一会话中唯一。下面仅为格式示例，不是实际用量：

```json
{"type":"usage.record","eventId":"call-001","sessionId":"session-001","time":1790935200000,"model":"model-id","cwd":"D:/project","usage":{"inputOther":100,"output":30,"inputCacheRead":200,"inputCacheCreation":0}}
```

监测目录、模型白名单及首次引导状态写入 `config.json`；旧版 `roots` 配置会显示为 Kimi 来源，保存新设置后迁移到 `monitors`。主题和图表筛选保存在当前浏览器 / WebView 的 localStorage，不跨设备同步。移除或禁用来源不会删除原日志，关闭所有来源后重启也不会自动恢复默认目录。Node 可用 `KIMI_MONITOR_CONFIG_DIR` 指定独立配置目录。

统计只反映本地记录，不能代替服务商账单、套餐用量、剩余额度。未落盘的调用无法补算；CodeBuddy 的版本差异、DSH v0/v1 历史格式及损坏的压缩文件没有完整兼容保证。DSH 官方用量字段按互不重叠的输入 / 缓存口径统计，继承的 seed 历史不重复计入新会话；重试另计。

验证命令：`npm test` 与 `cargo test --manifest-path src-tauri/Cargo.toml --locked`。新增合成数据测试覆盖多来源去重、模型白名单、禁用来源、配置持久化、ZCode / WorkBuddy 原生事件与 DSH 压缩、历史格式代和重试。源码浏览器验收不等同于安装包、托盘或 macOS 实机验收。

## 原项目发行说明

项目提供两条发行线，共享同一套界面：

| 发行线 | 当前版本 | 运行方式 |
|---|---|---|
| **Rust/Tauri 桌面版** | **v2.0.0，Latest** | 原生窗口和托盘，Windows 安装包、macOS 双架构 DMG |
| **Node/SEA 维护版** | **v1.2.1** | Windows 安装版、单文件版、绿色版，以及 macOS Node 应用包 |

Node/SEA v1.2.1 已包含统计溢出、重复扫描、JSONL 漏读、缺少会话元数据、异常请求和相对时间刷新等修复。历史版本继续保留在 [Releases](https://github.com/YFan945/kimi-usage-monitor/releases)，维护版不会取代 Rust 版的 Latest 标记。

## 下载与选择版本

### Rust/Tauri 桌面版

从 [v2.0.0 Release](https://github.com/YFan945/kimi-usage-monitor/releases/tag/v2.0.0) 下载，无需安装 Node.js 或 Rust：

| 文件 | 平台 | 安装方式 |
|---|---|---|
| [KimiMonitor_2.0.0_x64-setup.exe](https://github.com/YFan945/kimi-usage-monitor/releases/download/v2.0.0/KimiMonitor_2.0.0_x64-setup.exe) | Windows x64 | 运行安装器；需要 WebView2，缺少时按安装器提示处理 |
| [KimiMonitor_2.0.0_universal.dmg](https://github.com/YFan945/kimi-usage-monitor/releases/download/v2.0.0/KimiMonitor_2.0.0_universal.dmg) | macOS，Apple Silicon / Intel | 打开 DMG，将 KimiMonitor 拖入“应用程序” |

macOS 包未签名或公证，首次打开可能受到系统拦截。请确认文件来自本仓库的 Release，再按系统提供的方式允许打开。

### Node/SEA 维护版

从 [v1.2.1 Release](https://github.com/YFan945/kimi-usage-monitor/releases/tag/v1.2.1) 下载：

| 文件 | 平台 | 运行方式 |
|---|---|---|
| [KimiMonitor-Setup-1.2.1.exe](https://github.com/YFan945/kimi-usage-monitor/releases/download/v1.2.1/KimiMonitor-Setup-1.2.1.exe) | Windows x64 | 当前用户安装，默认位置为 `%LOCALAPPDATA%\Programs\KimiMonitor` |
| [KimiMonitor-Windows-x64.exe](https://github.com/YFan945/kimi-usage-monitor/releases/download/v1.2.1/KimiMonitor-Windows-x64.exe) | Windows x64 | 直接运行；首次启动会尝试创建桌面快捷方式 |
| [KimiMonitor-Windows-Portable.zip](https://github.com/YFan945/kimi-usage-monitor/releases/download/v1.2.1/KimiMonitor-Windows-Portable.zip) | Windows x64 | 解压后运行 `启动监控.bat`；包内已含 `runtime/node.exe` |
| [Kimi-Monitor-macOS.tar.gz](https://github.com/YFan945/kimi-usage-monitor/releases/download/v1.2.1/Kimi-Monitor-macOS.tar.gz) | macOS 11+，Apple Silicon / Intel | 解压后将 KimiMonitor.app 放入“应用程序”；包内含两种架构的 Node runtime |
| [SHA256SUMS.txt](https://github.com/YFan945/kimi-usage-monitor/releases/download/v1.2.1/SHA256SUMS.txt) | 所有 Node 发行包 | 用于核对四个安装包或压缩包的 SHA-256 |

这些成品包均自带运行所需的 Node，无需另行安装。Windows 版优先以 Edge 应用窗口打开界面，没有 Edge 时使用默认浏览器。macOS Node 版打开浏览器界面，没有原生托盘。

Windows 可用 PowerShell 核对下载文件：

```powershell
Get-FileHash .\KimiMonitor-Setup-1.2.1.exe -Algorithm SHA256
```

将结果与同一 Release 的 `SHA256SUMS.txt` 对比。

## 开始使用

1. 源码扩展版的首次引导会探测上表中的原生来源目录。原版发行包仅探测 `~/.kimi-code/sessions`。
2. 勾选来源并确认路径。目录存在但没有可识别日志时，统计会显示为零。
3. 扩展版进入界面后使用“设置”调整来源、目录、模型与主题，再选择时段、Agent、模型或项目；原版包的目录入口为“数据目录”。

支持以下目录布局；添加的是 `sessions` 或工作区目录，不是单个 `wire.jsonl` 文件：

```text
sessions/<工作区>/<会话>/agents/<agent>/wire.jsonl
工作区目录/<会话>/agents/<agent>/wire.jsonl
```

Kimi 重叠路径按会话真实路径去重，同一会话只统计一次；其他来源按会话与用量事件标识去重。扩展版保存设置时拒绝已启用但不存在或不可读取的目录，并保留旧配置；旧版 `roots` 接口仍剔除不存在的路径。移除扫描路径只修改配置，不删除会话文件。

## 窗口、后台与退出

| 版本 | 关闭窗口后 | 彻底退出 |
|---|---|---|
| Rust/Tauri | 窗口隐藏，监控继续运行；托盘可恢复窗口 | 托盘菜单“退出”，或页面“退出” |
| Windows Node/SEA、绿色版 | 服务继续运行；PowerShell 托盘可重新打开界面 | 托盘“退出”、页面“退出”，或同目录下的 `停止监控.bat`（如有） |
| macOS Node | 浏览器页面关闭，服务仍在后台 | 页面“退出”；无法打开页面时，在活动监视器中结束对应 Node 进程 |

Windows Node 版退出时会尝试关闭标题匹配的应用窗口，普通浏览器页面可能需要手动关闭。安装 PWA 或把页面安装为浏览器应用只增加一个界面入口，本地服务仍需运行。

## 配置、升级与卸载

扫描目录及首次引导状态保存在 `config.json`；主题、时段、筛选等界面设置保存在浏览器或 WebView 的 localStorage。

| 运行方式 | `config.json` 位置 |
|---|---|
| Windows Rust/Tauri | `%APPDATA%\com.yfan945.kimimonitor\config.json` |
| macOS Rust/Tauri | `~/Library/Application Support/com.yfan945.kimimonitor/config.json` |
| Windows Node/SEA 安装版 | `%LOCALAPPDATA%\Programs\KimiMonitor\config.json`（默认安装路径） |
| Windows Node 单文件版 | exe 所在目录 |
| Windows Node 源码版、绿色版 | `server.js` 所在目录 |
| macOS Node，包括源码运行 | `~/Library/Application Support/KimiMonitor/config.json` |

- **同一发行线升级**：先退出旧程序。Tauri 沿用应用标识与配置目录；Node 安装版沿用安装位置，便携版替换文件时保留 `config.json`。
- **Node 与 Tauri 互相迁移**：先退出原服务，再启动目标版本。两者配置位置不同，请在引导中重新选择数据目录；界面设置也不会自动迁移。
- **换电脑或移动会话目录**：配置中的路径需要在当前电脑上有效，请重新核对。
- **Windows 卸载**：安装版通过系统“应用”列表卸载。Node 安装器会询问是否删除配置；Tauri 不应视为一定自动清理配置，需要彻底重置时，在退出后手动删除对应配置目录。
- **macOS 卸载**：先退出，再删除“应用程序”中的 KimiMonitor.app；需要重置设置时删除上表对应目录。Node 包附带的 `卸载.command` 会删除应用及 Node 配置，请先退出服务再运行。
- **便携版卸载**：先退出，再删除解压目录或 exe 及其旁边的配置文件。自动创建的桌面快捷方式可手动移除。

以上操作针对 KimiMonitor 程序与配置，不需要删除 `~/.kimi-code` 中的原始会话。

## 功能与统计口径

- **活动总览**：按日、按周或累计显示最近约一年的活动，不受下方时段及模型/项目筛选影响。“累计”从当前可见范围的第一周起算。
- **时段**：支持今天、最近 24 小时、最近 7 天、最近 30 天、全部与自定义范围。日历预设按本机时间计算，7 天和 30 天包含今天；相对范围在刷新或重新渲染时更新，自定义范围固定。
- **趋势与筛选**：趋势图支持小时/天粒度，按 token 类型或模型着色；模型与项目均可多选，同一组内任选匹配，两组条件同时满足。
- **模型/项目排行**：展示所选时段内用量最高的前 8 项，不随模型/项目筛选缩小，用于选择和切换筛选条件。
- **缓存读取开关**：影响趋势图与排行的用量展示；总量卡片、活动总览和会话明细始终包含全部 token 类型。
- **会话明细**：按调用次数排序，最多展示前 50 个有调用记录的会话。
- **自动刷新**：界面每 10 秒请求一次数据。扫描复用未变化文件的缓存，日志变化时重读该文件；关闭自动刷新不会退出后台服务。
- **主题**：自动跟随系统，或手动选择浅色/深色。

每条可解析的 `usage.record` 事件计为一次调用，包含主 agent 和子 agent。token 合计为 `inputOther + output + inputCacheRead + inputCacheCreation`；异常或缺失的 token 字段按零处理，损坏的 JSON 行跳过。会话项目优先取 `state.json` 的 `cwd`，缺失时使用可找到的 `session_index.jsonl` 中的 `workDir`；没有 `state.json` 但有 `agents/` 的会话也会扫描。

这是日志中记录的用量，不是 Kimi 账户账单、余额、剩余额度或实时价格。缓存读取量可能较大，不能用 token 总量直接推算费用。监控代码只读取原会话文件，不修改或删除它们。

## 源码开发

### Node 服务

需要 Node.js ≥ 18；服务只使用 Node 内置模块，无需 `npm install`：

```bash
node server.js
node server.js --port 8080
```

默认访问 `http://127.0.0.1:43110`，指定端口时使用相应地址。端口也可由 `KIMI_MONITOR_PORT` 设置。Node 端口被占用时，新进程会退出，不会核验占用端口的程序是否为 KimiMonitor；遇到其他应用页面或无法启动时，请检查端口或换一个端口。

### Rust/Tauri 桌面版

需要 Rust 和相应平台构建工具；Windows 使用 MSVC，macOS 使用 Xcode Command Line Tools。安装 npm 开发依赖后运行：

```bash
npm ci
npm run desktop:dev
npm run desktop
```

Windows NSIS 安装包位于 `src-tauri/target/release/bundle/nsis/`。macOS 双架构包需在 macOS 上构建：

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
npx tauri build --target universal-apple-darwin
```

DMG 位于 `src-tauri/target/universal-apple-darwin/release/bundle/dmg/`。Tauri 内嵌服务仅绑定 `127.0.0.1`，优先使用 `43110`，被占用时回退到空闲端口；窗口自动使用实际端口，不依赖 Edge。

## Node/SEA 打包

`build.js` 在 Windows 上打包 Node 维护版，与 `npm run desktop` 的 Rust 构建分开：

```bash
node build.js
node build.js --mac
```

也可运行 `build.bat` 并传入同样的参数。输出位于 `dist/`：单文件 exe、绿色版 ZIP、安装器（可用时），以及 `--mac` 生成的 macOS TAR 包。

- Node.js ≥ 20.12：用于生成 SEA，并通过 `npx postject` 注入当前 Node 可执行文件；脚本自动检测 sentinel fuse，设置 GUI 子系统以隐藏控制台。
- Inno Setup：用于生成 Windows 安装器；未检测到时跳过此产物。
- Python：绿色版 ZIP 优先使用 Python，缺少时尝试 PowerShell；macOS TAR 打包需要 Python，Pillow 用于生成图标，缺少 Pillow 时仍可打包但没有应用图标。
- `--mac`：下载脚本指定版本的官方 arm64/x64 Node runtime，再组装 `.app`；这不是 Tauri DMG 构建，也不代表已在 macOS 实机运行验证。

完整流程包含 Windows SEA 启动、`/api/data` 响应与退出冒烟测试。成品绿色版已包含 runtime；仅自行复制源码到没有 Node 的 Windows 电脑时，才需要准备 `runtime/node.exe`。启动脚本优先使用 PATH 中的 Node，再使用包内 runtime。

## 测试与发布

```bash
npm test
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

Node 内置 `node:test` 覆盖服务与共享前端函数；Rust 单元测试覆盖桌面数据引擎。测试使用临时合成会话，覆盖索引关联、重复目录、缺少元数据、JSONL 空格/损坏行、大数 token、相对时段与异常配置。Node 另覆盖非法 URL 和超过 1 MiB 的配置请求；无效请求保留原配置。

Windows SEA 可按 [AGENTS.md](AGENTS.md) 的方法直接运行二进制回归测试。自动化测试不覆盖安装器交互、系统托盘操作或各系统上的实机兼容性，这些行为仍需手动验收。

| 发布线 | 版本来源 | 发布流程 |
|---|---|---|
| Rust/Tauri | `package.json` / npm lockfile、`src-tauri/Cargo.toml` / Cargo lockfile、`src-tauri/tauri.conf.json` | 回归 CI 通过后推送非 `v1.*` 的版本标签；两平台构建成功后生成 Release 草稿，核对资产再正式发布 |
| Node/SEA | `packaging/installer.iss` 中的 `MyAppVersion` | 独立构建并验证 Node 产物，发布 `v1.x` 补丁；保留历史资产，不设为 Latest |

`.github/workflows/test.yml` 在 PR 和 main 推送时运行回归测试。手动运行 “Build desktop apps” 会生成 Actions artifacts；只有标签触发的构建才会生成 Release 草稿。

`node build.js --release` 是 Node 发布辅助命令，需要已登录的 `gh`。它会直接新建 Release 或覆盖已有 Release 的同名资产，新建时使用 `--latest=false`，不提供上述人工核验草稿的完整流程。请只在明确准备发布时使用；更新维护版应创建新的补丁版本，不覆盖历史版本。

## 常见问题

**显示零条记录**：确认扫描的是会话集合或工作区目录，下面存在 `agents/<agent>/wire.jsonl`，日志包含 `usage.record`。目录存在不代表有可统计的用量。

**关闭窗口后仍在运行**：使用页面或托盘“退出”；Node 绿色版还可运行 `停止监控.bat`。关闭浏览器、PWA 或 Tauri 窗口都不等于停止服务。

**模型/项目排行与筛选后的合计不同**：排行用于展示当前时段的候选项，筛选只缩小合计、趋势和会话明细；活动总览独立显示最近约一年的活动。

**浏览器或 WebView 数据不一致**：不同入口有各自的 localStorage；Node 和 Tauri 也使用不同配置目录。先确认当前版本、扫描路径、时段及筛选条件。

## 开源协议

[MIT License](LICENSE)。
