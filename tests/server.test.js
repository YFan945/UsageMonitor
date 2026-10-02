'use strict';
const { test, before, after } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const net = require('node:net');
const { spawn } = require('node:child_process');
const { once } = require('node:events');

let fixture, child, base, root;
const request = (route, body) => fetch(base + route, body === undefined ? {} : {
  method: 'POST', headers: { 'Content-Type': 'application/json' },
  body: typeof body === 'string' ? body : JSON.stringify(body),
});
async function configure(roots) {
  const res = await request('/api/config', { roots, setupDone: true });
  assert.equal(res.status, 200);
  return res.json();
}
function makeSession(name, state, lines) {
  const dir = path.join(root, 'workspace', name);
  fs.mkdirSync(path.join(dir, 'agents', 'main'), { recursive: true });
  if (state) fs.writeFileSync(path.join(dir, 'state.json'), JSON.stringify(state));
  fs.writeFileSync(path.join(dir, 'agents', 'main', 'wire.jsonl'), lines.join('\n'));
}
const event = count => JSON.stringify({ type: 'usage.record', time: 1700000000000,
  model: 'demo', usage: { inputOther: count, output: 5 } });

function writeLog(relative, events) {
  const file = path.join(fixture, relative);
  fs.mkdirSync(path.dirname(file), {recursive:true});
  fs.writeFileSync(file, events.map(e => typeof e === 'string' ? e : JSON.stringify(e)).join('\n'));
  return path.dirname(file);
}

before(async () => {
  fixture = fs.mkdtempSync(path.join(os.tmpdir(), 'kimi-monitor-test-'));
  root = path.join(fixture, 'sessions');
  fs.copyFileSync(path.join(__dirname, '..', 'server.js'), path.join(fixture, 'server.js'));
  const seaExecutable = process.env.KIMI_MONITOR_TEST_EXE ? path.join(fixture, 'AgentMonitor.exe') : null;
  if (seaExecutable) fs.copyFileSync(process.env.KIMI_MONITOR_TEST_EXE, seaExecutable);
  fs.writeFileSync(path.join(fixture, 'config.json'), JSON.stringify({ roots: [], setupDone: true }));
  makeSession('a', { cwd: 'project-a', title: 'A' }, [event(3000000000), event(10).replace('"type":', '"type": '), '{broken']);
  makeSession('b', null, [event(20)]);
  fs.writeFileSync(path.join(root, 'session_index.jsonl'), JSON.stringify({ sessionId: 'b', workDir: 'project-b' }));
  const reservation = net.createServer();
  reservation.listen(0, '127.0.0.1');
  await once(reservation, 'listening');
  const port = reservation.address().port;
  await new Promise(resolve => reservation.close(resolve));
  base = `http://127.0.0.1:${port}`;
  child = spawn(seaExecutable || process.execPath,
    [...(seaExecutable ? [] : [path.join(fixture, 'server.js')]), '--port', String(port)], {
    cwd: fixture, stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, HOME: fixture, USERPROFILE: fixture, KIMI_MONITOR_CONFIG_DIR: fixture, KIMI_NO_TRAY: '1', KIMI_NO_OPEN: '1', KIMI_NO_SHORTCUT: '1' },
  });
  let logs = '';
  child.stdout.on('data', data => { logs += data; });
  child.stderr.on('data', data => { logs += data; });
  for (let attempt = 0; attempt < 100; attempt++) {
    if (child.exitCode !== null) throw new Error(`server exited: ${logs}`);
    try { if ((await request('/api/data')).ok) return; } catch { }
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  throw new Error(`server did not start: ${logs}`);
});
after(async () => {
  if (child && child.exitCode === null) {
    const stopped = once(child, 'exit');
    child.kill();
    await stopped;
  }
  if (fixture && path.dirname(fixture) === path.resolve(os.tmpdir()) && path.basename(fixture).startsWith('kimi-monitor-test-')) {
    fs.rmSync(fixture, { recursive: true, force: true });
  }
});

test('扫描合法 JSONL、保留大数，并通过索引恢复缺少 state.json 的项目', async () => {
  await configure([root]);
  const data = await (await request('/api/data')).json();
  assert.equal(data.sessions.length, 2);
  assert.equal(data.records.length, 3);
  assert.deepEqual(data.records.map(r => data.sessions[r.s].cwd).sort(), ['project-a', 'project-a', 'project-b']);
  assert.equal(data.records.find(r => r.i > 100).i, 3000000000);
});

