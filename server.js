#!/usr/bin/env node
'use strict';
/*
 * Kimi Code 用量监控 — 本地服务（便携版）
 * 扫描一个或多个数据目录下所有会话各 agent 目录的 wire.jsonl，
 * 解析其中的 usage.record 事件（每次 LLM API 调用一条），提供 JSON API + 静态页面。
 * 零依赖，Node >= 18 即可运行：node server.js [--port 43110]
 * 扫描路径保存在 config.json；Windows 源码/SEA 配置随程序目录保存，macOS 使用 Application Support。
 * 更换电脑或移动会话目录后需重新核对扫描路径。
 */
const http = require('http');
const fs = require('fs');
const os = require('os');
const path = require('path');
let DatabaseSync = null;
try { DatabaseSync = require('node:sqlite').DatabaseSync; } catch { }

// SEA 单文件模式检测（AgentMonitor.exe）：资源内嵌、数据/配置写在 exe 旁边
let SEA = null;
try { SEA = require('node:sea'); } catch { }
const isSEA = !!(SEA && typeof SEA.isSea === 'function' && SEA.isSea());
const ASSETS = isSEA ? {
  'index.html': Buffer.from(SEA.getAsset('index.html')),
  'icon.png': Buffer.from(SEA.getAsset('icon.png')),
  'favicon.ico': Buffer.from(SEA.getAsset('favicon.ico')),
  'manifest.webmanifest': Buffer.from(SEA.getAsset('manifest.webmanifest')),
  'icon-192.png': Buffer.from(SEA.getAsset('icon-192.png')),
  'icon-512.png': Buffer.from(SEA.getAsset('icon-512.png')),
} : null;

const HOME = process.env.USERPROFILE || process.env.HOME;
// 配置目录：Windows SEA → exe 旁；macOS Node（含源码运行）→ Application Support；其他源码运行 → server.js 旁
let CONFIG_DIR;
if (isSEA) CONFIG_DIR = path.dirname(process.execPath);
else if (process.platform === 'darwin') CONFIG_DIR = path.join(os.homedir(), 'Library', 'Application Support', 'AgentMonitor');
else CONFIG_DIR = __dirname;
const configOverride = process.env.AGENT_MONITOR_CONFIG_DIR || process.env.KIMI_MONITOR_CONFIG_DIR;
if (configOverride) CONFIG_DIR = path.resolve(configOverride);
try { fs.mkdirSync(CONFIG_DIR, { recursive: true }); } catch { }
function migrateLegacyConfig(target, legacy) {
  if (fs.existsSync(path.join(target,'config.json')) || fs.existsSync(path.join(target,'config.json.bak'))) return;
  for (const name of ['config.json','config.json.bak']) {
    try {
      const content = fs.readFileSync(path.join(legacy,name),'utf8');
      const parsed = JSON.parse(content);
      if (!parsed || Array.isArray(parsed) || typeof parsed !== 'object') continue;
      fs.writeFileSync(path.join(target,'config.json'),content,{flag:'wx'});
      return;
    } catch (error) { if (error.code === 'EEXIST') return; }
  }
}
// 仅迁移默认 macOS 配置；显式指定目录时保持隔离，旧文件保留。
if (!isSEA && process.platform === 'darwin' && !configOverride) {
  migrateLegacyConfig(CONFIG_DIR,path.join(os.homedir(),'Library','Application Support','KimiMonitor'));
}
const PUBLIC_DIR = path.join(__dirname, 'public');
const CONFIG_FILE = path.join(CONFIG_DIR, 'config.json');

const argvPortIdx = process.argv.indexOf('--port');
const PORT = Number(
  (argvPortIdx > -1 && process.argv[argvPortIdx + 1]) ||
  process.env.KIMI_MONITOR_PORT ||
  43110
);

// ---------- 配置 ----------
const CFG = { roots: [], setupDone: false, shortcut: false, monitors: null, models: [] };
const DEFAULT_ROOT = path.join(HOME, '.kimi-code', 'sessions');
// tool 是产品来源，a 仍表示原日志中的子 agent，二者不能混用。
const AGENTS = [
  ['kimi', 'Kimi Code', '.kimi-code/sessions', 'native'],
  ['claude', 'Claude Code', '.claude/projects', 'native'],
  ['opencode', 'OpenCode', '.local/share/opencode', 'native'],
  ['codex', 'Codex', '.codex/sessions', 'native'],
  ['workbuddy', 'WorkBuddy', '.workbuddy/projects', 'native'],
  ['codebuddy', 'CodeBuddy', '.codebuddy/projects', 'compatible'],
  ['zcode', 'ZCode', DatabaseSync ? '.zcode/cli/db' : '.zcode/cli/rollout', 'native'],
  ['doubao', 'Doubao', '', 'import'],
  ['trae', 'Trae', '', 'import'],
  ['dsh', 'DeepSeek Harness', '.dsh/sessions', 'native'],
].map(([id, name, dir, support]) => ({ id, name, support, defaultRoot: id === 'opencode' ? path.join(process.env.XDG_DATA_HOME || path.join(HOME, '.local', 'share'), 'opencode') : dir ? path.join(id === 'dsh' && process.env.DSH_HOME ? process.env.DSH_HOME : HOME, id === 'dsh' && process.env.DSH_HOME ? 'sessions' : dir) : '' }));
function validMonitors(monitors) {
  return Array.isArray(monitors) && monitors.every(m => m && AGENTS.some(a => a.id === m.agent) &&
    typeof m.root === 'string' && typeof m.enabled === 'boolean');
}
function normalizeMonitors(monitors) {
  const seen = new Set();
  return monitors.map(m => ({ agent: m.agent, root: m.root.trim() ? path.normalize(m.root.trim()) : '', enabled: m.enabled }))
    .filter(m => { const key = m.agent + ':' + (process.platform === 'win32' ? m.root.toLowerCase() : m.root); if (seen.has(key)) return false; seen.add(key); return true; });
}
function readSavedConfig() {
  for (const file of [CONFIG_FILE,CONFIG_FILE+'.bak']) {
    try { return JSON.parse(fs.readFileSync(file,'utf8')); } catch {}
  }
  throw new Error('没有可读取的配置');
}
function loadConfig() {
  try {
    const j = readSavedConfig();
    if (Array.isArray(j.roots)) CFG.roots = j.roots;
    if (typeof j.setupDone === 'boolean') CFG.setupDone = j.setupDone;
    else if (CFG.roots.length) CFG.setupDone = true; // 旧版配置已选过目录，视为完成引导
    if (typeof j.shortcut === 'boolean') CFG.shortcut = j.shortcut;
    if (validMonitors(j.monitors)) CFG.monitors = normalizeMonitors(j.monitors);
    if (Array.isArray(j.models) && j.models.every(m => typeof m === 'string')) CFG.models = j.models;
  } catch { }
  if (!CFG.roots.length && CFG.monitors === null && !CFG.setupDone) {
    if (fs.existsSync(DEFAULT_ROOT)) CFG.roots.push(DEFAULT_ROOT); // 首次运行自动使用本机默认目录
  }
  CFG.roots = [...new Set(CFG.roots.map(p => path.normalize(String(p).trim())).filter(Boolean))];
}
loadConfig();
function saveConfig() {
  const temp = CONFIG_FILE + '.tmp';
  try {
    const fd = fs.openSync(temp, 'w');
    try { fs.writeFileSync(fd, JSON.stringify({port:PORT,...CFG},null,2)); fs.fsyncSync(fd); }
    finally { fs.closeSync(fd); }
    if (fs.existsSync(CONFIG_FILE)) {
      const old = fs.readFileSync(CONFIG_FILE,'utf8');
      let valid=false;try {JSON.parse(old);valid=true;} catch {}
      if (valid) {const backup=fs.openSync(CONFIG_FILE+'.bak.tmp','w');try {fs.writeFileSync(backup,old);fs.fsyncSync(backup);} finally {fs.closeSync(backup);}}
      if (valid) fs.renameSync(CONFIG_FILE + '.bak.tmp',CONFIG_FILE + '.bak');
    }
    fs.renameSync(temp,CONFIG_FILE);
  } finally { try {fs.unlinkSync(temp);} catch {} }

}

