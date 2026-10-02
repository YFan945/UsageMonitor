'use strict';
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const vm = require('node:vm');

test('配置改名迁移保留旧文件，损坏主配置使用备份，不覆盖新目录设置', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(),'agent-migration-'));
  try {
    const target = path.join(root,'new'), legacy = path.join(root,'old');
    fs.mkdirSync(target); fs.mkdirSync(legacy);
    const source = fs.readFileSync(path.join(__dirname,'..','server.js'),'utf8');
    const ctx = vm.createContext({fs,path,target,legacy});
    vm.runInContext(source.slice(source.indexOf('function migrateLegacyConfig('),source.indexOf('// 仅迁移默认 macOS')),ctx);
    fs.writeFileSync(path.join(legacy,'config.json'),'corrupt');
    const saved = JSON.stringify({setupDone:true,monitors:[],models:['gpt-5.5']});
    fs.writeFileSync(path.join(legacy,'config.json.bak'),saved);
    vm.runInContext('migrateLegacyConfig(target,legacy)',ctx);
    assert.equal(fs.readFileSync(path.join(target,'config.json'),'utf8'),saved);
    assert.equal(fs.readFileSync(path.join(legacy,'config.json'),'utf8'),'corrupt');
    assert.equal(fs.readFileSync(path.join(legacy,'config.json.bak'),'utf8'),saved);
    fs.writeFileSync(path.join(target,'config.json'),'{}');
    vm.runInContext('migrateLegacyConfig(target,legacy)',ctx);
    assert.equal(fs.readFileSync(path.join(target,'config.json'),'utf8'),'{}');
    fs.unlinkSync(path.join(target,'config.json'));
    fs.writeFileSync(path.join(target,'config.json.bak'),'{}');
    vm.runInContext('migrateLegacyConfig(target,legacy)',ctx);
    assert.equal(fs.existsSync(path.join(target,'config.json')),false);
  } finally { fs.rmSync(root,{recursive:true,force:true}); }
});
