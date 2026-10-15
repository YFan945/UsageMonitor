'use strict';
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {spawn} = require('node:child_process');
const {once} = require('node:events');
let DatabaseSync;try {DatabaseSync=require('node:sqlite').DatabaseSync;} catch {}

test('OpenCode SQLite/旧 JSON 与其他来源落盘，删除日志及重启后保留，筛选不会销毁历史', async t => {
  if (!DatabaseSync) {t.skip('需要 Node 内置 SQLite');return;}
  const dir = fs.mkdtempSync(path.join(os.tmpdir(),'agentmonitor-history-'));
  let child, base;
  const write = (file, data) => {const target=path.join(dir,file);fs.mkdirSync(path.dirname(target),{recursive:true});fs.writeFileSync(target,data);};
  const get = async () => (await fetch(base+'/api/data')).json();
  const configure = async body => {const response=await fetch(base+'/api/config',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});assert.equal(response.status,200);};
  const stop = async () => {const stopped=once(child,'exit');child.kill();await stopped;};
  const start = async () => {
    const reservation=require('node:net').createServer();reservation.listen(0,'127.0.0.1');await once(reservation,'listening');const port=reservation.address().port;await new Promise(resolve=>reservation.close(resolve));
    base='http://127.0.0.1:'+port;
    child=spawn(process.execPath,[path.resolve(__dirname,'../server.js'),'--port',String(port)],{env:{...process.env,AGENT_MONITOR_CONFIG_DIR:dir,HOME:dir,USERPROFILE:dir,KIMI_NO_TRAY:'1',KIMI_NO_OPEN:'1',KIMI_NO_SHORTCUT:'1'},stdio:['ignore','pipe','pipe']});
    let logs='';child.stderr.on('data',b=>{logs+=b;});
    for(let n=0;n<200;n++) {
      if(child.exitCode!==null) throw new Error(logs);
      try {await get();return;} catch {}
      await new Promise(r=>setTimeout(r,25));
    }
    throw new Error('服务未启动 '+logs);
  };
  try {
    const oc=path.join(dir,'oc');fs.mkdirSync(oc);
    const database=path.join(oc,'opencode.db');const db=new DatabaseSync(database);
    db.exec('CREATE TABLE session(id TEXT,directory TEXT,title TEXT);CREATE TABLE message(id TEXT,session_id TEXT,data TEXT);CREATE TABLE part(id TEXT,data TEXT);');
    db.prepare('INSERT INTO session VALUES(?,?,?)').run('oc-session','project','OpenCode 标题');
    const message={role:'assistant',modelID:'oc-model',agent:'build',time:{created:1790935200000,completed:1790935200001},tokens:{input:20,output:10,reasoning:3,cache:{read:80,write:5}}};
    db.prepare('INSERT INTO message VALUES(?,?,?)').run('msg','oc-session',JSON.stringify(message));
    db.prepare('INSERT INTO message VALUES(?,?,?)').run('broken','oc-session','{bad');
    db.prepare('INSERT INTO part VALUES(?,?)').run('step',JSON.stringify({type:'step-finish',tokens:message.tokens}));db.close();
    const original=fs.readFileSync(database);
    const modernRoot=path.join(dir,'modern');fs.mkdirSync(modernRoot);
    const modern=new DatabaseSync(path.join(modernRoot,'opencode.db'));
    modern.exec('CREATE TABLE session_v2(id TEXT,directory TEXT,title TEXT);CREATE TABLE session_message(id TEXT,session_id TEXT,type TEXT,data TEXT);');
    modern.prepare('INSERT INTO session_v2 VALUES(?,?,?)').run('modern-session','modern-project','新版标题');
    const modernMessage={...message,modelID:undefined,role:undefined,model:{id:'modern-model',providerID:'test'}};
    modern.prepare('INSERT INTO session_message VALUES(?,?,?,?)').run('modern-msg','modern-session','assistant',JSON.stringify(modernMessage));
    modern.prepare('INSERT INTO session_message VALUES(?,?,?,?)').run('user','modern-session','user',JSON.stringify(modernMessage));modern.close();
    write('legacy/storage/session/project/s.json',JSON.stringify({id:'legacy',directory:'old',title:'旧版标题'}));
    write('legacy/storage/message/legacy/m.json',JSON.stringify({...message,id:'old-msg',sessionID:'legacy'}));
    write('legacy/storage/message/legacy/broken.json','{bad');
    const codex = (input,t) => JSON.stringify({type:'event_msg',timestamp:t,payload:{type:'token_count',info:{total_token_usage:{input_tokens:input,cached_input_tokens:input/2,output_tokens:10}}}});
    write('codex/a.jsonl',[JSON.stringify({type:'session_meta',payload:{id:'codex-session'}}),codex(100,'2026-10-02T10:00:00Z'),codex(200,'2026-10-02T10:01:00Z')].join('\n'));
    write('kimi/session/agents/main/wire.jsonl',JSON.stringify({type:'usage.record',time:1790935200000,model:'kimi-model',usage:{inputOther:9,output:2}}));
    const monitors=[{agent:'opencode',root:oc,enabled:true},{agent:'opencode',root:modernRoot,enabled:true},{agent:'opencode',root:path.join(dir,'legacy'),enabled:true},{agent:'codex',root:path.join(dir,'codex'),enabled:true}];
    const cfg={roots:[path.join(dir,'kimi')],monitors,models:[],setupDone:true};write('config.json',JSON.stringify(cfg));
    await start();let data=await get();assert.equal(data.records.length,6);assert.ok(data.warnings.some(w=>w.agent==='opencode' && w.error.includes('broken')));
    assert.deepEqual(data.records.filter(r=>r.tool==='opencode').map(r=>[r.i,r.o,r.r,r.c]),[[20,10,80,5],[20,10,80,5],[20,10,80,5]]);
    assert.deepEqual(fs.readFileSync(database),original);
    assert.ok(data.records.some(r=>r.m==='modern-model'));
    assert.ok(data.sessions.some(s=>s.title==='新版标题'));
    assert.equal(data.records.filter(r=>r.tool==='codex').reduce((n,r)=>n+r.i+r.r,0),200);
    const expected=data.records.map(r=>[r.tool,r.i,r.o,r.r,r.c]).sort();
    // 流式同一条消息增加输出，记录数不变。
    const update=new DatabaseSync(database);update.prepare("UPDATE message SET data=? WHERE id='msg'").run(JSON.stringify({...message,tokens:{...message.tokens,output:15}}));update.close();
    await configure(cfg);data=await get();assert.equal(data.records.length,6);assert.equal(data.records.filter(r=>r.tool==='opencode').reduce((n,r)=>n+r.o,0),35);
    const updated=data.records.map(r=>[r.tool,r.i,r.o,r.r,r.c]).sort();assert.notDeepEqual(updated,expected);
    for(const file of ['oc/opencode.db','modern/opencode.db','legacy/storage/message/legacy/m.json','legacy/storage/session/project/s.json','codex/a.jsonl','kimi/session/agents/main/wire.jsonl']) fs.unlinkSync(path.join(dir,file));
    await configure({...cfg,models:['absent']});assert.equal((await get()).records.length,0);
    await configure(cfg);assert.deepEqual((await get()).records.map(r=>[r.tool,r.i,r.o,r.r,r.c]).sort(),updated);
    assert.ok((await get()).sessions.some(s=>s.title==='旧版标题'));
    await stop();await start();assert.deepEqual((await get()).records.map(r=>[r.tool,r.i,r.o,r.r,r.c]).sort(),updated);
    await configure({roots:[],monitors:[],models:[]});assert.equal((await get()).records.length,0);
    await configure(cfg);assert.equal((await get()).records.length,6);
    // 主文件损坏时从备份读取，避免静默覆盖已有档案。
    await stop();fs.copyFileSync(path.join(dir,'usage-history.json'),path.join(dir,'usage-history.json.bak'));write('usage-history.json','broken');await start();assert.equal((await get()).records.length,6);
    // 后台扫描无需前端请求；新消息在下一次定时采集后进入档案。
    write('legacy/storage/message/legacy/background.json',JSON.stringify({...message,id:'background',sessionID:'legacy'}));
    const deadline=Date.now()+35000;
    let collected=false;
    while(Date.now()<deadline) {
      const saved=JSON.parse(fs.readFileSync(path.join(dir,'usage-history.json'),'utf8'));
      if(saved.entries.some(e=>e[1].record._key==='legacy:background')) {collected=true;break;}
      await new Promise(resolve=>setTimeout(resolve,250));
    }
    assert.ok(collected,'30 秒后台采集必须落盘');
    fs.unlinkSync(path.join(dir,'legacy/storage/message/legacy/background.json'));
    await stop();await start();assert.equal((await get()).records.length,7);
    // 写盘失败报告异常，恢复目录后即使源日志已删除仍可重试保存。
    fs.mkdirSync(path.join(dir,'usage-history.json.tmp'));
    write('legacy/storage/message/legacy/retry.json',JSON.stringify({...message,id:'retry',sessionID:'legacy'}));
    await configure(cfg);assert.ok((await get()).warnings.some(w=>w.agent==='history' && w.error.includes('保存失败')));
    fs.unlinkSync(path.join(dir,'legacy/storage/message/legacy/retry.json'));
    fs.rmdirSync(path.join(dir,'usage-history.json.tmp'));await configure(cfg);assert.equal((await get()).records.length,8);
    await stop();await start();assert.equal((await get()).records.length,8);
  } finally {
    if(child && child.exitCode===null) await stop();
    fs.rmSync(dir,{recursive:true,force:true});
  }
});
