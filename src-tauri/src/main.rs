#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// KimiMonitor 桌面版（Tauri v2）：
// Rust 内核完整移植原 server.js 数据引擎（扫描 wire.jsonl / 聚合 usage.record），
// 内嵌 HTTP 服务（127.0.0.1:43110，被占用时回退随机端口），原生托盘（打开窗口/退出），
// 关窗行为可在设置中选择：直接退出 / 隐藏到后台（默认）/ 每次询问。不再依赖 PowerShell / Edge --app。

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
mod agents;

const APP_TITLE: &str = "AgentMonitor";
const PORT: u16 = 43110; // 优先端口；被占用时自动回退随机端口

const INDEX_HTML: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../public/index.html"));
const ICON_PNG: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../public/icon.png"));
const FAVICON_ICO: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../public/favicon.ico"));
const MANIFEST_WEBMANIFEST: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../public/manifest.webmanifest"));
const ICON_192: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../public/icon-192.png"));
const ICON_512: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../public/icon-512.png"));

// ---------- 数据结构 ----------
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Rec {
    #[serde(default, skip_serializing)]
    s: usize,
    #[serde(rename="_key")]
    key: String,
    #[serde(rename="_origin")]
    origin: String,
    t: i64,
    m: String,
    a: String,
    tool: String,
    i: i64,
    o: i64,
    r: i64,
    c: i64,
    #[serde(default, rename="_inclusive")]
    inclusive: bool,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Session {
    id: String,
    cwd: String,
    title: String,
    tool: String,
    #[serde(rename="createdAt")]
    created_at: i64,
    #[serde(rename="updatedAt")]
    updated_at: i64,
}

#[derive(Default, Clone)]
struct Config {
    roots: Vec<String>,
    setup_done: bool,
    monitors: Option<Vec<Value>>,
    models: Vec<String>,
    close_action: String,
}

#[derive(Default)]
struct Engine {
    config_dir: PathBuf,
    cfg: Config,
    // wire.jsonl 路径 -> (mtime_ms, size, records)
    wire_cache: HashMap<PathBuf, (i64, u64, Vec<Rec>)>,
    // 会话目录 -> (mtime_ms, state.json Value)
    state_cache: HashMap<PathBuf, (i64, Option<Value>)>,
    // session_index.jsonl 路径 -> (mtime_ms, sessionId -> workDir)
    index_cache: HashMap<PathBuf, (i64, HashMap<String, String>)>,
    // 800ms 限流：缓存的 /api/data 响应
    last_scan: Option<(Instant, String)>,
    default_probe: Option<(bool, Option<Value>)>,
    agent_cache: HashMap<(String, PathBuf), (i64, u64, Vec<(String, String, Session, Rec)>)>,
    scan_warnings: Vec<Value>,
    history: std::collections::BTreeMap<String, HistoryEntry>,
    history_loaded: bool,
    history_dirty: bool,
    history_error: String,
}

fn merge_duplicate(records: &mut [Rec], ids: &mut HashMap<String,usize>, agent: &str, event: &str, rec: &Rec) -> bool {
    let key=format!("{agent}:{event}");
    if let Some(i)=ids.get(&key) {
        let old=&mut records[*i];
        if agent=="dsh" {if rec.t>=old.t {let si=old.s;*old=rec.clone();old.s=si;}}
        else {
            old.i=old.i.max(rec.i);old.o=old.o.max(rec.o);old.r=old.r.max(rec.r);old.c=old.c.max(rec.c);
            if if agent=="codex" {rec.t<old.t} else {rec.t>=old.t} {old.t=rec.t;old.m=rec.m.clone();old.a=rec.a.clone();}
            old.inclusive |= rec.inclusive;
        }
        true
    } else {ids.insert(key,records.len());false}
}

fn history_path(value: &str) -> String {
    let path=Path::new(value);
    let absolute=if path.is_absolute() {path.to_path_buf()} else {std::env::current_dir().unwrap_or_default().join(path)};
    let mut clean=PathBuf::new();
    for component in absolute.components() {
        match component {std::path::Component::CurDir=>{},std::path::Component::ParentDir=>{clean.pop();},other=>clean.push(other.as_os_str())}
    }
    // Windows TEMP 可能使用 RUNNER~1 等短路径；缺失文件也要规范化现存父目录。
    let mut ancestor=clean.as_path();
    let mut missing=Vec::new();
    let resolved=loop {
        if let Ok(mut canonical)=std::fs::canonicalize(ancestor) {
            for name in missing.iter().rev() {canonical.push(name);}
            break canonical;
        }
        let (Some(name),Some(parent))=(ancestor.file_name(),ancestor.parent()) else {break clean.clone();};
        missing.push(name.to_os_string());ancestor=parent;
    };
    let value=resolved.to_string_lossy().replace('\\',"/");
    let value=if let Some(unc)=value.strip_prefix("//?/UNC/") {format!("//{unc}")} else {value.trim_start_matches("//?/").to_string()};
    if cfg!(windows) {value.to_lowercase()} else {value}
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct HistoryEntry { session: Session, record: Rec }
#[derive(serde::Serialize, serde::Deserialize)]
struct HistoryFile { version: u32, entries: Vec<(String, HistoryEntry)> }

impl Engine {
    fn merge_history(&mut self, sessions: &mut Vec<Session>, records: &mut Vec<Rec>) {
        let file = self.config_dir.join("usage-history.json");
        if !self.history_loaded {
            self.history_loaded = true;
            for candidate in [file.clone(), file.with_extension("json.bak")] {
                match std::fs::read(&candidate) {
                    Ok(bytes) => match serde_json::from_slice::<HistoryFile>(&bytes) {
                        Ok(data) if data.version==1 => {self.history=data.entries.into_iter().collect();self.history_error.clear();self.history_dirty=candidate!=file;break;},
                        _ => self.history_error="用量档案格式无效".into(),
                    },
                    Err(e) if e.kind()==std::io::ErrorKind::NotFound => {},
                    Err(e) => self.history_error=e.to_string(),
                }
            }
        }
        if !self.history_error.is_empty() {self.scan_warnings.push(json!({"agent":"history","error":format!("用量档案读取失败：{}",self.history_error)}));}
        let mut paths:HashMap<String,String>=HashMap::new();
        let mut changed=false;
        for rec in records.iter() {
            let mut session=sessions[rec.s].clone();
            let mut record=rec.clone();record.s=0;record.origin=paths.entry(rec.origin.clone()).or_insert_with(||history_path(&rec.origin)).clone();
            let key=json!([rec.tool,session.id,if rec.tool=="kimi" {record.origin.as_str()} else {""},rec.key]).to_string();
            if let Some(old)=self.history.get(&key) {
                if session.title.is_empty() {session.title=old.session.title.clone();}
                if session.cwd.is_empty() {session.cwd=old.session.cwd.clone();}
                if rec.tool!="dsh" {
                    record.i=record.i.max(old.record.i);record.o=record.o.max(old.record.o);record.r=record.r.max(old.record.r);record.c=record.c.max(old.record.c);
                    record.inclusive |= old.record.inclusive;
                    if rec.tool=="codex" {record.t=record.t.min(old.record.t);}
                }
            }
            let value=HistoryEntry{session,record};
            if self.history.get(&key).map(|v|serde_json::to_value(v).ok()) != Some(serde_json::to_value(&value).ok()) {
                self.history.insert(key,value);changed=true;
            }
        }
        if changed {self.history_dirty=true;}
        if self.history_dirty && self.history_error.is_empty() {
            let result=(|| -> std::io::Result<()> {
                use std::io::Write;
                std::fs::create_dir_all(&self.config_dir)?;
                let tmp=file.with_extension("json.tmp");
                let mut output=std::fs::File::create(&tmp)?;
                serde_json::to_writer(&mut output,&HistoryFile{version:1,entries:self.history.iter().map(|(k,v)|(k.clone(),v.clone())).collect()})?;
                output.flush()?;output.sync_all()?;drop(output);
                if std::fs::read(&file).ok().and_then(|b|serde_json::from_slice::<HistoryFile>(&b).ok()).map(|h|h.version==1).unwrap_or(false) {std::fs::copy(&file,file.with_extension("json.bak"))?;}
                std::fs::rename(tmp,&file)?;
                Ok(())
            })();
            match result {Ok(())=>self.history_dirty=false,Err(e)=>self.scan_warnings.push(json!({"agent":"history","error":format!("用量档案保存失败：{e}")}))}
        }
        let mut enabled:HashMap<String,Vec<String>>=HashMap::new();
        enabled.insert("kimi".into(),self.cfg.roots.iter().map(|r|history_path(r)).collect());
        for monitor in self.cfg.monitors.clone().unwrap_or_default() {
            if monitor["enabled"]!=true {continue;}
            if let (Some(agent),Some(root))=(monitor["agent"].as_str(),monitor["root"].as_str().filter(|r|!r.is_empty())) {
                enabled.entry(agent.into()).or_default().push(history_path(root));
            }
        }
        records.clear();
        for entry in self.history.values() {
            let rec=&entry.record;
            let origin=paths.entry(rec.origin.clone()).or_insert_with(||history_path(&rec.origin));
            if !enabled.get(&rec.tool).map(|roots|roots.iter().any(|root|{let root=root.trim_end_matches('/');origin==root || origin.starts_with(&(root.to_string()+"/"))})).unwrap_or(false) {continue;}
            let si=sessions.iter().position(|s|s.tool==entry.session.tool && s.id==entry.session.id).unwrap_or_else(||{sessions.push(entry.session.clone());sessions.len()-1});
            if sessions[si].title.is_empty() {sessions[si].title=entry.session.title.clone();}
            if sessions[si].cwd.is_empty() {sessions[si].cwd=entry.session.cwd.clone();}
            let mut rec=rec.clone();rec.s=si;records.push(rec);
        }
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn token_count(v: &Value) -> i64 {
    v.as_i64().filter(|n| *n >= 0 && *n <= 9_007_199_254_740_991).unwrap_or(0)
}

fn valid_config(body: &Value) -> bool {
    (body.get("roots").is_some() || body.get("monitors").is_some())
        && body.get("roots").map(|v| v.as_array().map(|a| a.iter().all(Value::is_string)).unwrap_or(false)).unwrap_or(true)
        && body.get("monitors").map(agents::valid_monitors).unwrap_or(true)
        && body.get("models").map(|v| v.as_array().map(|a| a.iter().all(Value::is_string)).unwrap_or(false)).unwrap_or(true)
        && body.get("setupDone").map(Value::is_boolean).unwrap_or(true)
        && body.get("closeAction").map(|v| matches!(v.as_str(), Some("exit") | Some("background") | Some("ask"))).unwrap_or(true)
}

fn file_mtime(p: &Path) -> Option<i64> {
    std::fs::metadata(p)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64)
}

// state.json 里 createdAt/updatedAt 的解析：数值直传，字符串按 RFC3339 / 本地时间兜底
fn to_epoch(v: &Value) -> i64 {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        Value::String(s) => parse_iso(s),
        _ => 0,
    }
}

fn parse_iso(s: &str) -> i64 {
    let t = s.trim();
    use chrono::TimeZone;
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(t) {
        return dt.timestamp_millis();
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f", "%Y/%m/%d %H:%M:%S%.f"] {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(t, fmt) {
            return chrono::Local
                .from_local_datetime(&naive)
                .single()
                .map(|d| d.timestamp_millis())
                .unwrap_or(0);
        }
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(t, "%Y-%m-%d") {
        let naive = d.and_hms_opt(0, 0, 0).unwrap();
        return chrono::Local
            .from_local_datetime(&naive)
            .single()
            .map(|d| d.timestamp_millis())
            .unwrap_or(0);
    }
    0
}

impl Engine {
    fn new(config_dir: PathBuf) -> Self {
        let mut e = Engine::default();
        e.config_dir = config_dir;
        e.load_config();
        e
    }

    fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.json")
    }

    fn default_root() -> String {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        Path::new(&home).join(".kimi-code").join("sessions").to_string_lossy().to_string()
    }

    fn load_config(&mut self) {
        self.cfg = Config::default();
        self.cfg.close_action = String::from("background");
        if let Some(text) = [self.config_file(),self.config_dir.join("config.json.bak")].iter().find_map(|p| {
            std::fs::read_to_string(p).ok().filter(|s|serde_json::from_str::<Value>(s).is_ok())
        }) {
            if let Ok(j) = serde_json::from_str::<Value>(&text) {
                if let Some(a) = j.get("roots").and_then(|v| v.as_array()) {
                    self.cfg.roots = a.iter().filter_map(|x| x.as_str().map(String::from)).collect();
                }
                if agents::valid_monitors(&j["monitors"]) { self.cfg.monitors = Some(agents::normalize(j["monitors"].as_array().unwrap())); }
                if let Some(a) = j["models"].as_array() { self.cfg.models = a.iter().filter_map(|v| v.as_str().map(String::from)).collect(); }
                if let Some(a) = j.get("closeAction").and_then(|v| v.as_str()) { self.cfg.close_action = a.into(); }
                if let Some(b) = j.get("setupDone").and_then(|v| v.as_bool()) {
                    self.cfg.setup_done = b;
                } else if !self.cfg.roots.is_empty() {
                    self.cfg.setup_done = true; // 旧版配置已选过目录，视为完成引导
                }
            }
        }
        if self.cfg.roots.is_empty() && self.cfg.monitors.is_none() && !self.cfg.setup_done {
            let d = Self::default_root();
            if Path::new(&d).exists() {
                self.cfg.roots.push(d); // 首次运行自动使用本机默认目录
            }
        }
        self.cfg.roots = self.cfg.roots.iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
        self.cfg.roots.dedup();
    }

    fn save_config(&self) -> std::io::Result<()> {
        let body = json!({"roots": self.cfg.roots, "setupDone": self.cfg.setup_done,"monitors":self.cfg.monitors,"models":self.cfg.models,"closeAction":self.close_action()});
        std::fs::create_dir_all(&self.config_dir)?;
        let temp=self.config_dir.join("config.json.tmp");
        let result=(|| {
            let mut f=std::fs::File::create(&temp)?;
            f.write_all(serde_json::to_string_pretty(&body)?.as_bytes())?;f.sync_all()?;drop(f);
            if let Ok(old)=std::fs::read_to_string(self.config_file()) {
                if serde_json::from_str::<Value>(&old).is_ok() {
                    let backup_temp=self.config_dir.join("config.json.bak.tmp");
                    let mut backup=std::fs::File::create(&backup_temp)?;
                    backup.write_all(old.as_bytes())?;backup.sync_all()?;drop(backup);
                    std::fs::rename(backup_temp,self.config_dir.join("config.json.bak"))?;
                }
            }
            std::fs::rename(&temp,self.config_file())
        })();
        if result.is_err() {let _=std::fs::remove_file(temp);}
        result
    }

    // session_index.jsonl: sessionId -> workDir（state.json 缺 cwd 时的兜底）；
    // 可能在数据目录本身或其上一级（.kimi-code 目录）
    fn load_index_for(&mut self, root: &Path) -> HashMap<String, String> {
        let candidates = [root.join("session_index.jsonl"), root.parent().unwrap_or(root).join("session_index.jsonl")];
        for file in candidates {
            let Some(mtime) = file_mtime(&file) else { continue };
            if let Some((m, map)) = self.index_cache.get(&file) {
                if *m == mtime {
                    return map.clone();
                }
            }
            let mut map = HashMap::new();
            if let Ok(text) = std::fs::read_to_string(&file) {
                for line in text.lines() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Ok(j) = serde_json::from_str::<Value>(line) {
                        if let Some(id) = j.get("sessionId").and_then(|v| v.as_str()) {
                            let wd = j.get("workDir").and_then(|v| v.as_str()).unwrap_or("");
                            map.insert(id.to_string(), wd.to_string());
                        }
                    }
                }
            }
            self.index_cache.insert(file, (mtime, map.clone()));
            return map;
        }
        HashMap::new()
    }

    fn parse_wire_file(file: &Path) -> Vec<Rec> {
        let Ok(text) = std::fs::read_to_string(file) else { return Vec::new() };
        let mut out = Vec::new();
        for (line_index, line) in text.lines().enumerate() {
            if !line.contains("usage.record") {
                continue;
            }
            let Ok(ev) = serde_json::from_str::<Value>(line) else { continue };
            if ev.get("type").and_then(|v| v.as_str()) != Some("usage.record") {
                continue;
            }
            let Some(u) = ev.get("usage") else { continue };
            out.push(Rec {
                key: line_index.to_string(), origin: file.to_string_lossy().to_string(),
                s: 0, // 扫描会话时设置索引，缓存记录不绑定会话顺序
                t: ev.get("time").and_then(|v| v.as_i64()).unwrap_or(0),
                m: ev.get("model").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
                a: ev.get("agentId").and_then(|v| v.as_str()).unwrap_or("main").to_string(),
                tool: "kimi".to_string(),
                i: token_count(&u["inputOther"]),
                o: token_count(&u["output"]),
                r: token_count(&u["inputCacheRead"]),
                c: token_count(&u["inputCacheCreation"]),
                inclusive: false,
            });
        }
        out
    }

    fn read_state(&mut self, sess_dir: &Path) -> Option<Value> {
        let file = sess_dir.join("state.json");
        let mtime = file_mtime(&file)?;
        if let Some((m, state)) = self.state_cache.get(sess_dir) {
            if *m == mtime {
                return state.clone();
            }
        }
        let state = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
        self.state_cache.insert(sess_dir.to_path_buf(), (mtime, state.clone()));
        state
    }

    // 扫描含 agents/ 的会话；state.json 可缺省，项目目录由可找到的索引兜底
    fn scan_session(&mut self, sess_path: &Path, sess_name: &str, index: &HashMap<String, String>, sessions: &mut Vec<Session>, records: &mut Vec<Rec>, seen: &mut HashMap<PathBuf, ()>, seen_sessions: &mut HashSet<PathBuf>) {
        let Ok(canonical) = std::fs::canonicalize(sess_path) else { return };
        let key = if cfg!(windows) { PathBuf::from(canonical.to_string_lossy().to_lowercase()) } else { canonical.clone() };
        if !seen_sessions.insert(key) { return; }
        let sess_path = canonical.as_path();
        let state = self.read_state(sess_path);
        let sess_idx = sessions.len();
        sessions.push(Session {
            id: sess_name.to_string(),
            tool: "kimi".to_string(),
            cwd: state.as_ref()
                .and_then(|s| s.get("cwd").and_then(|v| v.as_str()).map(String::from))
                .or_else(|| index.get(sess_name).cloned())
                .unwrap_or_default(),
            title: state.as_ref().and_then(|s| s.get("title").and_then(|v| v.as_str()).map(String::from)).unwrap_or_default(),
            created_at: state.as_ref().map(|s| to_epoch(s.get("createdAt").unwrap_or(&Value::Null))).unwrap_or(0),
            updated_at: state.as_ref().map(|s| to_epoch(s.get("updatedAt").unwrap_or(&Value::Null))).unwrap_or(0),
        });
        let agents_dir = sess_path.join("agents");
        let Ok(entries) = std::fs::read_dir(&agents_dir) else { return };
        for ag in entries.flatten() {
            if !ag.path().is_dir() {
                continue;
            }
            let wire = ag.path().join("wire.jsonl");
            let Some(mtime) = file_mtime(&wire) else { continue };
            let size = std::fs::metadata(&wire).map(|m| m.len()).unwrap_or(0);
            seen.insert(wire.clone(), ());
            let cached = self.wire_cache.get(&wire).map(|(m, s, recs)| (*m == mtime && *s == size).then(|| recs.clone())).flatten();
            let recs = match cached {
                Some(r) => r,
                None => {
                    let r = Self::parse_wire_file(&wire);
                    self.wire_cache.insert(wire, (mtime, size, r.clone()));
                    r
                }
            };
            records.extend(recs.into_iter().map(|mut r| { r.s = sess_idx; r }));
        }
    }

    // 扫描一个数据目录。兼容两种布局：
    //   root\<工作区>\<会话>\...   （.kimi-code\sessions 原生结构）
    //   root\<会话>\...            （直接指向某个 sessions 子目录或备份）
    fn scan_root(&mut self, root: &Path, sessions: &mut Vec<Session>, records: &mut Vec<Rec>, seen: &mut HashMap<PathBuf, ()>, seen_sessions: &mut HashSet<PathBuf>) {
        let index = self.load_index_for(root);
        let Ok(entries) = std::fs::read_dir(root) else { return };
        for d in entries.flatten() {
            let p1 = d.path();
            if !p1.is_dir() {
                continue;
            }
            if p1.join("state.json").exists() || p1.join("agents").is_dir() {
                let name = d.file_name().to_string_lossy().to_string();
                self.scan_session(&p1, &name, &index, sessions, records, seen, seen_sessions);
                continue;
            }
            let Ok(subs) = std::fs::read_dir(&p1) else { continue };
            for d2 in subs.flatten() {
                let p2 = d2.path();
                if !p2.is_dir() || !(p2.join("state.json").exists() || p2.join("agents").is_dir()) {
                    continue;
                }
                let name = d2.file_name().to_string_lossy().to_string();
                self.scan_session(&p2, &name, &index, sessions, records, seen, seen_sessions);
            }
        }
    }

    fn scan(&mut self) -> (Vec<Session>, Vec<Rec>) {
        self.scan_warnings.clear();
        let mut seen: HashMap<PathBuf, ()> = HashMap::new();
        let mut seen_sessions = HashSet::new();
        let mut sessions = Vec::new();
        let mut records = Vec::new();
        for root in self.cfg.roots.clone() {
            self.scan_root(Path::new(&root), &mut sessions, &mut records, &mut seen, &mut seen_sessions);
        }
        let mut event_ids: HashMap<String,usize> = HashMap::new();
        let mut agent_seen = HashSet::new();
        for m in self.cfg.monitors.clone().unwrap_or_default() {
            if m["enabled"] != true { continue; }
            let agent = m["agent"].as_str().unwrap_or_default();
            let root = Path::new(m["root"].as_str().unwrap_or_default());
            if agent == "kimi" {
                self.scan_root(root, &mut sessions, &mut records, &mut seen, &mut seen_sessions);
                continue;
            }
            let database=root.join("db.sqlite");
            if agent=="opencode" || agent=="zcode" && database.exists() {
                let parsed=if agent=="opencode" {agents::parse_opencode(root,&mut self.scan_warnings)} else {agents::parse_zcode_database(&database)};
                match parsed {
                    Ok(data)=>for (sid,event,session,mut rec) in data {
                        if merge_duplicate(&mut records, &mut event_ids, agent, &event, &rec) {continue;}
                        let si=sessions.iter().position(|s|s.tool==agent && s.id==sid).unwrap_or_else(||{sessions.push(session);sessions.len()-1});
                        rec.s=si;records.push(rec);
                    },
                    Err(e)=>self.scan_warnings.push(json!({"agent":agent,"root":root,"error":e.to_string()}))
                }
                continue;
            }
            if let Err(e) = std::fs::read_dir(root) { self.scan_warnings.push(json!({"agent":agent,"root":root,"error":e.to_string()})); continue; }
            let (files,errors) = agents::files_with_warnings(root);
            for (file,error) in errors {self.scan_warnings.push(json!({"agent":agent,"root":root,"file":file,"error":error}));}
            let files = if agent == "dsh" {agents::select_dsh_files(files)} else {files};
            for file in files {
                let key = (agent.to_string(),file.clone());
                agent_seen.insert(key.clone());
                let Some(mtime) = file_mtime(&file) else { continue; };
                let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
                let data = match self.agent_cache.get(&key) {
                    Some((m,s,data)) if *m == mtime && *s == size => data.clone(),
                    _ => { let data = match agents::parse(&file,agent) {
                        Ok(data)=>data,
                        Err(e)=>{self.scan_warnings.push(json!({"agent":agent,"root":root,"file":file,"error":e.to_string()}));continue;}
                    }; self.agent_cache.insert(key,(mtime,size,data.clone())); data }
                };
                for (sid,event,session,mut rec) in data {
                    if !session.title.is_empty() {if let Some(old)=sessions.iter_mut().find(|s|s.tool==agent && s.id==sid) {if old.title.is_empty() || old.title.starts_with("首条提问：") {old.title=session.title.clone();}}}
                    if merge_duplicate(&mut records, &mut event_ids, agent, &event, &rec) { continue; }
                    let si = sessions.iter().position(|s| s.tool == agent && s.id == sid).unwrap_or_else(|| {sessions.push(session);sessions.len()-1});
                    rec.s = si;records.push(rec);
                }
            }
        }
        self.agent_cache.retain(|k,_| agent_seen.contains(k));
        self.wire_cache.retain(|k, _| seen.contains_key(k));
        agents::enrich_titles(&mut sessions,&self.cfg.monitors.clone().unwrap_or_default(),&mut self.scan_warnings);
        self.merge_history(&mut sessions, &mut records);
        records.sort_by_key(|r|r.t);
        let mut previous: HashMap<usize,[i64;4]> = HashMap::new();
        for r in &mut records {
            if r.tool == "codex" {
                let counts=[r.i,r.o,r.r,r.c];
                let prev=previous.entry(r.s).or_insert([0;4]);
                r.i=(counts[0]-prev[0]).max(0);r.o=(counts[1]-prev[1]).max(0);r.r=(counts[2]-prev[2]).max(0);r.c=(counts[3]-prev[3]).max(0);
                for i in 0..4 {prev[i]=prev[i].max(counts[i]);}
            }
            if r.inclusive {r.i=(r.i-r.r-r.c).max(0);}
        }
        records.retain(|r|r.tool!="codex" || r.i+r.o+r.r+r.c>0);
        for session in &mut sessions {if session.tool!="kimi" {session.created_at=i64::MAX;session.updated_at=0;}}
        for r in &records {let s=&mut sessions[r.s];if s.tool=="kimi" {continue;}s.created_at=s.created_at.min(r.t);s.updated_at=s.updated_at.max(r.t);}
        for s in &mut sessions {if s.created_at==i64::MAX {s.created_at=0;}}
        (sessions, records)
    }

    fn probe_root(&mut self, root: &str) -> Value {
        let mut sessions = Vec::new();
        let mut records = Vec::new();
        let mut seen = HashMap::new();
        let mut seen_sessions = HashSet::new();
        let ok = Path::new(root).exists();
        if ok {
            self.scan_root(Path::new(root), &mut sessions, &mut records, &mut seen, &mut seen_sessions);
        }
        json!({"path": root, "ok": ok, "sessions": sessions.len(), "records": records.len()})
    }

    fn get_data_string(&mut self) -> String {
        if let Some((at, body)) = &self.last_scan {
            if at.elapsed() < Duration::from_millis(800) {
                return body.clone();
            }
        }
        let (sessions, records) = self.scan();
        let sessions_json: Vec<Value> = sessions
            .iter()
            .map(|s| json!({"id": s.id, "cwd": s.cwd, "title": s.title,"tool":s.tool, "createdAt": s.created_at, "updatedAt": s.updated_at}))
            .collect();
        let mut available_models: Vec<String> = records.iter().map(|r| r.m.clone()).collect();
        available_models.sort();available_models.dedup();
        let records_json: Vec<Value> = records
            .iter()
            .filter(|r| self.cfg.models.is_empty() || self.cfg.models.contains(&r.m))
            .map(|r| json!({"s": r.s, "t": r.t, "m": r.m, "a": r.a,"tool":r.tool, "i": r.i, "o": r.o, "r": r.r, "c": r.c}))
            .collect();
        // 引导期对默认目录做一次探测（与 server.js 一致）
        let default_root = Self::default_root();
        if self.default_probe.is_none() {
            let ok = Path::new(&default_root).exists();
            let stats = if ok && !self.cfg.setup_done { Some(self.probe_root(&default_root)) } else { None };
            self.default_probe = Some((ok, stats));
        }
        let (ok, default_stats) = self.default_probe.as_ref().unwrap();
        let body = json!({
            "generatedAt": now_ms(),
            "roots": self.cfg.roots,
            "setupDone": self.cfg.setup_done,
            "defaultRoot": default_root,
            "defaultOk": ok,
            "defaultStats": default_stats,
            "sessions": sessions_json,
            "records": records_json,
            "agents":agents::catalog(),"monitors":self.monitors_json(),"models":self.cfg.models,"availableModels":available_models,"desktop":true,"closeAction":self.close_action(),
            "warnings":self.scan_warnings,
        });
        let s = body.to_string();
        self.last_scan = Some((Instant::now(), s.clone()));
        s
    }

    fn apply_config(&mut self, body: &Value) -> String {
        if let Some(monitors) = body["monitors"].as_array() {
            for m in monitors {
                if m["enabled"] == true {
                    let root = m["root"].as_str().unwrap_or_default().trim();
                    if root.is_empty() || !Path::new(root).is_dir() || std::fs::read_dir(root).is_err() {
                        return json!({"error":format!("已启用的监测目录不存在或无法读取：{root}")}).to_string();
                    }
                }
            }
        }
        let previous = self.cfg.clone();
        let mut roots: Vec<String> = body
            .get("roots")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        roots = roots.iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
        roots.dedup();
        let stats: Vec<Value> = roots.iter().map(|r| self.probe_root(r)).collect();
        let ok_roots: Vec<String> = roots
            .iter()
            .zip(stats.iter())
            .filter(|(_, s)| s.get("ok").and_then(|v| v.as_bool()).unwrap_or(false))
            .map(|(r, _)| r.clone())
            .collect();
        self.cfg.roots = ok_roots;
        self.cfg.monitors = body["monitors"].as_array().map(|v| agents::normalize(v));
        if let Some(models) = body["models"].as_array() {
            self.cfg.models = models.iter().filter_map(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
            self.cfg.models.sort();self.cfg.models.dedup();
        }
        if let Some(a) = body.get("closeAction").and_then(|v| v.as_str()) { self.cfg.close_action = a.into(); }
        match body.get("setupDone").and_then(|v| v.as_bool()) {
            Some(b) => self.cfg.setup_done = b,
            None => self.cfg.setup_done = self.cfg.setup_done || !self.cfg.roots.is_empty(),
        }
        if self.save_config().is_err() { self.cfg = previous; return json!({"error":"配置保存失败，请检查配置目录权限"}).to_string(); }
        self.wire_cache.clear();
        self.agent_cache.clear();
        self.state_cache.clear();
        self.index_cache.clear();
        self.last_scan = None;
        self.default_probe = None;
        json!({"ok": true, "roots": self.cfg.roots, "setupDone": self.cfg.setup_done, "stats": stats,"agents":agents::catalog(),"monitors":self.monitors_json(),"models":self.cfg.models}).to_string()
    }

    fn monitors_json(&self) -> Vec<Value> {
        let rows = self.cfg.monitors.clone().unwrap_or_else(|| self.cfg.roots.iter().map(|root|json!({"agent":"kimi","root":root,"enabled":true})).collect());
        rows.into_iter().map(|mut m| {m["exists"] = json!(Path::new(m["root"].as_str().unwrap_or_default()).is_dir());m}).collect()
    }

    // 关窗行为：未知或空值一律回退为后台运行
    fn close_action(&self) -> &str {
        match self.cfg.close_action.as_str() { "exit" | "ask" => &self.cfg.close_action, _ => "background" }
    }
}


// ---------- 内嵌 HTTP 服务 ----------
fn static_asset(p: &str) -> Option<(&'static [u8], &'static str)> {
    match p {
        "/" | "/index.html" => Some((INDEX_HTML, "text/html; charset=utf-8")),
        "/icon.png" => Some((ICON_PNG, "image/png")),
        "/favicon.ico" => Some((FAVICON_ICO, "image/x-icon")),
        "/manifest.webmanifest" => Some((MANIFEST_WEBMANIFEST, "application/manifest+json")),
        "/icon-192.png" => Some((ICON_192, "image/png")),
        "/icon-512.png" => Some((ICON_512, "image/png")),
        _ => None,
    }
}

fn read_request(stream: &mut TcpStream) -> Result<(String, String, Vec<u8>), &'static str> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    // 读完请求头
    let header_end = loop {
        match stream.read(&mut tmp) {
            Ok(0) => return Err("400 Bad Request"),
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                    if pos+4>1024*1024 {return Err("413 Payload Too Large");}
                    break pos + 4;
                }
                if buf.len() > 1024 * 1024 {
                    return Err("413 Payload Too Large");
                }
            }
            Err(_) => return Err("400 Bad Request"),
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().ok_or("400 Bad Request")?.to_string();
    let mut content_length = 0usize;
    for l in lines {
        let lower = l.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse().map_err(|_|"400 Bad Request")?;
            if content_length>1024*1024 {return Err("413 Payload Too Large");}
        }
    }
    let mut body = buf[header_end..].to_vec();
    if body.len()>1024*1024 {return Err("413 Payload Too Large");}
    while body.len() < content_length {
        match stream.read(&mut tmp) {
            Ok(0) => return Err("400 Bad Request"),
            Ok(n) => {if body.len()+n>1024*1024 {return Err("413 Payload Too Large");}body.extend_from_slice(&tmp[..n]);},
            Err(_) => return Err("400 Bad Request"),
        }
    }
    body.truncate(content_length);
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("400 Bad Request")?.to_string();
    let path_full = parts.next().ok_or("400 Bad Request")?.to_string();
    let path = path_full.split('?').next().unwrap_or("/").to_string();
    Ok((method, path, body))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn respond(stream: &mut TcpStream, status: &str, ctype: &str, body: Vec<u8>) -> std::io::Result<()> {
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", body.len());
    stream.write_all(head.as_bytes())?;
    stream.write_all(&body)?;
    stream.flush()
}

