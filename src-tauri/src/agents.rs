//! 与 Node 引擎一致的产品来源和 JSONL 用量适配器。
use super::{token_count, to_epoch, Rec, Session};
use serde_json::{json, Value};
use std::{collections::HashMap, path::Path, io::{BufRead, BufReader, Read}};

pub fn catalog() -> Vec<Value> {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    [
        ("kimi", "Kimi Code", ".kimi-code/sessions", "native"),
        ("claude", "Claude Code", ".claude/projects", "native"),
        ("codex", "Codex", ".codex/sessions", "native"),
        ("workbuddy", "WorkBuddy", ".workbuddy/projects", "native"),
        ("codebuddy", "CodeBuddy", ".codebuddy/projects", "compatible"),
        ("zcode", "ZCode", ".zcode/cli/db", "native"), ("doubao", "Doubao", "", "import"),
        ("trae", "Trae", "", "import"), ("dsh", "DeepSeek Harness", ".dsh/sessions", "native"),
    ].into_iter().map(|(id, name, dir, support)| {
        let root = if dir.is_empty() { String::new() } else { Path::new(&home).join(dir).to_string_lossy().to_string() };
        json!({"id":id,"name":name,"support":support,"defaultRoot":root,"detected":!root.is_empty() && Path::new(&root).is_dir(),"compressedLogs":true})
    }).collect()
}

pub fn valid_monitors(v: &Value) -> bool {
    v.as_array().map(|rows| rows.iter().all(|m| {
        catalog().iter().any(|a| a["id"] == m["agent"]) && m["root"].is_string() && m["enabled"].is_boolean()
    })).unwrap_or(false)
}

pub fn normalize(v: &[Value]) -> Vec<Value> {
    let mut seen = std::collections::HashSet::new();
    v.iter().filter_map(|m| {
        let root = m["root"].as_str().unwrap_or_default().trim();
        let agent = m["agent"].as_str().unwrap_or_default();
        if !seen.insert(format!("{agent}:{}", if cfg!(windows) {root.to_lowercase()} else {root.to_string()})) { return None; }
        Some(json!({"agent":agent,"root":root,"enabled":m["enabled"]}))
    }).collect()
}

fn pick<'a>(v: &'a Value, fields: &[&str]) -> &'a Value {
    fields.iter().find_map(|k| v.get(*k).filter(|x| !x.is_null())).unwrap_or(&Value::Null)
}