// 单文件 exe（SEA）模式下首次运行自动创建桌面快捷方式（仅 Windows；由 config.json 的 shortcut 标记保证只创建一次）
function ensureWinShortcut() {
  if (process.platform !== 'win32' || !isSEA || CFG.shortcut || process.env.KIMI_NO_SHORTCUT) return;
  CFG.shortcut = true;
  saveConfig();
  try {
    const cp = require('child_process');
    const exe = process.execPath;
    const ico = path.join(CONFIG_DIR, 'AgentMonitor.ico');
    try { fs.writeFileSync(ico, ASSETS['favicon.ico']); } catch { }
    const scr = `$d=[Environment]::GetFolderPath('Desktop');$ws=New-Object -ComObject WScript.Shell;$l=$ws.CreateShortcut($d+'\\AgentMonitor.lnk');$l.TargetPath='${exe}';$l.WorkingDirectory='${path.dirname(exe)}';$l.IconLocation='${ico}';$l.Save()`;
    cp.exec(`powershell -NoProfile -ExecutionPolicy Bypass -Command "${scr}"`, { windowsHide: true }, () => { });
  } catch { }
}

// ---------- 关闭 Edge 应用窗口（按页面标题匹配，WM_CLOSE 只关本应用窗口，不影响其他 Edge 标签） ----------
const APP_TITLE = 'AgentMonitor';
const WIN32_CLOSE_CS = "using System;using System.Runtime.InteropServices;using System.Text;public class Win32Close{public delegate bool EnumProc(IntPtr h,IntPtr l);[DllImport(\"user32.dll\")]public static extern bool EnumWindows(EnumProc cb,IntPtr l);[DllImport(\"user32.dll\")]public static extern int GetWindowText(IntPtr h,StringBuilder t,int n);[DllImport(\"user32.dll\")]public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);[DllImport(\"user32.dll\")]public static extern bool IsWindowVisible(IntPtr h);public static void CloseByTitle(string title){EnumWindows(delegate(IntPtr h,IntPtr l){if(IsWindowVisible(h)){var sb=new StringBuilder(256);GetWindowText(h,sb,256);if(sb.ToString().Contains(title))PostMessage(h,0x0010,IntPtr.Zero,IntPtr.Zero);}return true;},IntPtr.Zero);}}";
function closeAppWindows() {
  if (process.platform !== 'win32') return;
  try {
    const ps = [
      `Add-Type -TypeDefinition '${WIN32_CLOSE_CS}'`,
      `[Win32Close]::CloseByTitle('${APP_TITLE}')`,
    ].join("\r\n");
    const enc = Buffer.from(ps, 'utf16le').toString('base64');
    const closer = require('child_process').spawn('powershell',
      ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', enc],
      { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] }
    );
    closer.stdout.resume(); closer.stderr.resume();
    closer.unref();
  } catch { }
}

// ---------- 托盘图标（Windows）：窗口关闭后在后台运行，左键/右键菜单可打开窗口或退出 ----------
function startTray() {
  if (process.platform !== 'win32' || process.env.KIMI_NO_TRAY) return;
  try {
    let ico = path.join(PUBLIC_DIR, 'favicon.ico');
    if (isSEA) {
      ico = path.join(CONFIG_DIR, 'AgentMonitor.ico');
      try { fs.writeFileSync(ico, ASSETS['favicon.ico']); } catch { }
    }
    const ps = [
      `Add-Type -TypeDefinition '${WIN32_CLOSE_CS}'`,
      "Add-Type -AssemblyName System.Windows.Forms",
      "Add-Type -AssemblyName System.Drawing",
      `$ico = New-Object System.Drawing.Icon('${ico.replace(/'/g, "''")}')`,
      "$ni = New-Object System.Windows.Forms.NotifyIcon",
      "$ni.Icon = $ico",
      "$ni.Text = 'AgentMonitor — 点击打开'",
      "$ni.Visible = $true",
      `$base = 'http://127.0.0.1:${PORT}'`,
      "$menu = New-Object System.Windows.Forms.ContextMenuStrip",
      "$openItem = $menu.Items.Add('打开窗口')",
      "$openItem.add_Click({ try { Invoke-RestMethod -Method Post -Uri \"$base/api/open\" | Out-Null } catch {} })",
      "$quitItem = $menu.Items.Add('退出')",
      // 关闭应用窗口 → 按 PID 结束服务 → 立即收起托盘图标
      `$quitItem.add_Click({ [Win32Close]::CloseByTitle('${APP_TITLE}'); try { Stop-Process -Id ${process.pid} -Force -ErrorAction Stop } catch {}; $ni.Visible = $false; $timer.Stop(); [System.Windows.Forms.Application]::Exit() })`,
      "$ni.ContextMenuStrip = $menu",
      "$ni.add_Click({ if ($_.Button -eq [System.Windows.Forms.MouseButtons]::Left) { try { Invoke-RestMethod -Method Post -Uri \"$base/api/open\" | Out-Null } catch {} } })",
      "$timer = New-Object System.Windows.Forms.Timer",
      "$timer.Interval = 3000",
      "$timer.add_Tick({ try { Invoke-RestMethod -Uri \"$base/api/data\" -TimeoutSec 2 | Out-Null } catch { $ni.Visible = $false; $timer.Stop(); [System.Windows.Forms.Application]::Exit() } })",
      "$timer.Start()",
      "[System.Windows.Forms.Application]::Run()",
      "$ni.Dispose()",
    ].join("\r\n");
    const enc = Buffer.from(ps, 'utf16le').toString('base64');
    // 注意：不能用 detached（PowerShell 无控制台会秒退）；stdio 必须是 pipe 并消费掉——
    // 'ignore' 的无效句柄同样会让 powershell 宿主静默退出（exit 0）
    const tray = require('child_process').spawn('powershell',
      ['-NoProfile', '-Sta', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', enc],
      { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] }
    );
    tray.stdout.resume(); tray.stderr.resume();
    tray.unref();
  } catch { }
}
// 引导期对默认目录做一次探测（缓存结果，避免每次轮询都全量扫描）
let defaultProbe = null;
function getDefaultProbe() {
  if (defaultProbe) return defaultProbe;
  const ok = fs.existsSync(DEFAULT_ROOT);
  defaultProbe = { root: DEFAULT_ROOT, ok, stats: ok && !CFG.setupDone ? probeRoot(DEFAULT_ROOT) : null };
  return defaultProbe;
}