test('重叠根目录及不同路径写法不会重复累计', async () => {
  await configure([root, path.join(root, 'workspace'), path.join(root, 'workspace', '..')]);
  const data = await (await request('/api/data')).json();
  assert.equal(data.sessions.length, 2);
  assert.equal(data.records.length, 3);
});

test('无效配置返回 400，原配置保留且服务继续运行', async () => {
  await configure([root]);
  for (const body of ['{bad', 'null', '{}', '{"roots":"abc"}', '{"roots":[1]}', '{"roots":[],"setupDone":"yes"}']) {
    assert.equal((await request('/api/config', body)).status, 400);
    const data = await (await request('/api/data')).json();
    assert.deepEqual(data.roots, [root]);
    assert.equal(data.records.length, 3);
  }
});

test('过大的配置请求返回 413，不修改配置', async () => {
  assert.equal((await request('/api/config', ' '.repeat(1024 * 1024 + 1))).status, 413);
  assert.deepEqual((await (await request('/api/data')).json()).roots, [root]);
});

test('非法 URL 编码返回 400，后续请求仍然可用', async () => {
  assert.equal((await request('/%ZZ')).status, 400);
  assert.equal((await request('/api/data')).status, 200);
});

test('多 Agent 日志：流式去重、Codex 累计差分、子 agent 与重叠路径', async () => {
  const assistant = (output, id='msg-1') => ({type:'assistant',sessionId:'claude-session',cwd:'claude-project',timestamp:'2026-10-02T10:00:00Z',message:{id,model:'claude-test',usage:{input_tokens:100,output_tokens:output,cache_read_input_tokens:20,cache_creation_input_tokens:5}}});
  const claudeRoot = writeLog('claude/project/main.jsonl',[assistant(2),'{broken',assistant(7)]);
  writeLog('claude/project/subagents/child.jsonl',[assistant(3,'msg-2')]);
  const tc = (input,output,cached) => ({type:'event_msg',timestamp:'2026-10-02T10:00:00Z',payload:{type:'token_count',info:{total_token_usage:{input_tokens:input,output_tokens:output,cached_input_tokens:cached}}}});
  const codexRoot = writeLog('codex/rollout.jsonl',[
    {type:'session_meta',payload:{id:'codex-session',cwd:'codex-project'}},
    {type:'turn_context',payload:{model:'gpt-test'}},tc(100,10,40),tc(100,10,40),tc(150,15,60),
  ]);
  const workbuddyRoot = writeLog('workbuddy/main.jsonl',[{...assistant(4),sessionId:'buddy-session',message:{id:'buddy-1',usage:{input_tokens:30,output_tokens:4,cache_read_input_tokens:8}},providerData:{model:'buddy-test'}}]);
  const importRoot = writeLog('imports/log.jsonl',[{type:'usage.record',time:1700000000000,sessionId:'import-session',eventId:'call-1',model:'deepseek-test',usage:{inputOther:9,output:2}}]);
  const monitors = [
    {agent:'kimi',root,enabled:true},
    {agent:'claude',root:claudeRoot,enabled:true},
    {agent:'claude',root:path.dirname(claudeRoot),enabled:true},
    {agent:'codex',root:codexRoot,enabled:true},
    {agent:'workbuddy',root:workbuddyRoot,enabled:true},
    ...['zcode','doubao','trae','dsh','codebuddy'].map(agent=>({agent,root:importRoot,enabled:agent!=='codebuddy'})),
  ];
  const res = await request('/api/config',{monitors,models:[],setupDone:true});
  assert.equal(res.status,200);
  const data = await (await request('/api/data')).json();
  assert.equal(data.agents.length,9);
  const claude = data.records.filter(r=>r.tool==='claude');
  assert.equal(claude.length,2);
  assert.equal(claude.reduce((n,r)=>n+r.o,0),10);
  const codex = data.records.filter(r=>r.tool==='codex');
  assert.equal(codex.length,2);
  assert.deepEqual(codex.map(r=>[r.i,r.o,r.r]),[[60,10,40],[30,5,20]]);
  assert.equal(data.records.filter(r=>r.tool==='workbuddy')[0].m,'buddy-test');
  for (const agent of ['zcode','doubao','trae','dsh']) assert.equal(data.records.filter(r=>r.tool===agent).length,1);
  for (const r of data.records) assert.equal(data.sessions[r.s].tool,r.tool);
  assert.equal(data.records.some(r=>r.tool==='codebuddy'),false);
  const again = await (await request('/api/data')).json();
  assert.equal(again.records.length,data.records.length);
});

