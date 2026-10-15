//! 与 Node 引擎一致的产品来源和 JSONL 用量适配器。
use super::{token_count, to_epoch, Rec, Session};
use serde_json::{json, Value};
use std::{collections::HashMap, path::Path, io::{BufRead, BufReader, Read}};

pub fn catalog() -> Vec<Value> {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    [
        ("kimi", "Kimi Code", ".kimi-code/sessions", "native"),
        ("claude", "Claude Code", ".claude/projects", "native"),
        ("opencode", "OpenCode", ".local/share/opencode", "native"),
        ("codex", "Codex", ".codex/sessions", "native"),
        ("workbuddy", "WorkBuddy", ".workbuddy/projects", "native"),
        ("codebuddy", "CodeBuddy", ".codebuddy/projects", "compatible"),
        ("zcode", "ZCode", ".zcode/cli/db", "native"), ("doubao", "Doubao", "", "import"),
        ("trae", "Trae", "", "import"), ("dsh", "DeepSeek Harness", ".dsh/sessions", "native"),
    ].into_iter().map(|(id, name, dir, support)| {
        let root = if id=="opencode" {std::env::var("XDG_DATA_HOME").map(|p|Path::new(&p).join("opencode")).unwrap_or_else(|_|Path::new(&home).join(dir)).to_string_lossy().to_string()} else if dir.is_empty() { String::new() } else { if id=="dsh" {std::env::var("DSH_HOME").ok().filter(|p|!p.trim().is_empty()).map(|p|Path::new(&p).join("sessions")).unwrap_or_else(||Path::new(&home).join(dir)).to_string_lossy().to_string()} else {Path::new(&home).join(dir).to_string_lossy().to_string()} };
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

pub fn parse(file: &Path, agent: &str) -> std::io::Result<Vec<(String, String, Session, Rec)>> {
    let raw = std::fs::File::open(file)?;
    let reader: Box<dyn Read> = if file.extension().map(|x|x=="zstd").unwrap_or(false) {
        let decoder = zstd::stream::read::Decoder::new(raw)?;Box::new(decoder)
    } else {Box::new(raw)};
    let mut model = "unknown".to_string();
    let mut cwd = String::new();
    let mut id = file.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let mut retry = 0;
    let mut seeded_at=0;
    let mut labels: HashMap<String,String>=HashMap::new();
    let mut out = Vec::new();
    let mut indices: HashMap<String, usize> = HashMap::new();
    for line in BufReader::new(reader).lines() {
        let line = line?;
        if agent == "codex" && !["session_meta","turn_context","token_count","user_message"].iter().any(|t|line.contains(t)) {continue;}
        if ["claude","workbuddy","codebuddy"].contains(&agent) && !["usage","user","title","summary"].iter().any(|k|line.contains(k)) {continue;}
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
        if agent == "dsh" && e["type"] == "session" {id=e["id"].as_str().unwrap_or(&id).to_string();cwd=e["cwd"].as_str().unwrap_or(&cwd).to_string();seeded_at=if e["isSeeded"]==true {to_epoch(&e["createdAt"])} else {0};}
        if agent == "dsh" && e["type"] == "request/header" {model=e["data"]["header"]["config"]["model"].as_str().unwrap_or(&model).to_string();}
        if agent == "dsh" && e["type"] == "session/end-seed" && e["data"]["inherited"]==true {out.clear();indices.clear();}
        if agent == "dsh" && e["type"] == "llm/retry-started" {retry+=1;}
        let label_id=e["sessionId"].as_str().unwrap_or(&id).to_string();
        let explicit=pick(&e,&["customTitle","title"]).as_str().or_else(|| {
            if e["type"]=="summary" {e["summary"].as_str()} else if e["type"]=="session/title" {e["data"]["title"].as_str()} else {None}
        });
        if let Some(title)=explicit.filter(|t|!t.trim().is_empty()) {labels.insert(label_id.clone(),title.trim().into());}
        if !labels.contains_key(&label_id) {
            let content=if e["type"]=="user" {&e["message"]["content"]} else if e["type"]=="message" && e["role"]=="user" {&e["content"]} else if agent=="codex" && p["type"]=="user_message" {&p["message"]} else if e["type"]=="user/message" {&e["data"]["message"]["content"]} else {&Value::Null};
            let text=if let Some(text)=content.as_str() {text.to_string()} else {content.as_array().map(|a|a.iter().filter(|x|x["type"]=="text").filter_map(|x|x["text"].as_str()).collect::<Vec<_>>().join(" ")).unwrap_or_default()};
            let text=text.split_whitespace().collect::<Vec<_>>().join(" ");
            if !text.is_empty() {labels.insert(label_id,format!("首条提问：{}",text.chars().take(120).collect::<String>()));}
        }
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
            (json!({"input_tokens":counts[0],"output_tokens":counts[1],"cached_input_tokens":counts[2],"cache_write_input_tokens":counts[3]}), format!("{id}:{}:{}:{}:{}",counts[0],counts[1],counts[2],counts[3]), true)
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
        if t <= 0 || agent=="dsh" && seeded_at>0 && t<seeded_at { continue; }
        let sid = e["sessionId"].as_str().unwrap_or(&id).to_string();
        cwd = e["cwd"].as_str().unwrap_or(&cwd).to_string();
        let r = token_count(pick(&u, &["inputCacheRead", "cache_read_input_tokens", "cached_input_tokens", "cacheReadTokens"]));
        let c = token_count(pick(&u, &["inputCacheCreation", "cache_creation_input_tokens", "cache_write_input_tokens", "cacheWriteTokens"]));
        let input = token_count(pick(&u, &["inputOther", "input_tokens", "inputTokens"]));
        let mut rec = Rec {key:key.clone(),origin:file.to_string_lossy().to_string(),s:0,t,m:model.clone(),tool:agent.to_string(),a:e["agentId"].as_str().unwrap_or("main").to_string(),i:input,inclusive,o:token_count(pick(&u, &["output", "output_tokens", "outputTokens"])),r,c};
        if agent != "dsh" { if let Some(i) = indices.get(&key) { let old: &(String,String,Session,Rec) = &out[*i];rec.i=rec.i.max(old.3.i);rec.o=rec.o.max(old.3.o);rec.r=rec.r.max(old.3.r);rec.c=rec.c.max(old.3.c);rec.inclusive |= old.3.inclusive;if agent=="codex" && old.3.t<rec.t {rec.t=old.3.t;rec.m=old.3.m.clone();rec.a=old.3.a.clone();} } }
        let session = Session {id:sid.clone(),cwd:cwd.clone(),title:labels.get(&sid).cloned().unwrap_or_default(),tool:agent.to_string(),created_at:t,updated_at:t};
        let entry = (sid,key.clone(),session,rec);
        if let Some(i) = indices.get(&key) { out[*i] = entry; } else { indices.insert(key,out.len());out.push(entry); }
    }
    for (sid,_,session,_) in &mut out {if let Some(title)=labels.get(sid) {session.title=title.clone();}}
    Ok(out)
}

#[cfg(test)]
pub fn files(root: &Path) -> Vec<std::path::PathBuf> {files_with_warnings(root).0}

pub fn files_with_warnings(root: &Path) -> (Vec<std::path::PathBuf>,Vec<(std::path::PathBuf,String)>) {
    fn walk(root: &Path, depth: usize, out: &mut Vec<std::path::PathBuf>, seen: &mut std::collections::HashSet<std::path::PathBuf>, errors: &mut Vec<(std::path::PathBuf,String)>) {
        if depth > 12 { return; }
        let real = match std::fs::canonicalize(root) {Ok(p)=>p,Err(e)=>{errors.push((root.into(),e.to_string()));return;}};
        if !seen.insert(real.clone()) { return; }
        let entries = match std::fs::read_dir(&real) {Ok(v)=>v,Err(e)=>{errors.push((real,e.to_string()));return;}};
        for e in entries {
            let e=match e {Ok(e)=>e,Err(e)=>{errors.push((root.into(),e.to_string()));continue;}};
            let kind=match e.file_type() {Ok(k)=>k,Err(error)=>{errors.push((e.path(),error.to_string()));continue;}};
            if kind.is_dir() { walk(&e.path(),depth+1,out,seen,errors); }
            else if kind.is_file() && (e.path().extension().map(|x| x == "jsonl").unwrap_or(false) || e.file_name().to_string_lossy().ends_with(".jsonl.zstd")) { out.push(e.path()); }
        }
    }
    let mut out = Vec::new();let mut errors=Vec::new();walk(root,0,&mut out,&mut Default::default(),&mut errors);out.sort();(out,errors)
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
        let rec=Rec {key:format!("{sid}:{event}"),origin:file.to_string_lossy().to_string(),s:0,t,m:row.get(2)?,a:row.get::<_,Option<String>>(3)?.unwrap_or_else(||"main".into()),tool:"zcode".into(),i:count(6)?,inclusive:true,o:count(7)?,r,c};
        Ok((sid.clone(),format!("{sid}:{event}"),session,rec))
    })?;
    rows.collect()
}

/// 标题来自只读元数据库；仅匹配已有的会话 id，不导入其他会话或 token。
pub fn enrich_titles(sessions: &mut [Session],monitors: &[Value],warnings: &mut Vec<Value>) {
    let mut sources=std::collections::HashSet::new();
    for monitor in monitors {
        let agent=monitor["agent"].as_str().unwrap_or_default();
        if monitor["enabled"]!=true || !["codex","workbuddy","codebuddy"].contains(&agent) {continue;}
        let root=monitor["root"].as_str().unwrap_or_default();
        let mut dir=std::path::PathBuf::from(root);
        for _ in 0..5 {
            let mut candidates:Vec<_>=std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e|e.path()).filter(|p| {
                let name=p.file_name().unwrap_or_default().to_string_lossy();
                if agent=="codex" {name.strip_prefix("state_").and_then(|n|n.strip_suffix(".sqlite")).and_then(|n|n.parse::<u32>().ok()).is_some()}
                else {name==if agent=="workbuddy" {"workbuddy.db"} else {"codebuddy.db"}}
            }).collect();
            candidates.sort_by_key(|p|p.file_name().unwrap_or_default().to_string_lossy().strip_prefix("state_").and_then(|n|n.strip_suffix(".sqlite")).and_then(|n|n.parse::<u32>().ok()).unwrap_or(0));
            if let Some(file)=candidates.pop() {
                if !sources.insert((agent.to_string(),file.clone())) {break;}
                let result=(|| -> Result<(),rusqlite::Error> {
                    let db=rusqlite::Connection::open_with_flags(&file,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
                    let table=if agent=="codex" {"threads"} else {"sessions"};
                    let mut columns=db.prepare(&format!("PRAGMA table_info({table})"))?;
                    let columns:Vec<String>=columns.query_map([],|row|row.get::<_,String>(1))?.filter_map(Result::ok).collect();
                    let custom=columns.iter().any(|n|n=="custom_title");
                    let expression=if custom {"COALESCE(NULLIF(TRIM(custom_title),''),title)"} else {"title"};
                    let first=if columns.iter().any(|n|n=="first_user_message") {"first_user_message"} else {"NULL"};
                    let nickname=if columns.iter().any(|n|n=="agent_nickname") {"agent_nickname"} else {"NULL"};
                    let mut statement=db.prepare(&format!("SELECT id,{expression},{first},{nickname} FROM {table}"))?;
                    let titles:HashMap<String,(String,String,String)>=statement.query_map([],|row|Ok((row.get::<_,String>(0)?,(row.get::<_,Option<String>>(1)?.unwrap_or_default(),row.get::<_,Option<String>>(2)?.unwrap_or_default(),row.get::<_,Option<String>>(3)?.unwrap_or_default()))))?.filter_map(Result::ok).collect();
                    for session in sessions.iter_mut().filter(|s|s.tool==agent) {
                        if let Some((title,first,nickname))=titles.get(&session.id) {
                            if !title.trim().is_empty() {session.title=title.trim().to_string();}
                            else if session.title.is_empty() && !first.trim().is_empty() {session.title=format!("首条提问：{}",first.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect::<String>());}
                            else if session.title.is_empty() && !nickname.trim().is_empty() {session.title=format!("子 Agent：{}",nickname.trim());}
                        }
                    }
                    Ok(())
                })();
                if let Err(error)=result {warnings.push(json!({"agent":agent,"root":root,"file":file,"error":format!("会话标题读取失败：{error}")}));}
                break;
            }
            let Some(parent)=dir.parent() else {break;};dir=parent.to_path_buf();
        }
    }
}

/// OpenCode 新旧消息表汇总用量；不叠加 part 的 step-finish。
pub fn parse_opencode(root: &Path, warnings: &mut Vec<Value>) -> rusqlite::Result<Vec<(String,String,Session,Rec)>> {
    let mut sessions: HashMap<String,Session>=HashMap::new();
    let mut messages: Vec<(Value,String)>=Vec::new();
    let file=root.join("opencode.db");
    if file.exists() {
        let db=rusqlite::Connection::open_with_flags(&file,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let tables:std::collections::HashSet<String>=db.prepare("SELECT name FROM sqlite_master WHERE type='table'")?.query_map([],|r|r.get(0))?.collect::<rusqlite::Result<_>>()?;
        if !tables.contains("message") && !tables.contains("session_message") {return Err(rusqlite::Error::InvalidParameterName("不支持的 OpenCode 数据库结构：缺少消息表".into()));}
        for table in ["session","session_v2"].iter().filter(|t|tables.contains(**t)) {
            let mut stmt=db.prepare(&format!("SELECT id,directory,title FROM {table}"))?;
            for row in stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?)))? {
                let (id,cwd,title)=row?;sessions.insert(id.clone(),Session{id,cwd:cwd.unwrap_or_default(),title:title.unwrap_or_default(),tool:"opencode".into(),created_at:0,updated_at:0});
            }
        }
        for table in ["message","session_message"].iter().filter(|t|tables.contains(**t)) {
            let modern=*table=="session_message";
            let mut stmt=db.prepare(&format!("SELECT id,session_id,data,{} FROM {table}",if modern {"type"} else {"NULL"}))?;
            for row in stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?)))? {
                let (id,sid,data,role)=row?;
                let mut data:Value=match serde_json::from_str(&data) {Ok(v)=>v,Err(e)=>{warnings.push(json!({"agent":"opencode","root":root,"file":file,"error":format!("{table}/{id}：{e}")}));continue;}};
                if !data.is_object() {warnings.push(json!({"agent":"opencode","root":root,"file":file,"error":format!("{table}/{id}：消息格式无效")}));continue;}
                data["id"]=json!(id);data["sessionID"]=json!(sid);if let Some(role)=role {data["role"]=json!(role);}
                messages.push((data,file.to_string_lossy().to_string()));
            }
        }
    } else {
        fn walk(dir:&Path,root:&Path,depth:usize,out:&mut Vec<(Value,String)>,warnings:&mut Vec<Value>) -> std::io::Result<()> {
            if !dir.exists() || depth>4 {return Ok(());}
            for entry in std::fs::read_dir(dir)? {
                let entry=entry?;let kind=entry.file_type()?;let p=entry.path();
                if kind.is_dir() {if let Err(e)=walk(&p,root,depth+1,out,warnings) {warnings.push(json!({"agent":"opencode","root":root,"file":p,"error":e.to_string()}));}}
                else if kind.is_file() && p.extension().map(|e|e=="json").unwrap_or(false) {
                    match std::fs::read(&p).and_then(|b|serde_json::from_slice(&b).map_err(std::io::Error::from)) {
                        Ok(data)=>out.push((data,p.to_string_lossy().to_string())),
                        Err(e)=>warnings.push(json!({"agent":"opencode","root":root,"file":p,"error":e.to_string()})),
                    }
                }
            } Ok(())
        }
        let io_error=|e:std::io::Error|rusqlite::Error::ToSqlConversionFailure(Box::new(e));
        if !root.exists() {return Err(io_error(std::io::Error::new(std::io::ErrorKind::NotFound,"OpenCode 数据目录不存在")));}
        let storage=if root.join("storage").exists() {root.join("storage")} else {root.to_path_buf()};
        let mut metadata=Vec::new();walk(&storage.join("session"),root,0,&mut metadata,warnings).map_err(io_error)?;
        for (v,_) in metadata {if let Some(id)=v["id"].as_str() {sessions.insert(id.into(),Session{id:id.into(),cwd:v["directory"].as_str().unwrap_or_default().into(),title:v["title"].as_str().unwrap_or_default().into(),tool:"opencode".into(),created_at:to_epoch(&v["time"]["created"]),updated_at:to_epoch(&v["time"]["updated"])});}}
        walk(&storage.join("message"),root,0,&mut messages,warnings).map_err(io_error)?;
    }
    let mut out=Vec::new();
    for (m,origin) in messages {
        if m["role"]!="assistant" {continue;}
        let (Some(id),Some(sid))=(m["id"].as_str(),m["sessionID"].as_str()) else {continue;};
        let t=to_epoch(if token_count(&m["time"]["completed"])>0 {&m["time"]["completed"]} else {&m["time"]["created"]});
        let u=&m["tokens"];let i=token_count(&u["input"]);let o=token_count(&u["output"]);let r=token_count(&u["cache"]["read"]);let c=token_count(&u["cache"]["write"]);
        if t<=0 || i+o+r+c==0 {continue;}
        let session=sessions.entry(sid.into()).or_insert_with(||Session{id:sid.into(),cwd:m["path"]["cwd"].as_str().unwrap_or_default().into(),title:String::new(),tool:"opencode".into(),created_at:t,updated_at:t}).clone();
        let key=format!("{sid}:{id}");
        let rec=Rec{key:key.clone(),origin,s:0,t,m:m["modelID"].as_str().or_else(||m["model"]["id"].as_str()).unwrap_or("unknown").into(),a:m["agent"].as_str().or_else(||m["mode"].as_str()).unwrap_or("main").into(),tool:"opencode".into(),i,o,r,c,inclusive:false};
        out.push((sid.into(),key,session,rec));
    }
    Ok(out)
}