// ---------- 缓存 ----------
// wire.jsonl 路径 -> { mtimeMs, size, records: [{t,m,a,i,o,r,c}] }
const wireCache = new Map();
// 会话目录 -> { mtimeMs, state }
const stateCache = new Map();
// session_index.jsonl 路径 -> { mtimeMs, map: sessionId -> workDir }
const indexCache = new Map();
let lastScan = { at: 0, data: null };

function toEpoch(v) {
  if (typeof v === 'number') return v;
  if (typeof v === 'string') { const t = Date.parse(v); return isNaN(t) ? 0 : t; }
  return 0;
}

// session_index.jsonl: sessionId -> workDir（state.json 缺 cwd 时的兜底）；
// 可能在数据目录本身或其上一级（.kimi-code 目录）
function loadIndexFor(root) {
  const candidates = [path.join(root, 'session_index.jsonl'), path.join(root, '..', 'session_index.jsonl')];
  for (const file of candidates) {
    let st;
    try { st = fs.statSync(file); } catch { continue; }
    const hit = indexCache.get(file);
    if (hit && hit.mtimeMs === st.mtimeMs) return hit.map;
    const map = new Map();
    try {
      for (const line of fs.readFileSync(file, 'utf8').split('\n')) {
        if (!line.trim()) continue;
        try { const j = JSON.parse(line); if (j.sessionId) map.set(j.sessionId, j.workDir || ''); } catch { }
      }
    } catch { }
    indexCache.set(file, { mtimeMs: st.mtimeMs, map });
    return map;
  }
  return new Map();
}

// ---------- 解析 ----------
function listDir(p) {
  try { return fs.readdirSync(p, { withFileTypes: true }); } catch { return []; }
}

function parseWireFile(file) {
  let text;
  try { text = fs.readFileSync(file, 'utf8'); } catch { return []; }
  const out = [];
  for (const [lineIndex, line] of text.split('\n').entries()) {
    if (!line.includes('usage.record')) continue;
    try {
      const ev = JSON.parse(line);
      if (ev.type !== 'usage.record' || !ev.usage) continue;
      out.push({
        _key: String(lineIndex), _origin: file,
        t: typeof ev.time === 'number' ? ev.time : 0,
        m: ev.model || 'unknown',
        a: ev.agentId || 'main',
        i: tokenCount(ev.usage.inputOther),
        o: tokenCount(ev.usage.output),
        r: tokenCount(ev.usage.inputCacheRead),
        c: tokenCount(ev.usage.inputCacheCreation),
      });
    } catch { /* 跳过损坏行 */ }
  }
  return out;
}

function tokenCount(value) {
  return Number.isSafeInteger(value) && value >= 0 ? value : 0;
}

function readState(sessDir) {
  const file = path.join(sessDir, 'state.json');
  let st;
  try { st = fs.statSync(file); } catch { return { mtimeMs: 0, state: null }; }
  const hit = stateCache.get(sessDir);
  if (hit && hit.mtimeMs === st.mtimeMs) return hit;
  let state = null;
  try { state = JSON.parse(fs.readFileSync(file, 'utf8')); } catch { }
  const entry = { mtimeMs: st.mtimeMs, state };
  stateCache.set(sessDir, entry);
  return entry;
}

// 扫描会话目录（含 agents/，state.json 可缺省，项目目录可由索引兜底）
function scanSession(sessPath, sessName, index, sessions, records, seen, seenSessions) {
  try { sessPath = fs.realpathSync(sessPath); } catch { return; }
  const key = process.platform === 'win32' ? sessPath.toLowerCase() : sessPath;
  if (seenSessions.has(key)) return;
  seenSessions.add(key);
  const { state } = readState(sessPath);
  const sessIdx = sessions.length;
  sessions.push({
    id: sessName,
    cwd: (state && state.cwd) || index.get(sessName) || '',
    title: (state && state.title) || '',
    createdAt: toEpoch(state && state.createdAt),
    updatedAt: toEpoch(state && state.updatedAt),
  });
  const agentsDir = path.join(sessPath, 'agents');
  for (const ag of listDir(agentsDir)) {
    if (!ag.isDirectory()) continue;
    const wire = path.join(agentsDir, ag.name, 'wire.jsonl');
    let st;
    try { st = fs.statSync(wire); } catch { continue; }
    seen.add(wire);
    let entry = wireCache.get(wire);
    if (!entry || entry.mtimeMs !== st.mtimeMs || entry.size !== st.size) {
      entry = { mtimeMs: st.mtimeMs, size: st.size, records: parseWireFile(wire) };
      wireCache.set(wire, entry);
    }
    for (const r of entry.records) records.push(Object.assign({ s: sessIdx }, r));
  }
}

// 扫描一个数据目录。兼容两种布局：
//   root\<工作区>\<会话>\...   （.kimi-code\sessions 原生结构）
//   root\<会话>\...            （直接指向某个 sessions 子目录或备份）
function scanRoot(root, sessions, records, seen, seenSessions = new Set()) {
  const index = loadIndexFor(root);
  for (const d of listDir(root)) {
    if (!d.isDirectory()) continue;
    const p1 = path.join(root, d.name);
    if (fs.existsSync(path.join(p1, 'state.json')) || fs.existsSync(path.join(p1, 'agents'))) { scanSession(p1, d.name, index, sessions, records, seen, seenSessions); continue; }
    for (const d2 of listDir(p1)) {
      if (!d2.isDirectory()) continue;
      const p2 = path.join(p1, d2.name);
      if (fs.existsSync(path.join(p2, 'state.json')) || fs.existsSync(path.join(p2, 'agents'))) scanSession(p2, d2.name, index, sessions, records, seen, seenSessions);
    }
  }
}

function scan() {
  const seen = new Set();
  const seenSessions = new Set();
  const sessions = [];
  const records = [];
  for (const root of CFG.roots) scanRoot(root, sessions, records, seen, seenSessions);
  for (const s of sessions) s.tool = 'kimi';
  for (const r of records) r.tool = 'kimi';
  const eventIds = new Map();
  const warnings = [];
  for (const monitor of CFG.monitors || []) {
    if (!monitor.enabled || !monitor.root) continue;
    if (monitor.agent === 'kimi') {
      const si = sessions.length, ri = records.length;
      scanRoot(monitor.root, sessions, records, seen, seenSessions);
      for (const s of sessions.slice(si)) s.tool = 'kimi';
      for (const r of records.slice(ri)) r.tool = 'kimi';
    } else {
      try { scanAgent(monitor, sessions, records, seen, eventIds, warnings); }
      catch (e) { warnings.push({agent:monitor.agent,root:monitor.root,error:String(e.message || e)}); }
    }
  }
  enrichSessionTitles(sessions, warnings);
  mergeHistory(sessions, records, warnings);
  finalizeAgentRecords(records);
  for (const session of sessions) if (session.tool !== 'kimi') {session.createdAt=Infinity;session.updatedAt=0;}
  for (const rec of records) {const session=sessions[rec.s];if (session.tool === 'kimi') continue;session.createdAt=Math.min(session.createdAt,rec.t);session.updatedAt=Math.max(session.updatedAt,rec.t);}
  for (const session of sessions) if (!Number.isFinite(session.createdAt)) session.createdAt=0;
  const availableModels = [...new Set(records.map(r => r.m))].sort();
  if (CFG.models.length) {
    const allowed = new Set(CFG.models);
    for (let n = records.length - 1; n >= 0; n--) if (!allowed.has(records[n].m)) records.splice(n, 1);
  }
  for (const k of wireCache.keys()) if (!seen.has(k)) wireCache.delete(k);
  return { sessions, records, availableModels, warnings };
}