test('模型白名单、关闭来源及配置持久化', async () => {
  const previous = await (await request('/api/data')).json();
  let res = await request('/api/config',{monitors:previous.monitors,models:['gpt-test']});
  assert.equal(res.status,200);
  let data = await (await request('/api/data')).json();
  assert.equal(data.records.length,2);
  assert.ok(data.records.every(r=>r.m==='gpt-test'));
  assert.ok(data.availableModels.includes('claude-test'));
  const saved = JSON.parse(fs.readFileSync(path.join(fixture,'config.json'),'utf8'));
  assert.deepEqual(saved.models,['gpt-test']);
  res = await request('/api/config',{monitors:previous.monitors.map(m=>({...m,enabled:false})),models:[]});
  assert.equal(res.status,200);
  data = await (await request('/api/data')).json();
  assert.equal(data.records.length,0);
  assert.equal(data.roots.length,0);
});

test('非法监测来源、目录与模型配置被拒绝且保留旧配置', async () => {
  const before = await (await request('/api/data')).json();
  for (const body of [
    {monitors:[{agent:'fake',root,enabled:true}]},
    {monitors:[{agent:'codex',root,enabled:'yes'}]},
    {monitors:[{agent:'codex',root:'',enabled:true}]},
    {monitors:[{agent:'codex',root:path.join(fixture,'missing'),enabled:true}]},
    {monitors:[],models:[1]},
  ]) assert.equal((await request('/api/config',body)).status,400);
  const after = await (await request('/api/data')).json();
  assert.deepEqual(after.monitors,before.monitors);
});

test('WorkBuddy function_call 按响应去重；ZCode 原生 model_io；DSH 最新格式与重试', async () => {
  const buddy = {type:'function_call',timestamp:'2026-10-02T10:00:00Z',sessionId:'buddy-native',message:{usage:{input_tokens:100,output_tokens:10,cache_read_input_tokens:40}},providerData:{messageId:'response-1',model:'buddy-native',rawUsage:{}}};
  const buddyRoot = writeLog('buddy-native/main.jsonl',[{...buddy,id:'tool-1'},{...buddy,id:'tool-2'}]);
  const zRoot = writeLog('zcode-native/model-io.jsonl',[{type:'model_io',completedAt:'2026-10-02T10:00:00Z',sessionId:'z-session',requestId:'req',attempt:0,response:{responseId:'z-response',modelId:'glm-test',usage:{inputTokens:100,outputTokens:10,totalTokens:110,cacheReadTokens:40}}}]);
  const msg = (seq,output,type='assistant/message') => ({type,seq,time:1790935200000,data:{turn:1,step:1,message:{source:{model:'deepseek-native'}},stream:[{type:'chunk',chunk:{type:'usage',usage:{inputTokens:20,outputTokens:output,cacheReadTokens:80}}}]}});
  const header = {type:'session',id:'dsh-native',version:4,cwd:'dsh-project'};
  const dRoot = writeLog('dsh-native/session/session.v4.jsonl',[header,msg(1,3),{type:'session/end-seed',seq:2,time:1790935200000,data:{}},msg(3,4,'assistant/attempt'),msg(4,5),{type:'llm/retry-started',seq:5,time:1790935200000,data:{}},msg(6,6)]);
  writeLog('dsh-native/session/session.v3.jsonl',[header,msg(1,999)]);
  const monitors = [{agent:'workbuddy',root:buddyRoot,enabled:true},{agent:'zcode',root:zRoot,enabled:true},{agent:'dsh',root:path.dirname(dRoot),enabled:true}];
  assert.equal((await request('/api/config',{monitors,models:[]})).status,200);
  const data = await (await request('/api/data')).json();
  const buddyRecords = data.records.filter(r=>r.tool==='workbuddy');
  assert.equal(buddyRecords.length,1); assert.equal(buddyRecords[0].i,60);
  const z = data.records.find(r=>r.tool==='zcode'); assert.deepEqual([z.i,z.o,z.r],[60,10,40]);
  const d = data.records.filter(r=>r.tool==='dsh');
  assert.equal(d.length,2);assert.equal(d.reduce((n,r)=>n+r.o,0),11);
  assert.ok(d.every(r=>r.i===20 && r.r===80));
  const zlib=require('node:zlib');
  if (zlib.zstdCompressSync) {
    fs.writeFileSync(path.join(dRoot,'session.v4.jsonl.zstd'),zlib.zstdCompressSync(fs.readFileSync(path.join(dRoot,'session.v4.jsonl'))));
    fs.unlinkSync(path.join(dRoot,'session.v4.jsonl'));
    await request('/api/config',{monitors,models:[]});
    const compressed = await (await request('/api/data')).json();
    assert.equal(compressed.records.filter(r=>r.tool==='dsh').reduce((n,r)=>n+r.o,0),11);
  }
});