pub fn parse(file: &Path, agent: &str) -> Vec<(String, String, Session, Rec)> {
    let Ok(raw) = std::fs::File::open(file) else { return Vec::new(); };
    let reader: Box<dyn Read> = if file.extension().map(|x|x=="zstd").unwrap_or(false) {
        let Ok(decoder) = zstd::stream::read::Decoder::new(raw) else { return Vec::new(); };Box::new(decoder)
    } else {Box::new(raw)};
    let mut model = "unknown".to_string();
    let mut cwd = String::new();
    let mut id = file.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let mut previous: Option<[i64; 4]> = None;
    let mut retry = 0;
    let mut out = Vec::new();
    let mut indices: HashMap<String, usize> = HashMap::new();
    for line in BufReader::new(reader).lines() {
        let Ok(line) = line else { break; };
        if agent == "codex" && !["session_meta","turn_context","token_count"].iter().any(|t|line.contains(t)) {continue;}
        if ["claude","workbuddy","codebuddy"].contains(&agent) && !line.contains("usage") {continue;}
        let Ok(e) = serde_json::from_str::<Value>(&line) else { continue; };
        let p = &e["payload"];
        if agent == "codex" && e["type"] == "session_meta" {
            id = p["id"].as_str().unwrap_or(&id).to_string();
            cwd = p["cwd"].as_str().unwrap_or(&cwd).to_string();
        }
        if agent == "codex" && e["type"] == "turn_context" {
            model = p["model"].as_str().unwrap_or(&model).to_string();
            cwd = p["cwd"].as_str().unwrap_or(&cwd).to_string();
        }
        if agent == "dsh" && e["type"] == "session" {id=e["id"].as_str().unwrap_or(&id).to_string();cwd=e["cwd"].as_str().unwrap_or(&cwd).to_string();}
        if agent == "dsh" && e["type"] == "request/header" {model=e["data"]["header"]["config"]["model"].as_str().unwrap_or(&model).to_string();}
        if agent == "dsh" && e["type"] == "session/end-seed" {out.clear();indices.clear();}
        if agent == "dsh" && e["type"] == "llm/retry-started" {retry+=1;}
        let (u, key, inclusive) = if agent == "zcode" && e["type"] == "model_io" {
            if !e["response"]["usage"].is_object() {continue;}
            let raw = &e["response"]["providerMetadata"]["anthropic"]["usage"];
            let u = if raw.is_object() {raw.clone()} else {e["response"]["usage"].clone()};
            model=e["response"]["modelId"].as_str().or_else(||e["model"]["modelId"].as_str()).unwrap_or("unknown").to_string();
            let event=e["response"]["responseId"].as_str().map(String::from).unwrap_or_else(||format!("{}:{}",e["requestId"].as_str().map(String::from).unwrap_or_else(||out.len().to_string()),e["attempt"].as_i64().unwrap_or(0)));
            (u,format!("{}:{event}",e["sessionId"].as_str().unwrap_or(&id)),!raw.is_object())
        } else if agent == "dsh" && (e["type"] == "assistant/message" || e["type"] == "assistant/attempt") {
            let d=&e["data"];
            let sample = d["stream"].as_array().and_then(|a|a.iter().rev().find(|x|x["type"]=="chunk" && x["chunk"]["type"]=="usage")).map(|x|x["chunk"]["usage"].clone());
            let Some(u) = d.get("usage").cloned().or(sample) else {continue;};
            model=d["message"]["source"]["model"].as_str().unwrap_or(&model).to_string();
            (u,format!("{id}:{}:{}:{retry}",d["turn"],d["step"]),false)
        } else if agent == "codex" {
            if e["type"] != "event_msg" || p["type"] != "token_count" || !p["info"]["total_token_usage"].is_object() { continue; }
            let total = &p["info"]["total_token_usage"];
            let counts = [token_count(&total["input_tokens"]), token_count(&total["output_tokens"]), token_count(&total["cached_input_tokens"]), token_count(&total["cache_write_input_tokens"])];
            if previous.map(|prev| counts.iter().zip(prev).all(|(n, p)| *n <= p)).unwrap_or(false) { continue; }
            let delta: Vec<i64> = counts.iter().enumerate().map(|(i, n)| previous.map(|prev| (*n - prev[i]).max(0)).unwrap_or(*n)).collect();
            previous = Some(counts);
            if delta.iter().all(|n| *n == 0) { continue; }
            (json!({"input_tokens":delta[0],"output_tokens":delta[1],"cached_input_tokens":delta[2],"cache_write_input_tokens":delta[3]}), format!("{id}:{}:{}:{}:{}",counts[0],counts[1],counts[2],counts[3]), true)
        } else if ["claude", "workbuddy", "codebuddy"].contains(&agent) {
            let valid_type = if agent == "claude" { e["type"] == "assistant" } else { ["assistant","function_call","message"].iter().any(|t| e["type"] == *t) };
            if !valid_type || !e["message"]["usage"].is_object() { continue; }
            if e["isApiErrorMessage"] == true || e["message"]["model"] == "<synthetic>" {continue;}
            model = e["message"]["model"].as_str().or_else(|| e["providerData"]["model"].as_str()).unwrap_or(&model).to_string();
            let sid = e["sessionId"].as_str().unwrap_or(&id);
            let event = e["message"]["id"].as_str().or_else(|| e["providerData"]["messageId"].as_str()).or_else(|| e["requestId"].as_str()).or_else(|| e["uuid"].as_str()).or_else(|| e["id"].as_str()).map(String::from).unwrap_or_else(|| out.len().to_string());
            (e["message"]["usage"].clone(), format!("{sid}:{event}"), agent != "claude" && e["providerData"]["rawUsage"].is_object())
        } else {
            if e["type"] != "usage.record" || !e["usage"].is_object() { continue; }
            model = e["model"].as_str().unwrap_or("unknown").to_string();
            let sid = e["sessionId"].as_str().unwrap_or(&id);
            let event = e["eventId"].as_str().or_else(|| e["id"].as_str()).map(String::from).unwrap_or_else(|| out.len().to_string());
            (e["usage"].clone(), format!("{sid}:{event}"), false)
        };
        let t = to_epoch(pick(&e, &["timestamp", "time", "completedAt", "startedAt"]));
        if t <= 0 { continue; }
        let sid = e["sessionId"].as_str().unwrap_or(&id).to_string();
        cwd = e["cwd"].as_str().unwrap_or(&cwd).to_string();
        let r = token_count(pick(&u, &["inputCacheRead", "cache_read_input_tokens", "cached_input_tokens", "cacheReadTokens"]));
        let c = token_count(pick(&u, &["inputCacheCreation", "cache_creation_input_tokens", "cache_write_input_tokens", "cacheWriteTokens"]));
        let input = token_count(pick(&u, &["inputOther", "input_tokens", "inputTokens"]));
        let mut rec = Rec {s:0,t,m:model.clone(),tool:agent.to_string(),a:e["agentId"].as_str().unwrap_or("main").to_string(),i:if inclusive {(input-r-c).max(0)} else {input},o:token_count(pick(&u, &["output", "output_tokens", "outputTokens"])),r,c};
        if agent != "dsh" { if let Some(i) = indices.get(&key) { let old: &(String,String,Session,Rec) = &out[*i];rec.i=rec.i.max(old.3.i);rec.o=rec.o.max(old.3.o);rec.r=rec.r.max(old.3.r);rec.c=rec.c.max(old.3.c); } }
        let session = Session {id:sid.clone(),cwd:cwd.clone(),title:e["title"].as_str().unwrap_or_default().to_string(),tool:agent.to_string(),created_at:t,updated_at:t};
        let entry = (sid,key.clone(),session,rec);
        if let Some(i) = indices.get(&key) { out[*i] = entry; } else { indices.insert(key,out.len());out.push(entry); }
    }
    out
}