// 只解析明确的用量事件；不会把聊天文字或会话数量估算为 token。
function parseAgentFile(file, agent) {
  const sessionMap = new Map(), out = new Map();
  let model = 'unknown', cwd = '', id = path.basename(file, '.jsonl'), retry = 0, seededAt=0;
  const labels=new Map();
  for (const line of readLogLines(file)) {
    if (agent === 'codex' && !['session_meta','turn_context','token_count','user_message'].some(type => line.includes(type))) continue;
    if (['claude','workbuddy','codebuddy'].includes(agent) && !['usage','user','title','summary'].some(k=>line.includes(k))) continue;
    let e; try { e = JSON.parse(line); } catch { continue; }
    const p = e.payload || {};
    if (agent === 'codex' && e.type === 'session_meta') { id = p.id || id; cwd = p.cwd || cwd; }
    if (agent === 'codex' && e.type === 'turn_context') { model = p.model || model; cwd = p.cwd || cwd; }
    if (agent === 'dsh' && e.type === 'session') { id = e.id || id; cwd = e.cwd || cwd; seededAt=e.isSeeded ? toEpoch(e.createdAt) : 0; }
    if (agent === 'dsh' && e.type === 'request/header') model = e.data?.header?.config?.model || model;
    if (agent === 'dsh' && e.type === 'session/end-seed' && e.data?.inherited === true) { out.clear(); sessionMap.clear(); }
    if (agent === 'dsh' && e.type === 'llm/retry-started') retry++;
    const labelId=e.sessionId || id;
    const explicit=e.customTitle || e.title || (e.type==='custom-title' ? e.customTitle : e.type==='summary' ? e.summary : e.type==='session/title' ? e.data?.title : '');
    if (typeof explicit==='string' && explicit.trim()) labels.set(labelId,{title:explicit.trim(),priority:2});
    if (!labels.has(labelId)) {
      let content=e.type==='user' ? e.message?.content : e.type==='message' && e.role==='user' ? e.content : agent==='codex' && p.type==='user_message' ? p.message : e.type==='user/message' ? e.data?.message?.content : '';
      if(Array.isArray(content)) content=content.filter(x=>x.type==='text').map(x=>x.text || '').join(' ');
      if(typeof content==='string' && content.trim()) labels.set(labelId,{title:'首条提问：'+content.replace(/\s+/g,' ').trim().slice(0,120),priority:1});
    }
    let u, key, inclusive = false;
    if (agent === 'zcode' && e.type === 'model_io') {
      if (!e.response?.usage) continue;
      u = e.response.providerMetadata?.anthropic?.usage || e.response.usage;
      inclusive = !e.response.providerMetadata?.anthropic?.usage;
      model = e.response.modelId || e.model?.modelId || 'unknown';
      key = (e.sessionId || id) + ':' + (e.response.responseId || `${e.requestId || out.size}:${e.attempt || 0}`);
    } else if (agent === 'dsh' && ['assistant/message','assistant/attempt'].includes(e.type)) {
      const d = e.data || {};
      const samples = (d.stream || []).filter(x => x.type === 'chunk' && x.chunk?.type === 'usage');
      u = d.usage || samples[samples.length - 1]?.chunk.usage;
      if (!u) continue;
      model = d.message?.source?.model || model;
      key = id + ':' + d.turn + ':' + d.step + ':' + retry;
    } else if (agent === 'codex') {
      if (e.type !== 'event_msg' || p.type !== 'token_count' || !p.info?.total_token_usage) continue;
      const total = p.info.total_token_usage;
      const fields = ['input_tokens', 'output_tokens', 'cached_input_tokens', 'cache_write_input_tokens'];
      const counts = fields.map(k => tokenCount(total[k]));
      u = { input_tokens: counts[0], output_tokens: counts[1], cached_input_tokens: counts[2], cache_write_input_tokens: counts[3] };
      inclusive = true;
      key = id + ':' + counts.join(':');
    } else if (['claude', 'workbuddy', 'codebuddy'].includes(agent)) {
      if (!(agent === 'claude' ? e.type === 'assistant' : ['assistant', 'function_call', 'message'].includes(e.type)) || !e.message?.usage) continue;
      if (e.isApiErrorMessage || e.message.model === '<synthetic>') continue;
      u = e.message.usage;
      model = e.message.model || e.providerData?.model || model;
      key = (e.sessionId || id) + ':' + (e.message.id || e.providerData?.messageId || e.requestId || e.uuid || e.id || out.size);
      inclusive = agent !== 'claude' && !!e.providerData?.rawUsage;
    } else {
      if (e.type !== 'usage.record' || !e.usage) continue;
      u = e.usage; model = e.model || 'unknown';
      key = (e.sessionId || id) + ':' + (e.eventId || e.id || out.size);
    }
    const t = toEpoch(e.timestamp ?? e.time ?? e.completedAt ?? e.startedAt);
    if (t <= 0 || agent==='dsh' && seededAt && t<seededAt) continue;
    const sid = e.sessionId || id;
    cwd = e.cwd || cwd;
    sessionMap.set(sid, { id: sid, cwd, title: labels.get(sid)?.title || '', tool: agent, createdAt: t, updatedAt: t });
    const r = tokenCount(u.inputCacheRead ?? u.cache_read_input_tokens ?? u.cached_input_tokens ?? u.cacheReadTokens);
    const c = tokenCount(u.inputCacheCreation ?? u.cache_creation_input_tokens ?? u.cache_write_input_tokens ?? u.cacheWriteTokens);
    const input = tokenCount(u.inputOther ?? u.input_tokens ?? u.inputTokens);
    const record = { _origin: file, sid, key, t, m: model, tool: agent, a: e.agentId || 'main',
      i: input, _inclusive: inclusive,
      o: tokenCount(u.output ?? u.output_tokens ?? u.outputTokens), r, c };
    const existing = out.get(key);
    if (existing && agent !== 'dsh') {
      for (const field of ['i','o','r','c']) record[field] = Math.max(record[field],existing[field]);
      record._inclusive ||= existing._inclusive;
      if (agent === 'codex' && existing.t < record.t) Object.assign(record,{t:existing.t,m:existing.m,a:existing.a});
    }
    out.set(key, record);
  }
  for(const session of sessionMap.values()) session.title=labels.get(session.id)?.title || session.title;
  return { sessions: [...sessionMap.values()], records: [...out.values()] };
}
// 按块读取大型 JSONL，避免 ZCode model-io 日志占满内存。
function* readLogLines(file) {
  let fd;
  try {
    if (file.endsWith('.zstd')) {
      const decompress = require('node:zlib').zstdDecompressSync;
      if (!decompress) throw new Error('当前 Node.js 不支持 Zstandard 解压，请升级到 22.15+ 或使用桌面版');
      const compressed=fs.readFileSync(file),chunks=[];
      let offset=0,total=0;
      while(offset<compressed.length) {
        const result=decompress(compressed.subarray(offset),{info:true,maxOutputLength:256*1024*1024-total});
        const consumed=result.engine.bytesWritten;
        if(!consumed) throw new Error('Zstandard 解压未推进');
        offset+=consumed;total+=result.buffer.length;chunks.push(result.buffer);
        if(total>=256*1024*1024 && offset<compressed.length) throw new Error('解压内容超过 256 MiB');
      }
      yield* Buffer.concat(chunks).toString('utf8').split('\n');
      return;
    }
    fd = fs.openSync(file, 'r');
    const decoder = new (require('node:string_decoder').StringDecoder)('utf8');
    const buffer = Buffer.alloc(64 * 1024);
    let pending = [], n;
    while ((n = fs.readSync(fd, buffer, 0, buffer.length, null)) > 0) {
      const chunk = decoder.write(buffer.subarray(0, n));
      let start = 0, end;
      while ((end = chunk.indexOf('\n', start)) >= 0) {
        pending.push(chunk.slice(start, end)); yield pending.join(''); pending = []; start = end + 1;
      }
      if (start < chunk.length) pending.push(chunk.slice(start));
    }
    pending.push(decoder.end()); const tail = pending.join(''); if (tail.trim()) yield tail;
  } finally { if (fd !== undefined) fs.closeSync(fd); }
}
function scanAgent(monitor, sessions, records, seen, eventIds, warnings) {
  if (monitor.agent === 'opencode') {
    appendAgentRecords(parseOpencode(monitor.root, warnings), monitor.agent, sessions, records, eventIds);
    return;
  }
  const database = path.join(monitor.root, 'db.sqlite');
  if (monitor.agent === 'zcode' && fs.existsSync(database)) {
    const parsed = parseZcodeDatabase(database);
    appendAgentRecords(parsed, monitor.agent, sessions, records, eventIds);
    return;
  }
  const files = [], visited = new Set();
  function walk(dir, depth = 0) {
    if (depth > 12) return;
    const real = fs.realpathSync(dir);
    if (visited.has(real)) return; visited.add(real);
    for (const e of fs.readdirSync(real, {withFileTypes:true})) {
      const p = path.join(real, e.name);
      if (e.isDirectory()) {try {walk(p, depth + 1);} catch (error) {warnings.push({agent:monitor.agent,root:monitor.root,file:p,error:error.message});}}
      else if (e.isFile() && (e.name.endsWith('.jsonl') || monitor.agent === 'dsh' && e.name.endsWith('.jsonl.zstd'))) files.push(p);
    }
  }
  walk(monitor.root);
  // DSH 保留历史格式代，只读取每个 session 目录中的最高代。
  const selected = monitor.agent === 'dsh' ? selectDshFiles(files) : files;
  for (const file of selected.sort()) {
    const cacheKey = monitor.agent + ':' + file;
    seen.add(cacheKey);
    try {
      const st = fs.statSync(file);
      let hit = wireCache.get(cacheKey);
      if (!hit || hit.mtimeMs !== st.mtimeMs || hit.size !== st.size) {
        hit = { mtimeMs: st.mtimeMs, size: st.size, data: parseAgentFile(file, monitor.agent) };
        wireCache.set(cacheKey, hit);
      }
      appendAgentRecords(hit.data, monitor.agent, sessions, records, eventIds);
    } catch (e) { warnings.push({agent:monitor.agent,root:monitor.root,file,error:String(e.message || e)}); }
  }
}
function appendAgentRecords(data, agent, sessions, records, eventIds) {
    const sessionIndices = new Map();
    for (const r of data.records) {
      const eventKey = agent + ':' + r.key;
      const metadata=data.sessions.find(s=>s.id===r.sid);
      const existingSession=sessions.find(s=>s.tool===agent && s.id===r.sid);
      if(metadata?.title && existingSession && (!existingSession.title || existingSession.title.startsWith('首条提问：'))) existingSession.title=metadata.title;
      const existing = eventIds.get(eventKey);
      if (existing !== undefined) {
        const old = records[existing];
        if (agent === 'dsh') { if (r.t >= old.t) Object.assign(old, r, {s:old.s}); }
        else {
          for (const field of ['i','o','r','c']) old[field] = Math.max(old[field], r[field]);
          if (agent === 'codex' ? r.t < old.t : r.t >= old.t) Object.assign(old, {t:r.t,m:r.m,a:r.a});
          old._inclusive ||= r._inclusive;
        }
        continue;
      }
      let si = sessionIndices.get(r.sid);
      if (si === undefined) {
        si = sessions.findIndex(s => s.tool === agent && s.id === r.sid);
        if (si < 0) { si = sessions.length; sessions.push(data.sessions.find(s => s.id === r.sid)); }
        sessionIndices.set(r.sid, si);
      }
      const { sid, key, ...rec } = r;
      eventIds.set(eventKey, records.length);
      records.push({ s: si, ...rec, _key: key });
    }
}
// 累计快照跨文件统一排序求差；原始输入合并完成后才扣缓存。
function finalizeAgentRecords(records) {
  const previous = new Map();
  records.sort((a,b) => a.t - b.t);
  for (const rec of records) {
    if (rec.tool === 'codex') {
      const counts = [rec.i,rec.o,rec.r,rec.c];
      const key = rec.s;
      const prev = previous.get(key) || [0,0,0,0];
      [rec.i,rec.o,rec.r,rec.c] = counts.map((n,i) => Math.max(0,n-prev[i]));
      previous.set(key, counts.map((n,i)=>Math.max(n,prev[i])));
    }
    if (rec._inclusive) rec.i = Math.max(0,rec.i-rec.r-rec.c);
    delete rec._origin; delete rec._key; delete rec._inclusive; delete rec.sid; delete rec.key;
  }
  for (let i=records.length-1;i>=0;i--) if (records[i].tool === 'codex' && !['i','o','r','c'].some(k=>records[i][k])) records.splice(i,1);
}
function enrichSessionTitles(sessions,warnings) {
  const sources=new Map();
  for(const monitor of CFG.monitors || []) {
    if(!monitor.enabled || !['codex','workbuddy','codebuddy'].includes(monitor.agent)) continue;
    let dir=path.resolve(monitor.root);
    for(let depth=0;depth<5;depth++) {
      let files=[];try {files=fs.readdirSync(dir);} catch {}
      const file=monitor.agent==='codex' ? files.filter(n=>/^state_\d+\.sqlite$/.test(n)).sort((a,b)=>Number(b.match(/\d+/)[0])-Number(a.match(/\d+/)[0]))[0] : files.find(n=>n===(monitor.agent==='workbuddy'?'workbuddy.db':'codebuddy.db'));
      if(file) {sources.set(monitor.agent+':'+path.join(dir,file),{agent:monitor.agent,root:monitor.root,file:path.join(dir,file)});break;}
      const parent=path.dirname(dir);if(parent===dir) break;dir=parent;
    }
  }
  if(!DatabaseSync) return;
  for(const source of sources.values()) {
    let db;
    try {
      db=new DatabaseSync(source.file,{readOnly:true});
      const table=source.agent==='codex'?'threads':'sessions';
      const columns=db.prepare(`PRAGMA table_info(${table})`).all().map(x=>x.name);
      const expression=columns.includes('custom_title') ? "COALESCE(NULLIF(TRIM(custom_title),''),title)" : 'title';
      const first=columns.includes('first_user_message')?'first_user_message':'NULL';
      const nickname=columns.includes('agent_nickname')?'agent_nickname':'NULL';
      const rows=db.prepare(`SELECT id,${expression} AS title,${first} AS first,${nickname} AS nickname FROM ${table}`).all();
      const metadata=new Map(rows.map(r=>[r.id,r]));
      for(const session of sessions) {
        if(session.tool!==source.agent) continue;
        const row=metadata.get(session.id);if(!row) continue;
        if(typeof row.title==='string' && row.title.trim()) session.title=row.title.trim();
        else if(!session.title && typeof row.first==='string' && row.first.trim()) session.title='首条提问：'+row.first.replace(/\s+/g,' ').trim().slice(0,120);
        else if(!session.title && typeof row.nickname==='string' && row.nickname.trim()) session.title='子 Agent：'+row.nickname.trim();
      }
    } catch(error) {warnings.push({...source,error:'会话标题读取失败：'+error.message});}
    finally {if(db) db.close();}
  }
}
function parseZcodeDatabase(file) {
  if (!DatabaseSync) throw new Error('读取 ZCode 数据库需要 Node.js 22.13+，或选择 CLI rollout JSONL 目录');
  const db = new DatabaseSync(file, {readOnly:true});
  try {
    const rows = db.prepare(`SELECT u.id, u.session_id, u.model_id, u.agent, u.started_at,
      u.completed_at, u.input_tokens, u.output_tokens, u.cache_read_input_tokens,
      u.cache_creation_input_tokens, s.directory AS cwd, s.title
      FROM model_usage u LEFT JOIN session s ON s.id = u.session_id
      WHERE u.input_tokens > 0 OR u.output_tokens > 0 OR u.cache_read_input_tokens > 0 OR u.cache_creation_input_tokens > 0
      ORDER BY u.started_at, u.id`).all();
    const sessionMap = new Map(), records = [];
    for (const row of rows) {
      const t = toEpoch(row.completed_at || row.started_at); if (t <= 0) continue;
      const sid = row.session_id;
      sessionMap.set(sid, {id:sid,cwd:row.cwd || '',title:row.title || '',tool:'zcode',createdAt:t,updatedAt:t});
      records.push({_origin:file,sid,key:sid+':'+row.id,t,m:row.model_id || 'unknown',tool:'zcode',a:row.agent || 'main',
        i:tokenCount(row.input_tokens),_inclusive:true,o:tokenCount(row.output_tokens),r:tokenCount(row.cache_read_input_tokens),c:tokenCount(row.cache_creation_input_tokens)});
    }
    return {sessions:[...sessionMap.values()],records};
  } finally { db.close(); }
}
function selectDshFiles(files) {
  const generations = new Map(), other = [];
  for (const file of files) {
    const match = path.basename(file).match(/^session(?:\.v(\d+))?\.jsonl(?:\.zstd)?$/);
    if (!match) { other.push(file); continue; }
    const key = path.dirname(file), version = Number(match[1] || 0);
    const old = generations.get(key);
    if (!old || version > old.version) generations.set(key, {file,version});
  }
  return [...other, ...[...generations.values()].map(v => v.file)];
}