test('ZCode 只读数据库：缓存口径、同源重复入口及读取异常隔离', async t => {
  let DatabaseSync; try { DatabaseSync = require('node:sqlite').DatabaseSync; } catch { t.skip('当前 Node 不提供内置 SQLite'); return; }
  const dbRoot = path.join(fixture,'zcode-db');fs.mkdirSync(dbRoot,{recursive:true});
  const file=path.join(dbRoot,'db.sqlite');const db=new DatabaseSync(file);
  db.exec(`CREATE TABLE session(id TEXT PRIMARY KEY,directory TEXT,title TEXT);
    CREATE TABLE model_usage(id TEXT PRIMARY KEY,session_id TEXT,model_id TEXT,agent TEXT,started_at INTEGER,completed_at INTEGER,input_tokens INTEGER,output_tokens INTEGER,cache_read_input_tokens INTEGER,cache_creation_input_tokens INTEGER);
    INSERT INTO session VALUES('session','project','title');
    INSERT INTO model_usage VALUES('call','session','glm','main',1790935200000,1790935200001,100,10,40,5);`);
  db.close();const bytes=fs.readFileSync(file);
  const monitors=[{agent:'zcode',root:dbRoot,enabled:true},{agent:'zcode',root:path.join(dbRoot,'.'),enabled:true}];
  await request('/api/config',{monitors,models:[]});
  const data=await (await request('/api/data')).json();
  assert.equal(data.records.length,1);
  assert.deepEqual([data.records[0].i,data.records[0].o,data.records[0].r,data.records[0].c],[55,10,40,5]);
  assert.deepEqual(fs.readFileSync(file),bytes);
  const broken=path.join(fixture,'broken-db');fs.mkdirSync(broken);fs.writeFileSync(path.join(broken,'db.sqlite'),'not a database');
  await request('/api/config',{monitors:[...monitors,{agent:'zcode',root:broken,enabled:true}],models:[]});
  const partial=await (await request('/api/data')).json();
  assert.equal(partial.records.length,1);assert.equal(partial.warnings.length,1);
});

test('大型多字节 JSONL 跨读取块仍正确统计，损坏尾行不丢失前面的记录', async () => {
  const nativeRoot=writeLog('long-log/session.jsonl',[
    {type:'assistant',sessionId:'long-session',timestamp:'2026-10-02T10:00:00Z',message:{id:'large-response',model:'long-model',content:'汉字🙂'.repeat(40000),usage:{input_tokens:100,output_tokens:3}}},
    '{"type":"assistant","message":{"usage":',
  ]);
  await request('/api/config',{monitors:[{agent:'claude',root:nativeRoot,enabled:true}],models:[]});
  const data=await (await request('/api/data')).json();
  assert.equal(data.records.length,1);assert.equal(data.records[0].o,3);
});


