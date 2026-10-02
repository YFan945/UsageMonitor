'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const html = fs.readFileSync(path.join(__dirname, '..', 'public', 'index.html'), 'utf8');

function context() {
  const elements = new Map();
  const ctx = vm.createContext({
    $: key => { if (!elements.has(key)) elements.set(key, {}); return elements.get(key); },
    document: { querySelectorAll: () => [] },
    renderLegend() {}, drawHeatmap() {}, drawCharts() {}, drawBreakdowns() {}, drawSessions() {}, saveUI() {},
  });
  // 使用页面中的实际函数，避免复制实现后只测试副本。
  vm.runInContext(html.slice(html.indexOf('const p2 ='), html.indexOf('// ---------- 主题')), ctx);
  vm.runInContext(html.match(/function pct\([^\n]+/)[0], ctx);
  vm.runInContext('let curPreset="24h", range={}; const DATA={records:[],sessions:[]}; const fltModel=new Set(), fltProj=new Set(), fltAgent=new Set(); const agentName=id=>id;', ctx);
  vm.runInContext(html.slice(html.indexOf('function updatePresetRange('), html.indexOf("$('#presets').addEventListener")), ctx);
  vm.runInContext(html.slice(html.indexOf('function render()'), html.indexOf('// ---------- 数据目录管理')), ctx);
  return ctx;
}

test('大数格式化不发生 32 位溢出', () => {
  const ctx = context();
  assert.equal(vm.runInContext('fmt(3000000000)', ctx), '30.00亿');
  assert.equal(vm.runInContext('fmtFull(3000000000)', ctx), '3,000,000,000');
  assert.equal(vm.runInContext('fmtFull(4294967296)', ctx), '4,294,967,296');
  assert.equal(vm.runInContext('fmt(12345)', ctx), '1.2万');
});

test('渲染会移动 24 小时窗口，并在跨午夜后更新今天', () => {
  const ctx = context();
  vm.runInContext(`
    const RealDate=Date;
    let clock=new RealDate(2026,9,2,23,59).getTime();
    Date=class extends RealDate { constructor(...args){super(...(args.length?args:[clock]));} static now(){return clock;} };
    render();
  `, ctx);
  const first = vm.runInContext('range.start', ctx);
  vm.runInContext('$("#from").value="2026-10-01T10:00"; $("#to").value="2026-10-02T11:00";', ctx);
  vm.runInContext('clock+=120000; render()', ctx);
  assert.equal(vm.runInContext('range.start', ctx), first + 120000);
  assert.equal(vm.runInContext('$("#from").value', ctx), '2026-10-01T10:00');
  assert.equal(vm.runInContext('$("#to").value', ctx), '2026-10-02T11:00');
  vm.runInContext('curPreset="today"; render()', ctx);
  assert.equal(vm.runInContext('range.start', ctx), vm.runInContext('new RealDate(2026,9,3).getTime()', ctx));
  vm.runInContext('curPreset="custom"; range={start:100,end:200}; render()', ctx);
  assert.equal(vm.runInContext('range.start', ctx), 100);
  assert.equal(vm.runInContext('range.end', ctx), 200);
});

test('完整内联脚本语法有效', () => {
  assert.doesNotThrow(() => new vm.Script(html.match(/<script>([\s\S]*?)<\/script>/)[1]));
});

test('相邻同色柱块增加对比色分隔线，非相邻同色不加线且保持高度', () => {
  const ctx = context();
  vm.runInContext(html.slice(html.indexOf('function stackedSegments('), html.indexOf('function drawCharts(')), ctx);
  const output = vm.runInContext("stackedSegments([['a',10],['b',20],['c',30],['d',40]],v=>v,k=>k==='c'?'#3b82f6':'#ff5252')",ctx);
  assert.equal((output.match(/box-shadow/g) || []).length,1);
  assert.match(output,/height:20%;background:#ff5252;box-shadow:inset 0 -1px 0 #172033/);
  assert.match(output,/height:40%;background:#ff5252;"/);
  assert.match(vm.runInContext("stackedSegments([['a',10],['b',20]],v=>v,()=> '#171717')",ctx), /box-shadow:inset 0 -1px 0 #ffffff/);
});

test('Agent 多选与模型筛选共同生效，清空 Agent 选择恢复全部', () => {
  const ctx = context();
  vm.runInContext(`curPreset='all';DATA.records=[
    {s:0,tool:'codex',m:'gpt',t:100,i:10,o:2,r:0,c:0},
    {s:0,tool:'claude',m:'opus',t:100,i:20,o:3,r:0,c:0},
    {s:0,tool:'kimi',m:'kimi',t:100,i:30,o:4,r:0,c:0}];
    fltAgent.add('codex');fltAgent.add('claude');render();`,ctx);
  assert.match(vm.runInContext('$("#rangeInfo").textContent',ctx), /共 2 次/);
  vm.runInContext(`fltModel.add('gpt');render()`,ctx);
  assert.match(vm.runInContext('$("#rangeInfo").textContent',ctx), /共 1 次/);
  vm.runInContext('fltAgent.clear();fltModel.clear();render()',ctx);
  assert.match(vm.runInContext('$("#rangeInfo").textContent',ctx), /共 3 次/);
});

function rankingContext() {
  const ctx = context();
  vm.runInContext('const agentColor=()=>"#58a6ff", modelColor=()=>"#ff5252", categoryColor=()=>"#22c55e";', ctx);
  vm.runInContext(html.slice(html.indexOf('function aggregateRanking('), html.indexOf('function drawBreakdowns(')), ctx);
  vm.runInContext(html.slice(html.indexOf('function pieSlices('), html.indexOf('function renderDetails(')), ctx);
  return ctx;
}

test('柱状图图例仅包含当前时段和交叉筛选后有可见 Token 的模型及 Agent', () => {
  const ctx = context();
  vm.runInContext(`let barMode='model';const modelOrder=['current','old','cache-only'];const modelColor=()=> '#58a6ff',agentColor=()=> '#3fb950';
    DATA.agents=[{id:'codex',name:'Codex'},{id:'claude',name:'Claude Code'},{id:'kimi',name:'Kimi Code'}];
    DATA.sessions=[{cwd:'project-a'},{cwd:'project-b'}];
    DATA.records=[{s:0,tool:'codex',m:'current',t:150,i:10,o:2,r:0,c:0},
      {s:0,tool:'claude',m:'old',t:50,i:10,o:2,r:0,c:0},
      {s:1,tool:'kimi',m:'cache-only',t:150,i:0,o:0,r:100,c:0}];
    curPreset='custom';range={start:100,end:200};$('#includeCr').checked=true;`, ctx);
  vm.runInContext(html.slice(html.indexOf('function renderLegend('), html.indexOf('function syncFilterControls(')), ctx);
  vm.runInContext('render()', ctx);
  assert.match(vm.runInContext("$('#legend').innerHTML", ctx), /current/);
  assert.doesNotMatch(vm.runInContext("$('#legend').innerHTML", ctx), /old/);
  vm.runInContext("fltProj.add('project-a');render()", ctx);
  assert.doesNotMatch(vm.runInContext("$('#legend').innerHTML", ctx), /cache-only/);
  vm.runInContext("barMode='agent';render()", ctx);
  assert.match(vm.runInContext("$('#legend').innerHTML", ctx), /Codex/);
  assert.doesNotMatch(vm.runInContext("$('#legend').innerHTML", ctx), /Claude Code|Kimi Code/);
  vm.runInContext("fltProj.clear();fltModel.add('cache-only');$('#includeCr').checked=false;render()", ctx);
  assert.equal(vm.runInContext("$('#legend').innerHTML", ctx), '');
  vm.runInContext("$('#includeCr').checked=true;render()", ctx);
  assert.match(vm.runInContext("$('#legend').innerHTML", ctx), /Kimi Code/);
  vm.runInContext('range={start:300,end:400};render()', ctx);
  assert.equal(vm.runInContext("$('#legend').innerHTML", ctx), '');
});

test('完整排行保留超过 8 项，Agent 聚合与缓存开关口径一致', () => {
  const ctx = rankingContext();
  vm.runInContext(`DATA.sessions=[{cwd:'project-a',title:'session-a'}];
    DATA.records=Array.from({length:12},(_,i)=>({s:0,m:'model-'+i,tool:i%2?'claude':'codex',t:100+i,i:10,o:2,r:100,c:3}));`, ctx);
  assert.equal(vm.runInContext("aggregateRanking(DATA.records,'model',true).length", ctx), 12);
  assert.equal(vm.runInContext("aggregateRanking(DATA.records,'agent',true)[0].weight", ctx), 690);
  assert.equal(vm.runInContext("aggregateRanking(DATA.records,'agent',false)[0].weight", ctx), 90);
  assert.equal(vm.runInContext("aggregateRanking(DATA.records,'sessions',true)[0].calls", ctx), 12);
  assert.equal(vm.runInContext("aggregateRanking(DATA.records,'proj',true)[0].fullName", ctx), 'project-a');
});

test('饼图合并小占比和超过前 7 项的用量，总量不丢失且零值无饼图', () => {
  const ctx = rankingContext();
  vm.runInContext(`const entries=[50,20,10,8,4,3,2,2,1,0].map((weight,i)=>({fullName:'item-'+i,weight}));const slices=pieSlices(entries);`, ctx);
  assert.equal(vm.runInContext('slices.length', ctx), 8);
  assert.equal(vm.runInContext('slices.reduce((s,e)=>s+e.weight,0)', ctx), 100);
  assert.equal(vm.runInContext('slices[7].weight', ctx), 3);
  assert.equal(vm.runInContext('slices[7].count', ctx), 2);
  assert.equal(vm.runInContext("pieSlices([{fullName:'zero',weight:0}]).length", ctx), 0);
  assert.equal(vm.runInContext('pieSlices([]).length', ctx), 0);
});


test('模型颜色在新增、移除和刷新后稳定，排行饼图使用统一配色', () => {
  const values=new Map();
  const create=()=>{
    const ctx=vm.createContext({localStorage:{getItem:k=>values.get(k)||null,setItem:(k,v)=>values.set(k,v)}});
    vm.runInContext(html.slice(html.indexOf('const PALETTE ='),html.indexOf('// ---------- 格式化')),ctx);
    return ctx;
  };
  const ctx=create();
  const red=vm.runInContext("modelColor('first')",ctx);assert.equal(red,'#ff5252');
  vm.runInContext("modelColor('new'); categoryColor('projects','project');",ctx);
  assert.equal(vm.runInContext("modelColor('first')",ctx),red);
  assert.equal(vm.runInContext("modelColor('third')",ctx),'#ffdf00');
  assert.equal(vm.runInContext("modelColor('first')",create()),red);
  assert.match(html,/detailKind==='model' \? modelColor\(s.key\)/);
  assert.match(html,/outline:1px solid var\(--text\);outline-offset:-1px/);
});

test('发布脚本只指向扩展仓库且不覆盖同名资产', () => {
  const build=fs.readFileSync(path.join(__dirname,'..','build.js'),'utf8');
  assert.match(build,/const REPO = 'YFan945\/UsageMonitor'/);
  assert.doesNotMatch(build,/--clobber/);
});


test('选择 Agent 后模型和项目排行同步变化，Agent 排行保留全部来源', () => {
  const ctx=rankingContext();
  vm.runInContext('let rankData={}; function hbars(){};',ctx);
  vm.runInContext(html.slice(html.indexOf('function drawBreakdowns('),html.indexOf('// 全量明细')),ctx);
  vm.runInContext(`DATA.sessions=[{cwd:'codex-project'},{cwd:'claude-project'}];
    const rows=[{s:0,tool:'codex',m:'gpt',i:100,o:10,r:0,c:0,t:100},{s:1,tool:'claude',m:'opus',i:200,o:20,r:0,c:0,t:100}];
    fltAgent.add('codex');drawBreakdowns(rows);`,ctx);
  assert.equal(vm.runInContext('rankData.model.length',ctx),1);
  assert.equal(vm.runInContext('rankData.model[0].key',ctx),'gpt');
  assert.equal(vm.runInContext('rankData.proj[0].key',ctx),'codex-project');
  assert.equal(vm.runInContext('rankData.agent.length',ctx),2);
  vm.runInContext("fltAgent.clear();drawBreakdowns(rows)",ctx);
  assert.equal(vm.runInContext('rankData.model.length',ctx),2);
});

test('统计前两张卡片不显示冗余说明', () => {
  const ctx=context();vm.runInContext('render()',ctx);
  const output=vm.runInContext('$("#stats").innerHTML',ctx);
  assert.doesNotMatch(output,/按用量记录|两类缓存/);
  assert.doesNotMatch(output.split('<div class="stat"')[1],/class="s"/);
});