function monitoringData() {
  return { agents: AGENTS.map(a => ({ ...a, detected: !!a.defaultRoot && fs.existsSync(a.defaultRoot), compressedLogs: a.id === 'dsh' ? typeof require('node:zlib').zstdDecompressSync === 'function' : undefined })),
    monitors: (CFG.monitors || CFG.roots.map(root => ({ agent: 'kimi', root, enabled: true }))).map(m => ({ ...m, exists: !!m.root && fs.existsSync(m.root) })), models: CFG.models };
}

function probeRoot(root) {
  const sessions = [], records = [], seen = new Set();
  try { scanRoot(root, sessions, records, seen); } catch { }
  return { path: root, ok: fs.existsSync(root), sessions: sessions.length, records: records.length };
}

function getData() {
  const now = Date.now();
  if (lastScan.data && now - lastScan.at < 800) return lastScan.data; // 限流
  const data = scan();
  lastScan = { at: now, data };
  return data;
}

// 原始事件先归档，再做累计差值与模型筛选。源目录消失不删除历史。
const HISTORY_FILE = path.join(CONFIG_DIR, 'usage-history.json');
let history = new Map(), historyLoaded = false, historyDirty = false, historyError = '';
function historyPath(value) {
  const absolute=path.resolve(value).replaceAll('\\','/').replace(/^\/\/\?\//,'');
  return process.platform === 'win32' ? absolute.toLowerCase() : absolute;
}
function sourceEnabled(origin, tool) {
  const roots = [...(tool === 'kimi' ? CFG.roots : []), ...(CFG.monitors || []).filter(m => m.enabled && m.agent === tool && m.root).map(m => m.root)];
  return roots.some(root => {
    const relative = path.relative(path.resolve(root), path.resolve(origin));
    return relative === '' || relative !== '..' && !relative.startsWith('..' + path.sep) && !path.isAbsolute(relative);
  });
}
function validHistory(data) {
  return data?.version === 1 && Array.isArray(data.entries) && data.entries.every(entry => {
    if (!Array.isArray(entry) || typeof entry[0] !== 'string') return false;
    const session=entry[1]?.session, record=entry[1]?.record;
    return session && record && ['id','cwd','title','tool'].every(k=>typeof session[k]==='string') &&
      ['createdAt','updatedAt'].every(k=>Number.isFinite(session[k])) && session.tool===record.tool &&
      ['_key','_origin','m','a','tool'].every(k=>typeof record[k]==='string') && Number.isFinite(record.t) &&
      ['i','o','r','c'].every(k=>Number.isSafeInteger(record[k]) && record[k]>=0) &&
      (record._inclusive === undefined || typeof record._inclusive === 'boolean');
  });
}
function mergeHistory(sessions, records, warnings) {
  if (!historyLoaded) {
    historyLoaded = true;
    for (const file of [HISTORY_FILE, HISTORY_FILE + '.bak']) {
      try {
        const data = JSON.parse(fs.readFileSync(file, 'utf8'));
        if (!validHistory(data)) throw new Error('用量档案格式无效');
        history = new Map(data.entries); historyError = ''; historyDirty = file !== HISTORY_FILE; break;
      } catch (e) { if (e.code !== 'ENOENT') historyError = e.message; }
    }
  }
  if (historyError) warnings.push({agent:'history', error:'用量档案读取失败：' + historyError});
  let changed = false;
  for (const rec of records) {
    const session = sessions[rec.s];
    const {s, ...record} = rec;
    record._origin=historyPath(record._origin);
    const key = JSON.stringify([rec.tool, session.id, rec.tool === 'kimi' ? record._origin : '', rec._key]);
    const old = history.get(key);
    if (old && rec.tool !== 'dsh') {
      for (const f of ['i','o','r','c']) record[f] = Math.max(record[f], old.record[f]);
      record._inclusive ||= old.record._inclusive;
      if (rec.tool === 'codex') record.t = Math.min(record.t,old.record.t);
    }
    const metadata={...session};
    if (old) {metadata.title ||= old.session.title;metadata.cwd ||= old.session.cwd;}
    const value = {session:metadata,record};
    if (JSON.stringify(old) !== JSON.stringify(value)) { history.set(key,value); changed = true; }
  }
  historyDirty ||= changed;
  if (historyDirty && !historyError) {
    try {
      const tmp = HISTORY_FILE + '.tmp';
      const fd = fs.openSync(tmp,'w');
      try {fs.writeFileSync(fd,JSON.stringify({version:1,entries:[...history]}));fs.fsyncSync(fd);} finally {fs.closeSync(fd);}
      if (fs.existsSync(HISTORY_FILE)) {
        let valid=false;try {const old=JSON.parse(fs.readFileSync(HISTORY_FILE,'utf8'));valid=validHistory(old);} catch {}
        if(valid) fs.copyFileSync(HISTORY_FILE,HISTORY_FILE+'.bak');
      }
      fs.renameSync(tmp,HISTORY_FILE);
      historyDirty=false;
    } catch (e) {warnings.push({agent:'history',error:'用量档案保存失败：'+e.message});}
  }
  records.length = 0;
  for (const {session,record} of history.values()) {
    if (!sourceEnabled(record._origin,record.tool)) continue;
    let si = sessions.findIndex(s=>s.tool===session.tool && s.id===session.id);
    if (si < 0) {si=sessions.length;sessions.push({...session});}
    else {sessions[si].title ||= session.title;sessions[si].cwd ||= session.cwd;}
    records.push({s:si,...record});
  }
}

// OpenCode 新旧消息表已汇总用量，不叠加 part 的 step-finish。
function parseOpencode(root, warnings = []) {
  const sessions = new Map(), records = [];
  function message(m, origin) {
    if (m.role !== 'assistant' || !m.tokens || !m.id || !m.sessionID) return;
    const u=m.tokens, t=toEpoch(m.time?.completed || m.time?.created);
    if (t <= 0 || !['input','output'].some(k=>tokenCount(u[k])) && !tokenCount(u.cache?.read) && !tokenCount(u.cache?.write)) return;
    const sid=m.sessionID;
    if (!sessions.has(sid)) sessions.set(sid,{id:sid,cwd:m.path?.cwd || '',title:'',tool:'opencode',createdAt:t,updatedAt:t});
    records.push({sid,key:sid+':'+m.id,_origin:origin,t,m:m.modelID || m.model?.id || 'unknown',tool:'opencode',a:m.agent || m.mode || 'main',i:tokenCount(u.input),o:tokenCount(u.output),r:tokenCount(u.cache?.read),c:tokenCount(u.cache?.write)});
  }
  const database=path.join(root,'opencode.db');
  if (fs.existsSync(database)) {
    if (!DatabaseSync) throw new Error('OpenCode 数据库需要 Node.js 内置 SQLite；请使用 Node.js 24 或桌面版');
    const db=new DatabaseSync(database,{readOnly:true});
    try {
      const tables=new Set(db.prepare("SELECT name FROM sqlite_master WHERE type='table'").all().map(r=>r.name));
      if (!tables.has('message') && !tables.has('session_message')) throw new Error('不支持的 OpenCode 数据库结构：缺少消息表');
      for (const table of ['session','session_v2']) if (tables.has(table)) {
        for (const row of db.prepare('SELECT id,directory,title FROM '+table).iterate()) sessions.set(row.id,{id:row.id,cwd:row.directory || '',title:row.title || '',tool:'opencode',createdAt:0,updatedAt:0});
      }
      for (const table of ['message','session_message']) if (tables.has(table)) {
        const modern=table==='session_message';
        for (const row of db.prepare('SELECT id,session_id,data'+(modern?',type':'')+' FROM '+table).iterate()) {
          try {message({...JSON.parse(row.data),id:row.id,sessionID:row.session_id,...(modern?{role:row.type}:{})},database);}
          catch (e) {warnings.push({agent:'opencode',root,file:database,error:`${table}/${row.id}：${e.message}`});}
        }
      }
    } finally {db.close();}
  } else {
    const storage=fs.existsSync(path.join(root,'storage')) ? path.join(root,'storage') : root;
    function walk(dir,kind,depth=0) {
      if (depth>4) return;
      for (const e of listDir(dir)) {
        const file=path.join(dir,e.name);
        if(e.isDirectory()) walk(file,kind,depth+1);
        else if(e.isFile() && e.name.endsWith('.json')) {
          try {
            const data=JSON.parse(fs.readFileSync(file,'utf8'));
            if(kind==='message') message(data,file);
            else if(data.id) sessions.set(data.id,{id:data.id,cwd:data.directory || '',title:data.title || '',tool:'opencode',createdAt:toEpoch(data.time?.created),updatedAt:toEpoch(data.time?.updated)});
          } catch (e) {warnings.push({agent:'opencode',root,file,error:e.message});}
        }
      }
    }
    walk(path.join(storage,'session'),'session');walk(path.join(storage,'message'),'message');
    if (!fs.existsSync(root)) throw new Error('OpenCode 数据目录不存在');
  }
  return {sessions:[...sessions.values()],records};
}

// ---------- HTTP ----------
const MIME = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8', '.svg': 'image/svg+xml', '.png': 'image/png', '.ico': 'image/x-icon', '.webmanifest': 'application/manifest+json' };

function json(res, body, status = 200) {
  res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store' });
  res.end(typeof body === 'string' ? body : JSON.stringify(body));
}

const server = http.createServer((req, res) => {
  let url, decodedPath;
  try {
    url = new URL(req.url, 'http://127.0.0.1');
    decodedPath = decodeURIComponent(url.pathname);
  } catch {
    json(res, { error: '请求路径无效' }, 400);
    return;
  }
  if (url.pathname === '/api/data') {
    let body;
    try {
      const d = getData();
      const dp = getDefaultProbe();
      body = JSON.stringify({ generatedAt: Date.now(), roots: CFG.roots, setupDone: !!CFG.setupDone, defaultRoot: dp.root, defaultOk: dp.ok, defaultStats: dp.stats, ...monitoringData(), sessions: d.sessions, records: d.records, availableModels: d.availableModels, warnings: d.warnings });
    } catch (e) {
      body = JSON.stringify({ generatedAt: Date.now(), roots: CFG.roots, setupDone: !!CFG.setupDone, sessions: [], records: [], error: String(e && e.message || e) });
    }
    json(res, body);
    return;
  }
  if (url.pathname === '/api/config' && req.method === 'POST') {
    let raw = '';
    let size = 0, tooLarge = false;
    req.setEncoding('utf8');
    req.on('data', c => {
      size += Buffer.byteLength(c);
      if (tooLarge) return;
      if (size > 1024 * 1024) {
        tooLarge = true;
        raw = '';
        json(res, { error: '配置请求过大' }, 413);
        return;
      }
      raw += c;
    });
    req.on('end', () => {
      if (tooLarge) return;
      let roots, setupDone, monitors, models;
      try {
        const j = JSON.parse(raw);
        if (!j || (j.roots === undefined && j.monitors === undefined) ||
            (j.roots !== undefined && (!Array.isArray(j.roots) || !j.roots.every(p => typeof p === 'string'))) ||
            (j.monitors !== undefined && !validMonitors(j.monitors)) ||
            (j.models !== undefined && (!Array.isArray(j.models) || !j.models.every(m => typeof m === 'string'))) ||
            (j.setupDone !== undefined && typeof j.setupDone !== 'boolean')) throw new Error('invalid config');
        roots = j.roots || [];
        monitors = j.monitors;
        models = j.models;
        if (typeof j.setupDone === 'boolean') setupDone = j.setupDone;
      } catch {
        json(res, { error: '需要合法的 roots 或 monitors，以及可选的 models 字符串数组、setupDone 布尔值' }, 400);
        return;
      }
      roots = [...new Set(roots.map(p => p.trim()).filter(Boolean).map(p => path.normalize(p)))];
      if (monitors !== undefined) {
        for (const m of monitors) {
          if (!m.enabled) continue;
          try {
            if (!m.root.trim() || !fs.statSync(m.root.trim()).isDirectory()) throw new Error('invalid directory');
            fs.accessSync(m.root.trim(), fs.constants.R_OK);
          } catch { json(res, { error: '已启用的监测目录不存在或无法读取：' + m.root }, 400); return; }
        }
      }
      const stats = roots.map(probeRoot);
      const previousConfig = { ...CFG };
      CFG.roots = roots.filter((_, i) => stats[i].ok);
      CFG.monitors = monitors === undefined ? null : normalizeMonitors(monitors);
      if (models !== undefined) CFG.models = [...new Set(models.map(m => m.trim()).filter(Boolean))];
      if (typeof setupDone === 'boolean') CFG.setupDone = setupDone;
      else CFG.setupDone = CFG.setupDone || CFG.roots.length > 0;
      try { saveConfig(); } catch { Object.assign(CFG, previousConfig); json(res, { error: '配置保存失败，请检查配置目录权限' }, 500); return; }
      wireCache.clear(); stateCache.clear(); indexCache.clear(); defaultProbe = null;
      lastScan = { at: 0, data: null };
      json(res, { ok: true, roots: CFG.roots, setupDone: !!CFG.setupDone, stats, ...monitoringData() });
    });
    return;
  }
  if (url.pathname === '/api/open' && req.method === 'POST') {
    res.writeHead(200, { 'Content-Type': 'text/plain; charset=utf-8' });
    res.end('ok');
    openBrowser(`http://127.0.0.1:${PORT}/`);
    return;
  }
  if (url.pathname === '/api/quit' && req.method === 'POST') {
    res.writeHead(200, { 'Content-Type': 'text/plain; charset=utf-8' });
    res.end('bye');
    closeAppWindows(); // 连 Edge 应用窗口一起关闭（托盘退出路径由托盘脚本自行关闭）
    setTimeout(() => process.exit(0), 150);
    return;
  }
  let p = decodedPath === '/' ? '/index.html' : decodedPath;
  p = path.normalize(p).replace(/^([/\\]|\.\.[/\\])+/g, '');
  if (isSEA && ASSETS[p]) {
    res.writeHead(200, { 'Content-Type': MIME[path.extname(p)] || 'application/octet-stream' });
    res.end(ASSETS[p]);
    return;
  }
  const file = path.join(PUBLIC_DIR, p);
  if (!file.startsWith(PUBLIC_DIR)) { res.writeHead(403); res.end(); return; }
  fs.readFile(file, (err, buf) => {
    if (err) { res.writeHead(404, { 'Content-Type': 'text/plain; charset=utf-8' }); res.end('404 Not Found'); return; }
    res.writeHead(200, { 'Content-Type': MIME[path.extname(file)] || 'application/octet-stream' });
    res.end(buf);
  });
});

function openBrowser(u) {
  if (process.platform !== 'win32') {
    try { require('child_process').spawn('open', [u], { detached: true, stdio: 'ignore' }).unref(); } catch { }
    return;
  }
  const pf86 = process.env['ProgramFiles(x86)'] || '';
  const edge = path.join(pf86, 'Microsoft', 'Edge', 'Application', 'msedge.exe');
  const edge64 = path.join(process.env.ProgramFiles || '', 'Microsoft', 'Edge', 'Application', 'msedge.exe');
  const cp = require('child_process');
  try {
    if (fs.existsSync(edge)) cp.spawn(edge, ['--app=' + u, '--window-size=1280,900'], { detached: true, stdio: 'ignore' }).unref();
    else if (fs.existsSync(edge64)) cp.spawn(edge64, ['--app=' + u, '--window-size=1280,900'], { detached: true, stdio: 'ignore' }).unref();
    else cp.spawn('cmd', ['/c', 'start', '', u], { detached: true, stdio: 'ignore' }).unref();
  } catch { }
}

// GUI 子系统（无控制台窗口）下 stdout 无效，写日志必须兜异常
const log = (...a) => { try { console.log(...a); } catch { } };
const logErr = (...a) => { try { console.error(...a); } catch { } };

server.on('error', e => {
  if (e && e.code === 'EADDRINUSE') {
    log(`[kimi-usage-monitor] 端口 ${PORT} 已被占用，本次服务未启动；请确认占用者是 AgentMonitor，或更换端口。`);
    if (isSEA && !process.env.KIMI_NO_OPEN) openBrowser(`http://127.0.0.1:${PORT}/`);
    process.exit(0);
  }
  logErr('[kimi-usage-monitor] 启动失败:', e);
  process.exit(1);
});

server.listen(PORT, '127.0.0.1', () => {
  try { if (process.platform === 'win32') fs.writeFileSync(path.join(CONFIG_DIR, 'port.txt'), String(PORT)); } catch { }
  log(`[kimi-usage-monitor] http://127.0.0.1:${PORT}`);
  const activeRoots = [...CFG.roots, ...(CFG.monitors || []).filter(m => m.enabled).map(m => m.root)];
  log(`[kimi-usage-monitor] 数据目录: ${activeRoots.join(' ; ') || '(未配置，请打开页面右上角"设置"添加)'}`);
  getData();
  setInterval(() => {try {getData();} catch (e) {log('后台采集失败：'+e.message);}}, 30000).unref();
  ensureWinShortcut();
  startTray();
  if (isSEA && !process.env.KIMI_NO_OPEN) openBrowser(`http://127.0.0.1:${PORT}/`);
});