test('跨文件合并原始流式缓存、最终输出及 Codex 累计快照', async () => {
  const buddy=cache=>({type:'assistant',sessionId:'buddy-split',timestamp:'2026-10-02T10:00:00Z',message:{id:'response',model:'buddy',usage:{input_tokens:100,output_tokens:10,cache_read_input_tokens:cache}},providerData:{rawUsage:{}}});
  const buddyRoot=writeLog('split-buddy/a.jsonl',[buddy(0),buddy(40)]);writeLog('split-buddy/b.jsonl',[buddy(40)]);
  const claude=output=>({type:'assistant',sessionId:'claude-split',timestamp:'2026-10-02T10:00:00Z',message:{id:'response',model:'claude',usage:{input_tokens:10,output_tokens:output}}});
  const claudeRoot=writeLog('split-claude/a.jsonl',[claude(5)]);writeLog('split-claude/b.jsonl',[claude(10)]);
  const codex=(count,time)=>[{type:'session_meta',payload:{id:'codex-split'}},{type:'event_msg',timestamp:time,payload:{type:'token_count',info:{total_token_usage:{input_tokens:count}}}}];
  const codexRoot=writeLog('split-codex/z-first.jsonl',codex(100,'2026-10-02T10:00:00Z'));
  writeLog('split-codex/a-last.jsonl',codex(300,'2026-10-02T10:01:00Z'));
  writeLog('split-codex/b-duplicate.jsonl',codex(100,'2026-10-02T10:02:00Z'));
  const monitors=[{agent:'workbuddy',root:buddyRoot,enabled:true},{agent:'claude',root:claudeRoot,enabled:true},{agent:'codex',root:codexRoot,enabled:true}];
  await request('/api/config',{roots:[],monitors,models:[]});
  for(let scan=0;scan<2;scan++) {
    const data=await (await request('/api/data')).json();
    const b=data.records.find(r=>r.tool==='workbuddy');assert.deepEqual([b.i,b.o,b.r],[60,10,40]);
    assert.equal(data.records.find(r=>r.tool==='claude').o,10);
    const c=data.records.filter(r=>r.tool==='codex');assert.deepEqual(c.map(r=>r.i),[100,200]);
    assert.equal(data.records.length,4);assert.ok(data.records.every(r=>!('_inclusive' in r)));
  }
});

test('损坏压缩文件提示具体路径，其他文件继续统计，修复文件后自动恢复', async () => {
  const dRoot=writeLog('broken-compressed/healthy.jsonl',[{type:'usage.record',sessionId:'healthy',eventId:'call',model:'demo',time:1790935200000,usage:{inputOther:12}}]);
  const damaged=path.join(dRoot,'broken.jsonl.zstd');fs.writeFileSync(damaged,'broken');
  await request('/api/config',{roots:[],monitors:[{agent:'dsh',root:dRoot,enabled:true}],models:[]});
  const data=await (await request('/api/data')).json();
  assert.equal(data.records.length,1);assert.equal(data.warnings.length,1);assert.equal(data.warnings[0].file,fs.realpathSync(damaged));
  fs.unlinkSync(damaged);await request('/api/config',{roots:[],monitors:[{agent:'dsh',root:dRoot,enabled:true}],models:[]});
  assert.equal((await (await request('/api/data')).json()).warnings.length,0);
});

test('配置写入失败保留原文件与内存，成功替换保留有效备份', async () => {
  const file=path.join(fixture,'config.json'),temp=file+'.tmp';
  const before=fs.readFileSync(file);const config=await (await request('/api/data')).json();
  fs.mkdirSync(temp);
  try {assert.equal((await request('/api/config',{roots:[],monitors:[],models:['changed']})).status,500);}
  finally {fs.rmdirSync(temp);}
  assert.deepEqual(fs.readFileSync(file),before);
  assert.deepEqual((await (await request('/api/data')).json()).monitors,config.monitors);
  assert.equal((await request('/api/config',{roots:[],monitors:[],models:[]})).status,200);
  assert.deepEqual(fs.readFileSync(file+'.bak'),before);
  assert.doesNotThrow(()=>JSON.parse(fs.readFileSync(file,'utf8')));
});