pub fn files(root: &Path) -> Vec<std::path::PathBuf> {
    fn walk(root: &Path, depth: usize, out: &mut Vec<std::path::PathBuf>, seen: &mut std::collections::HashSet<std::path::PathBuf>) {
        if depth > 12 { return; }
        let Ok(real) = std::fs::canonicalize(root) else { return; };
        if !seen.insert(real.clone()) { return; }
        let Ok(entries) = std::fs::read_dir(real) else { return; };
        for e in entries.flatten() {
            let Ok(kind) = e.file_type() else { continue; };
            if kind.is_dir() { walk(&e.path(),depth+1,out,seen); }
            else if kind.is_file() && (e.path().extension().map(|x| x == "jsonl").unwrap_or(false) || e.file_name().to_string_lossy().ends_with(".jsonl.zstd")) { out.push(e.path()); }
        }
    }
    let mut out = Vec::new();walk(root,0,&mut out,&mut Default::default());out.sort();out
}

pub fn select_dsh_files(files: Vec<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
    let mut generations: HashMap<std::path::PathBuf,(u32,std::path::PathBuf)> = HashMap::new();
    let mut other=Vec::new();
    for file in files {
        let name=file.file_name().unwrap_or_default().to_string_lossy();
        let base=name.strip_suffix(".zstd").unwrap_or(&name).strip_suffix(".jsonl");
        let version=base.and_then(|s|if s=="session" {Some(0)} else {s.strip_prefix("session.v").and_then(|v|v.parse::<u32>().ok())});
        if let Some(version)=version {
            let dir=file.parent().unwrap().to_path_buf();
            if generations.get(&dir).map(|(v,_)| version>*v).unwrap_or(true) {generations.insert(dir,(version,file));}
        } else {other.push(file);}
    }
    other.extend(generations.into_values().map(|(_,file)|file));other.sort();other
}

pub fn parse_zcode_database(file: &Path) -> Result<Vec<(String,String,Session,Rec)>,rusqlite::Error> {
    let db=rusqlite::Connection::open_with_flags(file,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt=db.prepare("SELECT u.id,u.session_id,u.model_id,u.agent,u.started_at,u.completed_at,u.input_tokens,u.output_tokens,u.cache_read_input_tokens,u.cache_creation_input_tokens,s.directory,s.title FROM model_usage u LEFT JOIN session s ON s.id=u.session_id WHERE u.input_tokens>0 OR u.output_tokens>0 OR u.cache_read_input_tokens>0 OR u.cache_creation_input_tokens>0 ORDER BY u.started_at,u.id")?;
    let rows=stmt.query_map([],|row| {
        let sid:String=row.get(1)?;let event:String=row.get(0)?;
        let t:Option<i64>=row.get(5)?;let t=t.unwrap_or(row.get(4)?);
        let count=|i:usize| -> Result<i64,rusqlite::Error> {Ok(token_count(&json!(row.get::<_,i64>(i)?)))};
        let r=count(8)?;let c=count(9)?;
        let session=Session {id:sid.clone(),cwd:row.get::<_,Option<String>>(10)?.unwrap_or_default(),title:row.get::<_,Option<String>>(11)?.unwrap_or_default(),tool:"zcode".into(),created_at:t,updated_at:t};
        let rec=Rec {s:0,t,m:row.get(2)?,a:row.get::<_,Option<String>>(3)?.unwrap_or_else(||"main".into()),tool:"zcode".into(),i:(count(6)?-r-c).max(0),o:count(7)?,r,c};
        Ok((sid.clone(),format!("{sid}:{event}"),session,rec))
    })?;
    rows.collect()
}