fn http_server(engine: std::sync::Arc<std::sync::Mutex<Engine>>, app: AppHandle, port_tx: std::sync::mpsc::Sender<u16>) {
    let listener = match TcpListener::bind(("127.0.0.1", PORT)) {
        Ok(l) => l,
        Err(_) => match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(l) => l, // 优先端口被占用时回退随机端口（前端同源相对路径，任意端口都可用）
            Err(_) => return,
        },
    };
    let _ = port_tx.send(listener.local_addr().map(|a| a.port()).unwrap_or(PORT));
    for stream in listener.incoming().flatten() {
        let engine = engine.clone();
        let app = app.clone();
        std::thread::spawn(move || {
            let _ = handle_conn(stream, engine, app);
        });
    }
}

fn handle_conn(mut stream: TcpStream, engine: std::sync::Arc<std::sync::Mutex<Engine>>, app: AppHandle) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let (method,path,body)=match read_request(&mut stream) {
        Ok(request)=>request,
        Err(status)=>return respond(&mut stream,status,"application/json; charset=utf-8",json!({"error":if status.starts_with("413") {"请求超过 1 MiB 限制"} else {"请求不完整或格式错误"}}).to_string().into_bytes())
    };

    if path == "/api/data" && method == "GET" {
        let s = engine.lock().map(|mut e| e.get_data_string()).unwrap_or_else(|_| "{\"error\":\"监控数据暂不可用\"}".into());
        return respond(&mut stream, "200 OK", "application/json; charset=utf-8", s.into_bytes());
    }
    if path == "/api/config" && method == "POST" {
        let j: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        if !valid_config(&j) {
            return respond(&mut stream, "400 Bad Request", "application/json; charset=utf-8", json!({"error":"配置需要 roots 字符串数组及可选的 setupDone 布尔值"}).to_string().into_bytes());
        }
        let s = engine.lock().map(|mut e| e.apply_config(&j)).unwrap_or_else(|_| "{\"error\":\"配置暂时无法保存\"}".into());
        let status = if serde_json::from_str::<Value>(&s).ok().and_then(|v|v.get("error").cloned()).is_some() { "400 Bad Request" } else { "200 OK" };
        return respond(&mut stream, status, "application/json; charset=utf-8", s.into_bytes());
    }
    if path == "/api/open" && method == "POST" {
        show_main(&app);
        return respond(&mut stream, "200 OK", "text/plain; charset=utf-8", b"ok".to_vec());
    }
    if path == "/api/quit" && method == "POST" {
        respond(&mut stream, "200 OK", "text/plain; charset=utf-8", b"bye".to_vec())?;
        let app2 = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            app2.exit(0);
        });
        return Ok(());
    }
    // 关窗询问弹窗的落点：exit=彻底退出，hide=隐藏到托盘
    if path == "/api/app/close" && method == "POST" {
        let j: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        match j.get("action").and_then(|v| v.as_str()) {
            Some("exit") => {
                respond(&mut stream, "200 OK", "text/plain; charset=utf-8", b"bye".to_vec())?;
                let app2 = app.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(150));
                    app2.exit(0);
                });
            }
            Some("hide") => {
                if let Some(w) = app.get_webview_window("main") { let _ = w.hide(); }
                respond(&mut stream, "200 OK", "text/plain; charset=utf-8", b"ok".to_vec())?;
            }
            _ => return respond(&mut stream, "400 Bad Request", "application/json; charset=utf-8", json!({"error":"action 需为 exit 或 hide"}).to_string().into_bytes()),
        }
        return Ok(());
    }
    if method == "GET" {
        if let Some((bytes, ctype)) = static_asset(&path) {
            return respond(&mut stream, "200 OK", ctype, bytes.to_vec());
        }
    }
    respond(&mut stream, "404 Not Found", "text/plain; charset=utf-8", b"404 Not Found".to_vec())
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn migrate_legacy_config(target: &Path, legacy: &Path) -> std::io::Result<()> {
    if target.join("config.json").exists() || target.join("config.json.bak").exists() { return Ok(()); }
    for name in ["config.json", "config.json.bak"] {
        let Ok(content) = std::fs::read_to_string(legacy.join(name)) else { continue; };
        if !serde_json::from_str::<Value>(&content).is_ok_and(|v|v.is_object()) { continue; }
        let file = std::fs::OpenOptions::new().write(true).create_new(true).open(target.join("config.json"));
        match file {
            Ok(mut file) => { file.write_all(content.as_bytes())?; file.sync_all()?; return Ok(()); }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

// 配置目录固定为 AgentMonitor，不跟随应用标识生成 com.yfan945.* 目录。
fn renamed_config_dir(app_config_dir: Option<PathBuf>) -> PathBuf {
    app_config_dir
        .and_then(|dir| dir.parent().map(|parent| parent.join("AgentMonitor")))
        .unwrap_or_else(|| PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join("AgentMonitor"))
}

fn main() {
    let engine = std::sync::Arc::new(std::sync::Mutex::new(Engine::new(PathBuf::from(""))));
    let close_engine = engine.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .setup(move |app| {
            let config_dir = renamed_config_dir(app.path().app_config_dir().ok());
            let _ = std::fs::create_dir_all(&config_dir);
            if let Some(parent) = config_dir.parent() {
                migrate_legacy_config(&config_dir, &parent.join("com.yfan945.agentmonitor"))?;
                migrate_legacy_config(&config_dir, &parent.join("com.yfan945.kimimonitor"))?;
            }
            {
                let mut e = engine.lock().unwrap();
                e.config_dir = config_dir;
                e.load_config();
            }

            let collector = engine.clone();
            std::thread::spawn(move || loop {
                if let Ok(mut e) = collector.lock() {e.get_data_string();}
                std::thread::sleep(Duration::from_secs(30));
            });

            // 启动内嵌 HTTP 服务并等待端口就绪
            let (tx, rx) = std::sync::mpsc::channel::<u16>();
            let app_handle = app.handle().clone();
            let e2 = engine.clone();
            std::thread::spawn(move || http_server(e2, app_handle, tx));
            let port = rx.recv_timeout(Duration::from_secs(5)).unwrap_or(PORT);

            // 主窗口：指向内嵌服务（同源相对路径，前端零改动）
            let url = format!("http://127.0.0.1:{port}/");
            let win = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse()?))
                .title(APP_TITLE)
                .inner_size(1280.0, 900.0)
                .min_inner_size(980.0, 640.0)
                .visible(false)
                .build()?;
            let _ = win.show();
            let _ = win.set_focus();

            // 托盘：打开窗口 / 退出
            use tauri::menu::{MenuBuilder, MenuItem};
            let open_item = MenuItem::with_id(app, "open", "打开窗口", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = MenuBuilder::new(app).item(&open_item).item(&quit_item).build()?;
            let _tray = tauri::tray::TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("AgentMonitor")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, ev| match ev.id().as_ref() {
                    "open" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, ev| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = ev
                    {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(move |window, event| {
            // 关窗行为跟随设置中的“关闭应用时”：直接退出 / 隐藏到后台（默认）/ 每次询问
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let mode = close_engine.lock().map(|e| e.close_action().to_string()).unwrap_or_default();
                match mode.as_str() {
                    "exit" => {} // 允许窗口关闭，应用随之退出
                    "ask" => {
                        api.prevent_close();
                        show_main(window.app_handle());
                        if let Some(w) = window.get_webview_window("main") {
                            let _ = w.eval("if(window.__closeAsk)window.__closeAsk()");
                        }
                    }
                    _ => { api.prevent_close(); let _ = window.hide(); } // 后台运行：托盘常驻
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("启动 AgentMonitor Rust/Tauri 桌面版失败");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_paths_resolve_aliases_and_missing_descendants() {
        let f=Fixture::new();
        let canonical=std::fs::canonicalize(&f.0).unwrap();
        assert_eq!(history_path(&f.0.to_string_lossy()),history_path(&canonical.to_string_lossy()));
        let missing=f.0.join("removed").join("wire.jsonl");
        let expected=canonical.join("removed").join("wire.jsonl");
        assert_eq!(history_path(&missing.to_string_lossy()),history_path(&expected.to_string_lossy()));
        let dotted=f.0.join(".").join("child").join("..").join("missing");
        assert_eq!(history_path(&dotted.to_string_lossy()),history_path(&canonical.join("missing").to_string_lossy()));
    }

    #[test]
    fn opencode_history_retains_deleted_sources_across_restart_and_filters() {
        let f=Fixture::new();
        let oc=f.0.join("oc");std::fs::create_dir_all(&oc).unwrap();let file=oc.join("opencode.db");
        let db=rusqlite::Connection::open(&file).unwrap();
        db.execute_batch("CREATE TABLE session(id TEXT,directory TEXT,title TEXT);CREATE TABLE message(id TEXT,session_id TEXT,data TEXT);CREATE TABLE part(id TEXT,data TEXT);INSERT INTO session VALUES('s','project','OC 标题');").unwrap();
        let message=json!({"role":"assistant","modelID":"oc-model","agent":"build","time":{"created":1790935200000i64,"completed":1790935200001i64},"tokens":{"input":20,"output":10,"reasoning":3,"cache":{"read":80,"write":5}}});
        db.execute("INSERT INTO message VALUES('m','s',?1)",[message.to_string()]).unwrap();db.execute_batch("INSERT INTO message VALUES('broken','s','{bad');").unwrap();db.execute("INSERT INTO part VALUES('part',?1)",[message.to_string()]).unwrap();db.close().unwrap();
        let original=std::fs::read(&file).unwrap();
        let modern_root=f.0.join("modern");std::fs::create_dir_all(&modern_root).unwrap();
        let modern=rusqlite::Connection::open(modern_root.join("opencode.db")).unwrap();
        modern.execute_batch("CREATE TABLE session_v2(id TEXT,directory TEXT,title TEXT);CREATE TABLE session_message(id TEXT,session_id TEXT,type TEXT,data TEXT);INSERT INTO session_v2 VALUES('modern','modern-project','新版标题');").unwrap();
        let mut modern_message=message.clone();modern_message.as_object_mut().unwrap().remove("role");modern_message.as_object_mut().unwrap().remove("modelID");modern_message["model"]=json!({"id":"modern-model"});
        modern.execute("INSERT INTO session_message VALUES('modern-msg','modern','assistant',?1)",[modern_message.to_string()]).unwrap();
        modern.execute("INSERT INTO session_message VALUES('user','modern','user',?1)",[modern_message.to_string()]).unwrap();modern.close().unwrap();
        f.write("legacy/storage/session/project/s.json",&json!({"id":"legacy","directory":"old","title":"旧版标题"}).to_string());
        f.write("legacy/storage/message/legacy/broken.json","{bad");
        let mut old=message.clone();old["id"]=json!("old-msg");old["sessionID"]=json!("legacy");
        f.write("legacy/storage/message/legacy/m.json",&old.to_string());
        f.write("sessions/k/agents/main/wire.jsonl",&event("kimi",9));
        let codex=|n,time|json!({"type":"event_msg","timestamp":time,"payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":n,"cached_input_tokens":n/2,"output_tokens":10}}}}).to_string();
        f.write("codex/a.jsonl",&format!("{}\n{}\n{}",json!({"type":"session_meta","payload":{"id":"codex"}}),codex(100,"2026-10-02T10:00:00Z"),codex(200,"2026-10-02T10:01:00Z")));
        let cfg=json!({"roots":[f.0.join("sessions")],"models":[],"setupDone":true,"monitors":[{"agent":"opencode","root":oc,"enabled":true},{"agent":"opencode","root":modern_root,"enabled":true},{"agent":"opencode","root":f.0.join("legacy"),"enabled":true},{"agent":"codex","root":f.0.join("codex"),"enabled":true}]});
        let mut e=f.engine();e.apply_config(&cfg);
        let (sessions,records)=e.scan();assert_eq!(records.len(),6);assert_eq!(std::fs::read(&file).unwrap(),original);assert!(e.scan_warnings.iter().any(|w|w["agent"]=="opencode"));
        let oc_rec=records.iter().find(|r|r.tool=="opencode").unwrap();assert_eq!((oc_rec.i,oc_rec.o,oc_rec.r,oc_rec.c),(20,10,80,5));
        assert!(sessions.iter().any(|s|s.title=="OC 标题"));
        assert!(sessions.iter().any(|s|s.title=="新版标题"));assert!(records.iter().any(|r|r.m=="modern-model"));
        let total=records.iter().map(|r|r.i+r.o+r.r+r.c).sum::<i64>();
        assert_eq!(records.iter().filter(|r|r.tool=="codex").map(|r|r.i+r.r).sum::<i64>(),200);
        for relative in ["oc/opencode.db","modern/opencode.db","legacy/storage/message/legacy/m.json","legacy/storage/session/project/s.json","codex/a.jsonl","sessions/k/agents/main/wire.jsonl"] {std::fs::remove_file(f.0.join(relative)).unwrap();}
        let (_,again)=e.scan();assert_eq!(again.len(),6);assert_eq!(again.iter().map(|r|r.i+r.o+r.r+r.c).sum::<i64>(),total);
        let mut restored=Engine::new(e.config_dir.clone());let (_,again)=restored.scan();assert_eq!(again.len(),6);assert_eq!(again.iter().map(|r|r.i+r.o+r.r+r.c).sum::<i64>(),total);
        restored.apply_config(&json!({"roots":[],"monitors":[],"models":[]}));assert!(restored.scan().1.is_empty());
        restored.apply_config(&cfg);assert_eq!(restored.scan().1.len(),6);
        // 备份可恢复损坏主文件。
        let history=e.config_dir.join("usage-history.json");std::fs::copy(&history,e.config_dir.join("usage-history.json.bak")).unwrap();std::fs::write(&history,"broken").unwrap();
        let mut recovered=Engine::new(e.config_dir.clone());assert_eq!(recovered.scan().1.len(),6);assert!(recovered.history_error.is_empty());
        let tmp=e.config_dir.join("usage-history.json.tmp");std::fs::create_dir(&tmp).unwrap();
        let mut retry=old.clone();retry["id"]=json!("retry");
        f.write("legacy/storage/message/legacy/retry.json",&retry.to_string());
        assert_eq!(recovered.scan().1.len(),7);assert!(recovered.scan_warnings.iter().any(|w|w["agent"]=="history"));
        std::fs::remove_file(f.0.join("legacy/storage/message/legacy/retry.json")).unwrap();std::fs::remove_dir(tmp).unwrap();
        assert_eq!(recovered.scan().1.len(),7);assert!(!recovered.history_dirty);
        let mut restarted=Engine::new(e.config_dir.clone());assert_eq!(restarted.scan().1.len(),7);
    }

    #[test]
    fn close_action_roundtrip_and_validation() {
        let fixture = Fixture::new();
        let mut e = fixture.engine();
        assert_eq!(e.close_action(), "background"); // 默认后台运行
        assert!(!e.apply_config(&json!({"monitors":[],"closeAction":"exit"})).contains("error"));
        assert_eq!(e.close_action(), "exit");
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(e.config_file()).unwrap()).unwrap();
        assert_eq!(saved["closeAction"], "exit");
        // 非法值在 HTTP 配置校验层被拒绝；Engine 内未知值回退为后台运行
        assert!(!valid_config(&json!({"monitors":[],"closeAction":"never"})));
        assert!(valid_config(&json!({"monitors":[],"closeAction":"ask"})));
        e.cfg.close_action = String::from("weird");
        assert_eq!(e.close_action(), "background");
    }

    #[test]
    fn renamed_config_dir_drops_identifier_naming() {
        let base = std::env::temp_dir().join("agentmonitor-config-dir-test");
        assert_eq!(renamed_config_dir(Some(base.join("com.yfan945.agentmonitor"))), base.join("AgentMonitor"));
    }

    #[test]
    fn renamed_config_migration_preserves_legacy_and_existing_settings() {
        let fixture = Fixture::new();
        let legacy = fixture.0.join("legacy");
        let target = fixture.0.join("renamed");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(legacy.join("config.json"),"corrupt").unwrap();
        let saved = r#"{"setupDone":true,"monitors":[],"models":["gpt-5.5"]}"#;
        std::fs::write(legacy.join("config.json.bak"),saved).unwrap();
        migrate_legacy_config(&target,&legacy).unwrap();
        assert_eq!(std::fs::read_to_string(target.join("config.json")).unwrap(),saved);
        assert_eq!(std::fs::read_to_string(legacy.join("config.json")).unwrap(),"corrupt");
        assert_eq!(std::fs::read_to_string(legacy.join("config.json.bak")).unwrap(),saved);
        std::fs::write(target.join("config.json"),"{}").unwrap();
        migrate_legacy_config(&target,&legacy).unwrap();
        assert_eq!(std::fs::read_to_string(target.join("config.json")).unwrap(),"{}");
        std::fs::remove_file(target.join("config.json")).unwrap();
        std::fs::write(target.join("config.json.bak"),"{}").unwrap();
        migrate_legacy_config(&target,&legacy).unwrap();
        assert!(!target.join("config.json").exists());
    }
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static SEQ: AtomicUsize = AtomicUsize::new(0);
            let name = format!("kimi-rust-test-{}-{}-{}", std::process::id(), now_ms(), SEQ.fetch_add(1, Ordering::Relaxed));
            let dir = std::env::temp_dir().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn write(&self, relative: &str, content: &str) {
            let file = self.0.join(relative);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, content).unwrap();
        }
        fn engine(&self) -> Engine {
            Engine {
                config_dir: self.0.join("config"),
                cfg: Config { roots: vec![self.0.join("sessions").to_string_lossy().to_string()], setup_done: true, ..Default::default() },
                ..Engine::default()
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
            assert!(self.0.file_name().unwrap().to_string_lossy().starts_with("kimi-rust-test-"));
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn event(model: &str, count: i64) -> String {
        json!({"type":"usage.record", "time":1700000000000i64, "model":model,
            "usage":{"inputOther":count,"output":5}}).to_string()
    }
    fn assert_associations(data: &Value) {
        assert_eq!(data["sessions"].as_array().unwrap().len(), 2);
        let records = data["records"].as_array().unwrap();
        assert_eq!(records.len(), 3);
        for r in records {
            let s = r["s"].as_u64().unwrap() as usize;
            assert_eq!(data["sessions"][s]["cwd"], r["m"]);
        }
        assert!(records.iter().any(|r| r["i"] == 3_000_000_000i64));
    }

    #[test]
    fn session_indices_survive_cached_scans_and_overlapping_roots() {
        let f = Fixture::new();
        f.write("sessions/ws/a/state.json", r#"{"cwd":"project-a"}"#);
        f.write("sessions/ws/a/agents/main/wire.jsonl", &format!("{}\n{}\n{{broken", event("project-a", 3_000_000_000), event("project-a", 10).replace("\"type\":", "\"type\": ")));
        // b 没有 state.json，使用索引恢复项目。
        f.write("sessions/ws/b/agents/main/wire.jsonl", &event("project-b", 20));
        f.write("sessions/session_index.jsonl", r#"{"sessionId":"b","workDir":"project-b"}"#);
        let mut engine = f.engine();
        assert_associations(&serde_json::from_str(&engine.get_data_string()).unwrap());
        engine.cfg.roots.insert(0, f.0.join("sessions/ws").to_string_lossy().to_string());
        engine.last_scan = None;
        assert_associations(&serde_json::from_str(&engine.get_data_string()).unwrap());
    }

    #[test]
    fn parser_accepts_whitespace_and_skips_malformed_lines() {
        let f = Fixture::new();
        f.write("wire.jsonl", &format!("{}\n{{broken\n{{\"type\":\"other\"}}", event("demo", 3_000_000_000).replace("\"type\":", "\"type\": ")));
        let records = Engine::parse_wire_file(&f.0.join("wire.jsonl"));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].i, 3_000_000_000);
        assert_eq!(token_count(&json!(-1)), 0);
        assert_eq!(token_count(&json!(1.5)), 0);
        assert_eq!(token_count(&json!("100")), 0);
        assert_eq!(token_count(&json!(9_007_199_254_740_992i64)), 0);
    }

    #[test]
    fn invalid_config_is_rejected() {
        for body in [Value::Null, json!({}), json!({"roots":"abc"}), json!({"roots":[1]}), json!({"roots":[],"setupDone":"yes"})] {
            assert!(!valid_config(&body));
        }
        assert!(valid_config(&json!({"roots":[],"setupDone":true})));
        assert!(valid_config(&json!({"roots":["somewhere"]})));
    }

    #[test]
    fn default_probe_is_cached_and_invalidated_by_config() {
        let f = Fixture::new();
        let mut engine = f.engine();
        let data: Value = serde_json::from_str(&engine.get_data_string()).unwrap();
        assert!(data["defaultStats"].is_null()); // 完成引导后不扫描默认目录
        engine.default_probe = Some((true, Some(json!({"cached":true}))));
        engine.last_scan = None;
        let data: Value = serde_json::from_str(&engine.get_data_string()).unwrap();
        assert_eq!(data["defaultStats"]["cached"], true);
        engine.apply_config(&json!({"roots":[],"setupDone":true}));
        assert!(engine.default_probe.is_none());
    }

    #[test]
    fn agent_adapters_deduplicate_and_keep_session_links() {
        let f = Fixture::new();
        let assistant = |n:i64| json!({"type":"assistant","sessionId":"s","timestamp":"2026-10-02T10:00:00Z","cwd":"project","message":{"id":"msg","model":"claude-test","usage":{"input_tokens":100,"output_tokens":n,"cache_read_input_tokens":20,"cache_creation_input_tokens":5}}}).to_string();
        f.write("claude/main.jsonl",&format!("{}\n{{bad\n{}",assistant(2),assistant(7)));
        let token_event = |i:i64,o:i64,r:i64| json!({"type":"event_msg","timestamp":"2026-10-02T10:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":i,"output_tokens":o,"cached_input_tokens":r}}}}).to_string();
        f.write("codex/main.jsonl",&format!("{}\n{}\n{}\n{}\n{}",json!({"type":"session_meta","payload":{"id":"c","cwd":"codex-project"}}),json!({"type":"turn_context","payload":{"model":"gpt-test"}}),token_event(100,10,40),token_event(100,10,40),token_event(150,15,60)));
        f.write("imports/main.jsonl",&json!({"type":"usage.record","time":1700000000000i64,"eventId":"call","model":"deepseek-test","usage":{"inputOther":9,"output":2}}).to_string());
        let mut e = f.engine();
        let monitors = json!([
            {"agent":"claude","root":f.0.join("claude"),"enabled":true},
            {"agent":"codex","root":f.0.join("codex"),"enabled":true},
            {"agent":"dsh","root":f.0.join("imports"),"enabled":true}
        ]);
        assert!(valid_config(&json!({"monitors":monitors})));
        let result: Value = serde_json::from_str(&e.apply_config(&json!({"monitors":monitors,"setupDone":true}))).unwrap();
        assert_eq!(result["ok"],true);
        let data: Value = serde_json::from_str(&e.get_data_string()).unwrap();
        let recs = data["records"].as_array().unwrap();
        assert_eq!(recs.len(),4);
        assert_eq!(recs.iter().find(|r|r["tool"]=="claude").unwrap()["o"],7);
        let codex: Vec<_> = recs.iter().filter(|r|r["tool"]=="codex").collect();
        assert_eq!(codex[0]["i"],60);assert_eq!(codex[1]["i"],30);
        for r in recs {assert_eq!(data["sessions"][r["s"].as_u64().unwrap() as usize]["tool"],r["tool"]);}
        e.apply_config(&json!({"monitors":monitors,"models":["gpt-test"]}));
        let data: Value = serde_json::from_str(&e.get_data_string()).unwrap();
        assert_eq!(data["records"].as_array().unwrap().len(),2);
        assert!(data["availableModels"].as_array().unwrap().contains(&json!("claude-test")));
        let reloaded = Engine::new(f.0.join("config"));
        assert_eq!(reloaded.cfg.models,vec!["gpt-test"]);
        e.apply_config(&json!({"monitors":[],"models":[]}));
        let data: Value = serde_json::from_str(&e.get_data_string()).unwrap();
        assert!(data["records"].as_array().unwrap().is_empty());
    }

    #[test]
    fn native_zcode_workbuddy_and_compressed_dsh() {
        let f=Fixture::new();
        let buddy=json!({"type":"function_call","sessionId":"buddy","timestamp":"2026-10-02T10:00:00Z","message":{"usage":{"input_tokens":100,"output_tokens":10,"cache_read_input_tokens":40}},"providerData":{"messageId":"response","model":"buddy-test","rawUsage":{}}}).to_string();
        f.write("buddy.jsonl",&format!("{buddy}\n{buddy}"));
        let parsed=agents::parse(&f.0.join("buddy.jsonl"),"workbuddy").unwrap();
        assert_eq!(parsed.len(),1);assert_eq!(parsed[0].3.i,100);assert!(parsed[0].3.inclusive);
        f.write("zcode.jsonl",&json!({"type":"model_io","completedAt":"2026-10-02T10:00:00Z","sessionId":"z","response":{"responseId":"response","modelId":"glm-test","usage":{"inputTokens":100,"outputTokens":10,"cacheReadTokens":40}}}).to_string());
        let parsed=agents::parse(&f.0.join("zcode.jsonl"),"zcode").unwrap();
        assert_eq!(parsed[0].3.i,100);assert!(parsed[0].3.inclusive);assert_eq!(parsed[0].3.m,"glm-test");
        let message=|n:i64| json!({"type":"assistant/message","time":1790935200000i64,"data":{"turn":1,"step":1,"message":{"source":{"model":"deepseek-test"}},"stream":[{"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":20,"outputTokens":n,"cacheReadTokens":80}}}]}}).to_string();
        let text=format!("{}\n{}\n{}\n{}\n{}\n{}\n{}",json!({"type":"session","id":"dsh","version":4}),message(999),json!({"type":"session/end-seed","data":{"inherited":true}}),message(4),message(5),json!({"type":"llm/retry-started"}),message(6));
        let compressed=zstd::stream::encode_all(text.as_bytes(),0).unwrap();
        std::fs::write(f.0.join("session.v4.jsonl.zstd"),compressed).unwrap();
        f.write("session.v3.jsonl",&text);
        let files=agents::select_dsh_files(agents::files(&f.0));
        assert!(!files.contains(&f.0.join("session.v3.jsonl")));
        let parsed=agents::parse(&f.0.join("session.v4.jsonl.zstd"),"dsh").unwrap();
        assert_eq!(parsed.len(),2);
        assert_eq!(parsed.iter().map(|x|x.3.o).sum::<i64>(),11);
        assert!(parsed.iter().all(|x|x.3.i==20 && x.3.r==80));
    }

    #[test]
    fn zcode_database_uses_readonly_usage_rows_and_cache_split() {
        let f=Fixture::new();let file=f.0.join("db.sqlite");
        let db=rusqlite::Connection::open(&file).unwrap();
        db.execute_batch("CREATE TABLE session(id TEXT PRIMARY KEY,directory TEXT,title TEXT);CREATE TABLE model_usage(id TEXT PRIMARY KEY,session_id TEXT,model_id TEXT,agent TEXT,started_at INTEGER,completed_at INTEGER,input_tokens INTEGER,output_tokens INTEGER,cache_read_input_tokens INTEGER,cache_creation_input_tokens INTEGER);INSERT INTO session VALUES('s','project','title');INSERT INTO model_usage VALUES('call','s','glm','main',1790935200000,1790935200001,100,10,40,5);").unwrap();
        db.close().unwrap();let before=std::fs::read(&file).unwrap();
        let parsed=agents::parse_zcode_database(&file).unwrap();
        assert_eq!(parsed.len(),1);assert_eq!((parsed[0].3.i,parsed[0].3.o,parsed[0].3.r,parsed[0].3.c),(100,10,40,5));assert!(parsed[0].3.inclusive);
        assert_eq!(std::fs::read(&file).unwrap(),before);
        let mut e=f.engine();
        e.apply_config(&json!({"monitors":[{"agent":"zcode","root":f.0,"enabled":true}],"models":[]}));
        let data:Value=serde_json::from_str(&e.get_data_string()).unwrap();
        assert_eq!(data["records"].as_array().unwrap().len(),1);
        assert_eq!(data["sessions"][0]["cwd"],"project");
        assert_eq!(data["records"][0]["i"],55);
    }
    #[test]
    fn snapshots_merge_before_cache_split_and_across_files() {
        let f=Fixture::new();
        let buddy=|cache| json!({"type":"assistant","sessionId":"buddy","timestamp":"2026-10-02T10:00:00Z","message":{"id":"response","model":"buddy","usage":{"input_tokens":100,"output_tokens":10,"cache_read_input_tokens":cache}},"providerData":{"rawUsage":{}}}).to_string();
        f.write("buddy/a.jsonl",&buddy(0));f.write("buddy/b.jsonl",&buddy(40));
        let claude=|output| json!({"type":"assistant","sessionId":"claude","timestamp":"2026-10-02T10:00:00Z","message":{"id":"response","model":"claude","usage":{"input_tokens":10,"output_tokens":output}}}).to_string();
        f.write("claude/a.jsonl",&claude(5));f.write("claude/b.jsonl",&claude(10));
        let codex=|n,time| format!("{}\n{}",json!({"type":"session_meta","payload":{"id":"codex"}}),json!({"type":"event_msg","timestamp":time,"payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":n}}}}));
        f.write("codex/z-first.jsonl",&codex(100,"2026-10-02T10:00:00Z"));
        f.write("codex/a-last.jsonl",&codex(300,"2026-10-02T10:01:00Z"));
        let mut e=f.engine();e.apply_config(&json!({"roots":[],"monitors":[{"agent":"workbuddy","root":f.0.join("buddy"),"enabled":true},{"agent":"claude","root":f.0.join("claude"),"enabled":true},{"agent":"codex","root":f.0.join("codex"),"enabled":true}]}));
        let (_,records)=e.scan();
        let buddy=records.iter().find(|r|r.tool=="workbuddy").unwrap();assert_eq!((buddy.i,buddy.o,buddy.r),(60,10,40));
        assert_eq!(records.iter().find(|r|r.tool=="claude").unwrap().o,10);
        assert_eq!(records.iter().filter(|r|r.tool=="codex").map(|r|r.i).sum::<i64>(),300);
        assert_eq!(records.len(),4);
        let (_,cached)=e.scan();assert_eq!(cached.iter().map(|r|r.i+r.o+r.r+r.c).sum::<i64>(),430);
    }

    #[test]
    fn unreadable_compressed_logs_report_warning_and_do_not_poison_cache() {
        let f=Fixture::new();f.write("dsh/session.v4.jsonl.zstd","broken zstd");
        let mut e=f.engine();e.apply_config(&json!({"roots":[],"monitors":[{"agent":"dsh","root":f.0.join("dsh"),"enabled":true}]}));
        let (_,records)=e.scan();assert!(records.is_empty());assert_eq!(e.scan_warnings.len(),1);assert!(e.agent_cache.is_empty());
    }

    #[test]
    fn atomic_configuration_survives_write_failure_and_recovers_backup() {
        let f=Fixture::new();let mut e=f.engine();e.save_config().unwrap();
        let old=std::fs::read(e.config_file()).unwrap();
        std::fs::create_dir(e.config_dir.join("config.json.tmp")).unwrap();
        e.cfg.models=vec!["changed".into()];assert!(e.save_config().is_err());assert_eq!(std::fs::read(e.config_file()).unwrap(),old);
        std::fs::remove_dir(e.config_dir.join("config.json.tmp")).unwrap();e.save_config().unwrap();
        std::fs::write(e.config_file(),"corrupt").unwrap();let recovered=Engine::new(e.config_dir.clone());assert!(recovered.cfg.models.is_empty());
        recovered.save_config().unwrap();assert!(serde_json::from_slice::<Value>(&std::fs::read(e.config_file()).unwrap()).is_ok());
    }

    #[test]
    fn http_rejects_oversized_and_truncated_requests() {
        fn request(raw:&str)->Result<(String,String,Vec<u8>),&'static str> {
            let listener=TcpListener::bind(("127.0.0.1",0)).unwrap();
            let mut client=TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            client.write_all(raw.as_bytes()).unwrap();client.shutdown(std::net::Shutdown::Write).unwrap();
            let (mut server,_)=listener.accept().unwrap();read_request(&mut server)
        }
        assert_eq!(request("POST /api/config HTTP/1.1\r\nContent-Length: 1048577\r\n\r\n").unwrap_err(),"413 Payload Too Large");
        assert_eq!(request("POST /api/config HTTP/1.1\r\nContent-Length: 20\r\n\r\n{}").unwrap_err(),"400 Bad Request");
        assert_eq!(request("POST /api/config HTTP/1.1\r\nContent-Length: nope\r\n\r\n").unwrap_err(),"400 Bad Request");
        assert_eq!(request("POST /api/config HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}").unwrap().2,b"{}");
    }

    #[test]
    fn titles_use_readonly_metadata_and_log_labels() {
        let f=Fixture::new();
        f.write("title-codex/sessions/thread.jsonl",&format!("{}\n{}",json!({"type":"session_meta","payload":{"id":"thread-title"}}),json!({"type":"event_msg","timestamp":"2026-10-02T10:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100}}}})));
        let c_file=f.0.join("title-codex/state_5.sqlite");let db=rusqlite::Connection::open(&c_file).unwrap();db.execute_batch("CREATE TABLE threads(id TEXT,title TEXT);INSERT INTO threads VALUES('thread-title','Codex 数据库标题');").unwrap();db.close().unwrap();
        let assistant=|sid:&str|json!({"type":"assistant","sessionId":sid,"timestamp":"2026-10-02T10:00:00Z","message":{"id":"call","model":"demo","usage":{"input_tokens":10,"output_tokens":2}}}).to_string();
        f.write("title-workbuddy/projects/thread.jsonl",&assistant("wb-title"));
        let w_file=f.0.join("title-workbuddy/workbuddy.db");let db=rusqlite::Connection::open(&w_file).unwrap();db.execute_batch("CREATE TABLE sessions(id TEXT,title TEXT,custom_title TEXT);INSERT INTO sessions VALUES('wb-title','自动标题','用户自定义标题');").unwrap();db.close().unwrap();
        f.write("title-claude/thread.jsonl",&format!("{}\n{}\n{}",json!({"type":"user","sessionId":"cl-title","message":{"content":"第一个问题"}}),assistant("cl-title"),json!({"type":"custom-title","sessionId":"cl-title","customTitle":"Claude 自定义标题"})));
        f.write("title-claude/fallback.jsonl",&format!("{}\n{}",json!({"type":"user","sessionId":"fallback","message":{"content":[{"type":"text","text":"没有正式标题的提问"}]}}),assistant("fallback")));
        let bytes=(std::fs::read(&c_file).unwrap(),std::fs::read(&w_file).unwrap());
        let mut e=f.engine();e.apply_config(&json!({"roots":[],"monitors":[{"agent":"codex","root":f.0.join("title-codex/sessions"),"enabled":true},{"agent":"workbuddy","root":f.0.join("title-workbuddy/projects"),"enabled":true},{"agent":"claude","root":f.0.join("title-claude"),"enabled":true}]}));
        let (sessions,_)=e.scan();let titles:HashMap<_,_>=sessions.iter().map(|s|(s.id.as_str(),s.title.as_str())).collect();
        assert_eq!(titles["thread-title"],"Codex 数据库标题");assert_eq!(titles["wb-title"],"用户自定义标题");assert_eq!(titles["cl-title"],"Claude 自定义标题");assert_eq!(titles["fallback"],"首条提问：没有正式标题的提问");
        assert_eq!(std::fs::read(c_file).unwrap(),bytes.0);assert_eq!(std::fs::read(w_file).unwrap(),bytes.1);assert!(e.scan_warnings.is_empty());
    }

    #[test]
    fn dsh_multiframe_restore_keeps_usage_and_excludes_fork_history() {
        let f=Fixture::new();let time=1790935200000i64;
        let usage=|step,at|json!({"type":"assistant/message","time":at,"data":{"turn":1,"step":step,"usage":{"inputTokens":20,"outputTokens":5},"message":{"source":{"model":"deepseek"}}}});
        let frames=vec![vec![json!({"type":"session","id":"original","isSeeded":false,"createdAt":time-1000})],vec![usage(1,time),json!({"type":"session/end-seed","data":{}})],vec![usage(2,time+1),json!({"type":"session/title","data":{"title":"DSH 真实标题"}}),json!({"type":"session/end-seed","data":{}})]];
        let mut compressed=Vec::new();for frame in frames {let text=frame.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")+"\n";compressed.extend(zstd::stream::encode_all(text.as_bytes(),1).unwrap());}
        f.write("dsh-original/session.v4.jsonl.zstd","");std::fs::write(f.0.join("dsh-original/session.v4.jsonl.zstd"),compressed).unwrap();
        f.write("dsh-fork/session.v4.jsonl",&vec![json!({"type":"session","id":"fork","isSeeded":true,"createdAt":time}),usage(1,time-10),json!({"type":"session/end-seed","data":{"inherited":true}}),usage(2,time+1),json!({"type":"session/end-seed","data":{}})].iter().map(Value::to_string).collect::<Vec<_>>().join("\n"));
        let mut e=f.engine();e.apply_config(&json!({"roots":[],"monitors":[{"agent":"dsh","root":f.0.join("dsh-original"),"enabled":true},{"agent":"dsh","root":f.0.join("dsh-fork"),"enabled":true}]}));
        let (sessions,records)=e.scan();assert_eq!(records.len(),3);assert_eq!(records.iter().map(|r|r.o).sum::<i64>(),15);assert_eq!(sessions.iter().find(|s|s.id=="original").unwrap().title,"DSH 真实标题");assert!(e.scan_warnings.is_empty());
    }

}