test('会话标题读取只读数据库、自定义标题及首条提问，后续用量不覆盖标题', async t => {
  let DatabaseSync;try {DatabaseSync=require('node:sqlite').DatabaseSync;} catch {t.skip('当前 Node 无 SQLite');return;}
  const cRoot=writeLog('title-codex/sessions/thread.jsonl',[{type:'session_meta',payload:{id:'thread-title'}},{type:'event_msg',timestamp:'2026-10-02T10:00:00Z',payload:{type:'token_count',info:{total_token_usage:{input_tokens:100}}}}]);
  const cFile=path.join(path.dirname(cRoot),'state_5.sqlite'),cDb=new DatabaseSync(cFile);
  cDb.exec("CREATE TABLE threads(id TEXT,title TEXT);INSERT INTO threads VALUES('thread-title','Codex 数据库标题');");cDb.close();
  const assistant={type:'assistant',sessionId:'wb-title',timestamp:'2026-10-02T10:00:00Z',message:{id:'wb-call',model:'buddy',usage:{input_tokens:10,output_tokens:2}}};
  const wRoot=writeLog('title-workbuddy/projects/thread.jsonl',[assistant]);
  const wFile=path.join(path.dirname(wRoot),'workbuddy.db'),wDb=new DatabaseSync(wFile);
  wDb.exec("CREATE TABLE sessions(id TEXT,title TEXT,custom_title TEXT);INSERT INTO sessions VALUES('wb-title','自动标题','用户自定义标题');");wDb.close();
  const clRoot=writeLog('title-claude/main.jsonl',[{type:'user',sessionId:'cl-title',message:{content:'第一个问题'}},{...assistant,sessionId:'cl-title'}, {type:'custom-title',sessionId:'cl-title',customTitle:'Claude 重命名标题'}]);
  writeLog('title-claude/fallback.jsonl',[{type:'user',sessionId:'fallback-title',message:{content:[{type:'text',text:'没有正式标题的提问'}]}},{...assistant,sessionId:'fallback-title'}]);
  const before=[fs.readFileSync(cFile),fs.readFileSync(wFile)];
  await request('/api/config',{roots:[],monitors:[{agent:'codex',root:cRoot,enabled:true},{agent:'workbuddy',root:wRoot,enabled:true},{agent:'claude',root:clRoot,enabled:true}],models:[]});
  const data=await (await request('/api/data')).json(),titles=new Map(data.sessions.map(s=>[s.id,s.title]));
  assert.equal(titles.get('thread-title'),'Codex 数据库标题');assert.equal(titles.get('wb-title'),'用户自定义标题');
  assert.equal(titles.get('cl-title'),'Claude 重命名标题');assert.equal(titles.get('fallback-title'),'首条提问：没有正式标题的提问');
  assert.deepEqual(fs.readFileSync(cFile),before[0]);assert.deepEqual(fs.readFileSync(wFile),before[1]);assert.equal(data.warnings.length,0);
});

test('DSH 拼接压缩帧完整解码，普通恢复标记保留用量，继承历史不重复计入', async t => {
  const zlib=require('node:zlib');if(!zlib.zstdCompressSync) {t.skip('Node 缺少 Zstandard');return;}
  const time=1790935200000;
  const usage=(step,at)=>({type:'assistant/message',time:at,data:{turn:1,step,usage:{inputTokens:20,outputTokens:5},message:{source:{model:'deepseek'}}}});
  const dRoot=path.join(fixture,'dsh-frames');fs.mkdirSync(dRoot,{recursive:true});
  const frames=[[{type:'session',id:'original',isSeeded:false,createdAt:time-1000}], [usage(1,time),{type:'session/end-seed',time:time+1,data:{}}],[usage(2,time+2),{type:'session/title',data:{title:'DSH 真实标题'}},{type:'session/end-seed',time:time+3,data:{}}]];
  const file=path.join(dRoot,'session.v4.jsonl.zstd');fs.writeFileSync(file,Buffer.concat(frames.map(rows=>zlib.zstdCompressSync(rows.map(e=>JSON.stringify(e)).join('\n')+'\n'))));
  writeLog('dsh-fork/session.v4.jsonl',[{type:'session',id:'fork',isSeeded:true,createdAt:time},usage(1,time-10),{type:'session/end-seed',data:{inherited:true}},usage(2,time+1),{type:'session/end-seed',data:{}}]);
  await request('/api/config',{roots:[],monitors:[{agent:'dsh',root:dRoot,enabled:true},{agent:'dsh',root:path.join(fixture,'dsh-fork'),enabled:true}],models:[]});
  const data=await (await request('/api/data')).json();assert.equal(data.records.length,3);assert.equal(data.records.reduce((n,r)=>n+r.o,0),15);
  assert.equal(data.sessions.find(s=>s.id==='original').title,'DSH 真实标题');assert.equal(data.warnings.length,0);
});
