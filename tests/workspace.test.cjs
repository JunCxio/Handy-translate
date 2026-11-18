const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

class Element {
  constructor() {
    this.value = ''; this.hidden = false; this.textContent = ''; this.children = []; this.events = {};
    const classes = new Set();
    this.classList = {
      add: name => classes.add(name), remove: name => classes.delete(name),
      toggle: (name, active) => active ? classes.add(name) : classes.delete(name),
      contains: name => classes.has(name),
    };
  }
  addEventListener(name, action) { this.events[name] = action; }
  setAttribute() {}
  removeAttribute() {}
  reportValidity() { return true; }
  dispatchEvent(event) { this.events[event.type]?.(event); }
  querySelectorAll(selector) {
    const types = selector.split(',');
    return this.children.flatMap(child => [...(types.includes(child.tagName) ? [child] : []), ...child.querySelectorAll(selector)]);
  }
  replaceChildren(...children) { this.children = children; }
  append(...children) { this.children.push(...children); }
  querySelector() { return new Element(); }
  showModal() { this.open = true; }
  close() { this.open = false; }
}

async function settle() {
  for (let i = 0; i < 8; i++) await new Promise(setImmediate);
}

async function setup(settings = {}) {
  const nodes = {}, windowEvents = {}, polls = [], calls = [];
  const html = fs.readFileSync(path.join(__dirname, '../web/index.html'), 'utf8');
  for (const tag of html.matchAll(/<[^>]*\bid="([^"]+)"[^>]*>/g)) {
    const element = nodes[tag[1]] = new Element();
    element.hidden = /\bhidden\b/.test(tag[0]);
    element.value = /\bvalue="([^"]*)"/.exec(tag[0])?.[1] || '';
  }
  const body = new Element();
  const catalog = { title: '测试游戏', root: 'D:\\ゲーム', engine: 'RPG Maker MZ', entries: 1, translated: 0, failed: 0, warnings: [], source_language: 'ja', target_language: 'zh-CN' };
  const controls = { catalog, busy: false, scanError: '', selected: '', scanGate: null, localError: '', settings: { preset:'custom', auth_mode:'api_key', provider:'openai', base_url:'https://api.openai.com/v1', model:'', api_key:'', source_language:'ja', target_language:'zh-CN', concurrency:4, batch_size:48, remember_key:false, ...settings } };
  const storage = { getItem: () => null, setItem() {}, removeItem() {} };
  controls.entries = [{ id:'one', original:'こんにちは', contexts:['System.json'] }];
  controls.terms = [];
  controls.translateError = '';
  controls.imageProject = null;
  controls.ocr = {language:'ja',available:false,installable:true,running:false,reboot_required:false,message:'本机没有日语 OCR 语言包'};
  controls.imageInstallation = {installed:false,record:false,images:0};
  const imageAsset = {id:'image-one',path:'img/pictures/help.png',hash:'hash',references:['Map001.json：显示图片事件'],loaded:false,width:200,height:100,regions:[],recognized:null,rendered:null};
  const context = vm.createContext({
    document: { getElementById: id => nodes[id], createElement: tag => Object.assign(new Element(),{tagName:tag}), body, hidden: false },
    window: { addEventListener: (name, action) => { windowEvents[name] = action; } },
    location: { hash: '#test-token' }, history: { replaceState() {} },
    sessionStorage: { ...storage, getItem: () => 'test-token' }, localStorage: storage,
    URLSearchParams, setInterval: action => polls.push(action), setTimeout, clearTimeout,
    URL:{createObjectURL:()=> 'blob:test',revokeObjectURL(){}}, Event:class {constructor(type){this.type=type;}},
    fetch: async (url, options) => {
      calls.push({ url, body: options.body && JSON.parse(options.body) });
      assert.equal(options.headers['X-Session-Token'], 'test-token');
      let value;
      if (url === '/api/state') value = { catalog: controls.catalog, progress: { running: false, total: 0, processed: 0, message: '' }, busy: controls.busy };
      else if (url === '/api/settings') {
        if (options.body) { controls.settings = JSON.parse(options.body); value = { ok:true, message:'配置已保存' }; }
        else value = controls.settings;
      } else if (url === '/api/models') value = { models:['test-model','other-model'] };
      else if (url === '/api/test-service') value = { ok:true, message:'连接成功' };
      else if (url === '/api/local-config') {
        if (controls.localError) return { ok:false, json:async () => ({error:controls.localError}) };
        const claude = JSON.parse(options.body).preset === 'claude_code';
        value = {provider:claude?'anthropic':'responses',auth_mode:claude?'bearer':'api_key',base_url:'https://local.test/v1',model:'local-model',api_key:'local-secret',model_options:['local-model']};
      }
      else if (url === '/api/scan') {
        if (controls.scanGate) await controls.scanGate;
        if (controls.scanError) return { ok: false, json: async () => ({ error: controls.scanError }) };
        const request = JSON.parse(options.body);
        controls.catalog.source_language = request.source_language === 'auto' ? 'ja' : request.source_language;
        controls.catalog.target_language = request.target_language;
        value = { ok: true };
      } else if (url === '/api/pick-game') value = { path: controls.selected };
      else if (url === '/api/glossary') {
        if (options.body) { controls.terms = JSON.parse(options.body).terms; value = {ok:true,message:'术语表已保存'}; }
        else value = {terms:controls.terms};
      } else if (url === '/api/review') value = {checked:1,pending:0,flagged:1};
      else if (url === '/api/launch') value = {ok:true};
      else if (url === '/api/translate') {
        if (controls.translateError) return {ok:false,json:async () => ({error:controls.translateError})};
        value = {ok:true};
      } else if (url === '/api/images/state') value={project:controls.imageProject,installation:controls.imageInstallation,progress:{running:false},ocr:controls.ocr};
      else if (url === '/api/images/ocr') value=controls.ocr;
      else if (url === '/api/images/ocr/install') {
        if (controls.ocrInstallError) return {ok:false,json:async()=>({error:controls.ocrInstallError})};
        controls.busy=true;controls.ocr.running=true;controls.ocr.message='等待系统授权或正在下载、安装日语 OCR 包';value={ok:true};
      }
      else if (url === '/api/images/scan') {controls.imageProject={assets:[structuredClone(imageAsset)],warnings:[]};value={count:1};}
      else if (url.startsWith('/api/images/preview?')) return {ok:true,blob:async()=>({})};
      else if (url === '/api/images/edit') {controls.imageProject.assets[0].regions=JSON.parse(options.body).regions;value={ok:true};}
      else if (url === '/api/images/trace') {controls.imageInstallation={...controls.imageInstallation,installed:true,record:JSON.parse(options.body).enabled};value={message:'设置已保存'};}
      else if (url === '/api/images/uninstall') {controls.imageInstallation={installed:false,record:false,images:0};value={message:'已还原'};}
      else if (url === '/api/restore-installed') {controls.catalog.translated=controls.catalog.entries;controls.entries[0].translation='已恢复译文';value={restored:1,message:'已恢复游戏内译文'};}
      else if (url === '/api/images/process' || url === '/api/images/stop') value={ok:true};
      else if (url.startsWith('/api/entries?')) value = { page: 0, total: controls.entries.length, entries:controls.entries };
      else throw new Error('Unexpected route: ' + url);
      return { ok: true, json: async () => JSON.parse(JSON.stringify(value)) };
    },
  });
  vm.runInContext(fs.readFileSync(path.join(__dirname, '../web/app.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(__dirname, '../web/images.js'), 'utf8'), context);
  await settle();
  return {
    nodes, controls, calls, body,
    drag: async detail => { windowEvents['game-file-drag']({ detail }); await settle(); },
    click: async id => { await nodes[id].events.click(); await settle(); },
    poll: async () => { for (const poll of polls) await poll(); await settle(); },
    scans: () => calls.filter(call => call.url === '/api/scan'),
  };
}

test('图片模块默认关闭，进入后仍需主动扫描与选择才处理', async () => {
  const app=await setup();await app.drag({kind:'drop',paths:['D:\\Game.exe']});
  assert.equal(app.nodes['image-workspace'].hidden,true);assert.equal(app.calls.some(c=>c.url.startsWith('/api/images/')),false);
  await app.click('image-mode');assert.equal(app.nodes['text-workspace'].hidden,true);assert.equal(app.nodes['image-workspace'].hidden,false);assert.equal(app.controls.imageProject,null);
  assert.equal(app.calls.some(c=>c.url==='/api/images/scan'),false);
  await app.click('image-scan');assert.equal(app.controls.imageProject.assets.length,1);assert.equal(app.calls.some(c=>c.url==='/api/images/process'),false);
  await app.click('image-select-visible');assert.equal(app.nodes['image-selection-count'].textContent,'已选 1 张');
  app.nodes['image-ocr-mode'].value='local';await app.click('image-recognize');
  const process=app.calls.find(c=>c.url==='/api/images/process');assert.equal(process.body.mode,'local');assert.deepEqual(process.body.ids,['image-one']);assert.equal(process.body.action,'recognize');
  await app.click('image-trace');assert.equal(app.controls.imageInstallation.record,true);await app.click('image-uninstall');assert.equal(app.controls.imageInstallation.installed,false);assert.equal(app.calls.some(c=>c.url==='/api/uninstall'),false);
});

test('图片区域草稿必须保存，换游戏后回到文本模块', async () => {
  const app=await setup();await app.drag({kind:'drop',paths:['D:\\Game.exe']});await app.click('image-mode');await app.click('image-scan');
  const row=app.nodes['image-list'].children[0];await row.children[1].events.click();await settle();await app.click('image-add-region');
  assert.equal(app.nodes['image-save'].disabled,false);await app.click('text-mode');assert.equal(app.nodes['text-workspace'].hidden,true);
  const fieldset=app.nodes['image-regions'].children[0];const input=fieldset.children[1].children[0].children[0];input.value='Help';input.events.input();
  await app.click('image-save');assert.equal(app.controls.imageProject.assets[0].regions[0].original,'Help');await app.click('text-mode');assert.equal(app.nodes['text-workspace'].hidden,false);
  await app.click('image-mode');await app.drag({kind:'drop',paths:['D:\\OtherGame.exe']});assert.equal(app.nodes['image-workspace'].hidden,true);assert.equal(app.nodes['text-workspace'].hidden,false);
});

test('新加载游戏优先本地 OCR，明确文字去重和视觉用量', async () => {
  const app=await setup();await app.drag({kind:'drop',paths:['D:\\Game.exe']});
  assert.equal(app.nodes['image-ocr-mode'].value,'local');
  assert.match(app.nodes['image-ocr-hint'].textContent,/汇总去重并查缓存/);
  app.nodes['image-ocr-mode'].value='vision';app.nodes['image-ocr-mode'].events.change();
  assert.match(app.nodes['image-ocr-hint'].textContent,/产生识别用量/);
  await app.drag({kind:'drop',paths:['D:\\OtherGame.exe']});
  assert.equal(app.nodes['image-ocr-mode'].value,'local');
  assert.equal(app.calls.some(c=>c.url==='/api/images/process'),false);
});

test('缺少日语 OCR 时由用户发起安装，等待期间禁用操作，取消后可重试', async () => {
  const app=await setup();await app.drag({kind:'drop',paths:['D:\\Game.exe']});await app.click('image-mode');
  assert.equal(app.nodes['image-ocr-package'].hidden,false);
  assert.equal(app.nodes['image-ocr-install'].hidden,false);
  assert.equal(app.calls.some(c=>c.url==='/api/images/ocr/install'),false);
  await app.click('image-ocr-install');
  assert.match(app.nodes['image-ocr-status'].textContent,/系统授权/);
  assert.equal(app.nodes['image-ocr-install'].disabled,true);
  assert.equal(app.nodes['image-scan'].disabled,true);
  assert.equal(app.calls.some(c=>c.url==='/api/images/process'||c.url==='/api/translate'),false);
  app.controls.busy=false;app.controls.ocr.running=false;app.controls.ocr.message='已取消系统管理员授权';await app.poll();
  assert.match(app.nodes['image-ocr-status'].textContent,/已取消/);
  assert.equal(app.nodes['image-ocr-install'].disabled,false);
  await app.click('image-ocr-install');
  app.controls.busy=false;Object.assign(app.controls.ocr,{available:true,installable:false,running:false,message:'日语 OCR 包已安装并通过检测'});await app.poll();
  assert.equal(app.nodes['image-ocr-install'].hidden,true);
  assert.equal(app.nodes['image-ocr-package'].classList.contains('ready'),true);
  assert.equal(app.calls.filter(c=>c.url==='/api/images/ocr/install').length,2);
  assert.equal(app.calls.some(c=>c.url==='/api/images/process'),false);
});

test('OCR 检测支持手动刷新，视觉模式隐藏安装区，安装请求失败仍可重试', async () => {
  const app=await setup();await app.drag({kind:'drop',paths:['D:\\Game.exe']});await app.click('image-mode');
  const checks=app.calls.filter(c=>c.url==='/api/images/ocr').length;
  await app.click('image-ocr-check');assert.equal(app.calls.filter(c=>c.url==='/api/images/ocr').length,checks+1);
  app.nodes['image-ocr-mode'].value='vision';app.nodes['image-ocr-mode'].events.change();await settle();
  assert.equal(app.nodes['image-ocr-package'].hidden,true);
  app.nodes['image-ocr-mode'].value='local';app.nodes['image-ocr-mode'].events.change();await settle();
  assert.equal(app.nodes['image-ocr-package'].hidden,false);
  app.controls.ocrInstallError='无法启动系统安装';await app.click('image-ocr-install');
  assert.match(app.nodes.notice.textContent,/无法启动系统安装/);
  assert.equal(app.nodes['image-ocr-install'].disabled,false);
  Object.assign(app.controls.ocr,{language:'en',installable:false,message:'请在 Windows 可选功能安装对应组件'});
  app.controls.catalog.source_language='en';await app.click('image-ocr-check');
  assert.equal(app.nodes['image-ocr-install'].hidden,true);
});

test('全部图片一键处理不受选择限制，已检出文字筛选排除未识别和无字图片', async () => {
  const app=await setup();await app.drag({kind:'drop',paths:['D:\\Game.exe']});await app.click('image-mode');
  assert.equal(app.nodes['image-auto'].disabled,true);
  await app.click('image-scan');
  const base=app.controls.imageProject.assets[0];
  app.controls.imageProject.assets=[base,{...base,id:'empty',recognized:'done'},{...base,id:'text',regions:[{original:'Help'}]}];
  await app.poll();
  app.nodes['image-text-only'].checked=true;app.nodes['image-text-only'].events.change();
  assert.equal(app.nodes['image-list'].children.length,2);
  app.nodes['image-ocr-mode'].value='vision';await app.click('image-auto');
  const call=app.calls.find(c=>c.url==='/api/images/process');
  assert.equal(call.body.scope,'all');assert.equal(call.body.action,'auto');assert.equal(call.body.mode,'vision');assert.equal(call.body.ids,undefined);
  assert.equal(app.nodes['image-selection-count'].textContent,'已选 0 张');
});

test('游戏内译文可恢复，恢复后清除未翻译筛选并显示译文', async () => {
  const app=await setup();app.controls.catalog.installed=true;
  app.controls.catalog.installation={translated:1,language:'zh-CN',current:false};
  await app.drag({kind:'drop',paths:['D:\\Game.exe']});
  assert.equal(app.nodes['restore-button'].hidden,false);
  app.nodes.filter.value='pending';app.nodes.search.value='query';await app.click('restore-button');
  assert.equal(app.nodes.filter.value,'');assert.equal(app.nodes.search.value,'');
  assert.equal(app.nodes['restore-button'].hidden,true);assert.match(app.nodes.counts.textContent,/已保存 1/);
  app.controls.catalog.translated=0;app.controls.catalog.installation.language='ko';await app.poll();
  assert.equal(app.nodes['restore-button'].hidden,true);
});

test('启动时隐藏缓存项目，拖入后才提取并显示工作台', async () => {
  const app = await setup();
  assert.equal(app.nodes.welcome.hidden, false);
  assert.equal(app.nodes.workbench.hidden, true);
  assert.equal(app.calls.some(call => call.url.startsWith('/api/entries')), false);
  await app.drag({ kind: 'enter' });
  assert.equal(app.body.classList.contains('file-dragging'), true);
  await app.drag({ kind: 'drop', paths: ['D:\\ゲーム\\Game.exe'] });
  assert.deepEqual(app.scans()[0].body, { path: 'D:\\ゲーム\\Game.exe', source_language: 'ja', target_language: 'zh-CN' });
  assert.equal(app.nodes.workbench.hidden, false);
  assert.equal(app.nodes.welcome.hidden, true);
  assert.equal(app.body.classList.contains('file-dragging'), false);
  assert.equal(app.nodes['text-rows'].children.length, 1);
});

test('启动游戏位于顶部游戏信息区，文本与图片共用且任务期间禁用', async () => {
  const html=fs.readFileSync(path.join(__dirname,'../web/index.html'),'utf8');
  assert.equal((html.match(/id="launch-button"/g)||[]).length,1);
  const heading=html.slice(html.indexOf('<section class="game-section"'),html.indexOf('<div class="workspace-navigation">'));
  assert.match(heading,/id="launch-button"[^>]*class="secondary game-launch"/);
  const app=await setup();app.controls.catalog.can_launch=true;
  await app.drag({kind:'drop',paths:['D:\\Game.exe']});
  assert.equal(app.nodes['launch-button'].disabled,false);
  await app.click('image-mode');assert.equal(app.nodes['launch-button'].disabled,false);
  await app.click('launch-button');assert.equal(app.calls.filter(c=>c.url==='/api/launch').length,1);
  app.controls.busy=true;await app.poll();assert.equal(app.nodes['launch-button'].disabled,true);
  app.controls.busy=false;app.controls.catalog.can_launch=false;await app.poll();
  assert.equal(app.nodes['launch-button'].disabled,true);
});

test('多个文件和识别失败都留在入口，不显示翻译内容', async () => {
  const app = await setup();
  await app.drag({ kind: 'drop', paths: ['a.exe', 'b.exe'] });
  assert.equal(app.scans().length, 0);
  assert.match(app.nodes.notice.textContent, /每次/);
  app.controls.scanError = '不支持该文件';
  await app.drag({ kind: 'drop', paths: ['archive.zip'] });
  assert.equal(app.nodes.workbench.hidden, true);
  assert.equal(app.nodes.welcome.hidden, false);
  assert.equal(app.nodes.notice.textContent, '不支持该文件');
  assert.equal(app.nodes['welcome-pick'].disabled, false);
});

test('目录也可拖入，更换游戏后重新隐藏工作台', async () => {
  const app = await setup();
  await app.drag({ kind: 'drop', paths: ['D:\\Games\\Folder'] });
  await app.drag({ kind: 'enter' });
  assert.equal(app.nodes['drop-overlay'].hidden, false);
  await app.drag({ kind: 'leave' });
  assert.equal(app.nodes['drop-overlay'].hidden, true);
  await app.click('change-game');
  assert.equal(app.nodes.workbench.hidden, true);
  assert.equal(app.nodes.welcome.hidden, false);
});

test('选择程序自动加载，取消文件选择仍停留在入口', async () => {
  const app = await setup();
  await app.click('welcome-pick');
  assert.equal(app.scans().length, 0);
  assert.equal(app.nodes.workbench.hidden, true);
  app.controls.selected = 'D:\\Game.exe';
  await app.click('welcome-pick');
  assert.equal(app.scans().length, 1);
  assert.equal(app.nodes.workbench.hidden, false);
});

test('读取期间不重复加载，翻译期间拒绝切换游戏', async () => {
  const app = await setup();
  let release;
  app.controls.scanGate = new Promise(resolve => { release = resolve; });
  await app.drag({ kind: 'drop', paths: ['first.exe'] });
  await app.drag({ kind: 'drop', paths: ['second.exe'] });
  assert.equal(app.scans().length, 1);
  release(); await settle();
  app.controls.busy = true;
  await app.poll();
  await app.drag({ kind: 'drop', paths: ['third.exe'] });
  assert.equal(app.scans().length, 1);
  assert.equal(app.nodes.workbench.hidden, false);
  assert.match(app.nodes.notice.textContent, /停止后/);
});

test('Responses 显示并要求模型名称，切换到 DeepL 后隐藏模型字段', async () => {
  const app = await setup();
  app.nodes.provider.value = 'responses';
  app.nodes.provider.events.change();
  assert.equal(app.nodes['model-field'].hidden, false);
  assert.equal(app.nodes['model-presets'].required, true);
  assert.equal(app.nodes['base-url'].value, 'https://api.openai.com/v1');
  assert.match(app.nodes['url-hint'].textContent, /Responses/);
  app.nodes.provider.value = 'deepl';
  app.nodes.provider.events.change();
  assert.equal(app.nodes['model-field'].hidden, true);
  assert.equal(app.nodes['api-key'].required, true);
});

test('从本地恢复配置和加密密钥，切换大模型协议保留自定义地址', async () => {
  const app = await setup({ model:'test-model', base_url:'https://example.test/v1', api_key:'restored-key', remember_key:true, concurrency:3, batch_size:32 });
  assert.equal(app.nodes.model.value,'test-model');
  assert.equal(app.nodes['api-key'].value,'restored-key');
  assert.equal(app.nodes['remember-key'].checked,true);
  assert.equal(app.nodes.workbench.hidden,true);
  app.nodes.provider.value = 'responses'; app.nodes.provider.events.change();
  assert.equal(app.nodes['base-url'].value,'https://example.test/v1');
  assert.equal(app.nodes['api-key'].value,'restored-key');
  await app.click('save-settings');
  assert.equal(app.controls.settings.concurrency,3);
  assert.equal(app.controls.settings.remember_key,true);
});

test('获取模型后可选择模型，并显示未安装译文也可恢复的保存进度', async () => {
  const app = await setup();
  await app.click('fetch-models');
  assert.equal(app.nodes['model-presets'].children.length,4);
  assert.equal(app.nodes.model.value,'');
  app.nodes['model-presets'].value = 'test-model';
  app.nodes['model-presets'].events.change();
  assert.equal(app.nodes.model.value,'test-model');
  await app.click('test-service');
  assert.equal(app.nodes['config-status'].textContent,'连接成功');
  app.controls.catalog.translated = 1;
  await app.drag({kind:'drop',paths:['Game.exe']});
  assert.match(app.nodes.notice.textContent,/恢复 1 条译文/);
  assert.match(app.nodes['progress-number'].textContent,/本地已保存 1 \/ 1/);
  assert.equal(app.nodes['translate-button'].disabled,true);
});

test('保存模型下拉列表和选择，重开后恢复；自定义名称按需填写', async () => {
  const app = await setup({ model:'other-model', model_options:['test-model','other-model'] });
  assert.equal(app.nodes['model-presets'].value,'other-model');
  assert.equal(app.nodes['model-presets'].children.length,4);
  assert.equal(app.nodes['custom-model-field'].hidden,true);
  app.nodes['model-presets'].value = '__custom__';
  app.nodes['model-presets'].events.change();
  assert.equal(app.nodes['custom-model-field'].hidden,false);
  assert.equal(app.nodes.model.required,true);
  app.nodes.model.value = 'custom-model';
  await app.click('save-settings');
  const reopened = await setup(app.controls.settings);
  assert.equal(reopened.nodes['model-presets'].value,'custom-model');
  assert.equal(reopened.nodes['model-presets'].children.length,5);
  assert.equal(reopened.nodes.model.value,'custom-model');
});

test('DeepSeek 预设填写地址和模型，切换服务时清除原密钥', async () => {
  const app = await setup({api_key:'old-key'});
  app.nodes['service-preset'].value = 'deepseek';
  await app.nodes['service-preset'].events.change();
  assert.equal(app.nodes['api-key'].value,'');
  assert.equal(app.nodes['base-url'].value,'https://api.deepseek.com');
  assert.equal(app.nodes.model.value,'deepseek-flash');
  assert.equal(app.nodes.provider.disabled,true);
  assert.equal(app.nodes['api-key'].readOnly,false);
  await app.click('save-settings');
  assert.equal(app.controls.settings.preset,'deepseek');
});

test('Codex 与 Claude Code 读取本机配置，保存来源且不另存密钥', async () => {
  const app = await setup();
  for (const preset of ['codex','claude_code']) {
    app.nodes['service-preset'].value = preset;
    await app.nodes['service-preset'].events.change();
    assert.equal(app.nodes.provider.value,preset==='codex'?'responses':'anthropic');
    assert.equal(app.nodes['api-key'].value,'local-secret');
    assert.equal(app.nodes['api-key'].readOnly,true);
    assert.equal(app.nodes['remember-key-field'].hidden,true);
    assert.equal(app.nodes['local-config-field'].hidden,false);
    await app.poll();
    assert.equal(app.nodes.provider.disabled,true);
    await app.click('save-settings');
    assert.equal(app.controls.settings.preset,preset);
    assert.equal(app.controls.settings.remember_key,false);
  }
  assert.equal(app.nodes['auth-mode'].value,'bearer');
});

test('读取失败和重启后来源改变时清空密钥并给出说明', async () => {
  const app = await setup({api_key:'old-key'});
  app.controls.localError = '本机未配置 API 密钥';
  app.nodes['service-preset'].value = 'claude_code';
  await app.nodes['service-preset'].events.change();
  assert.equal(app.nodes['api-key'].value,'');
  assert.match(app.nodes['config-status'].textContent,/未配置/);
  assert.equal(app.nodes['read-local-config'].disabled,false);
  const restored = await setup({preset:'codex',provider:'responses',config_warning:'地址已改变，请重新读取'});
  assert.equal(restored.nodes['api-key'].value,'');
  assert.match(restored.nodes['config-status'].textContent,/地址已改变/);
  assert.equal(restored.nodes['base-url'].readOnly,true);
});

test('源语言可自动识别，目标语言有名称列表并恢复自定义代码', async () => {
  const app = await setup({source_language:'auto'});
  assert.equal(app.nodes['source-language'].value,'auto');
  assert.equal(app.nodes['target-language'].children.length,31);
  await app.drag({kind:'drop',paths:['Game.exe']});
  assert.equal(app.scans()[0].body.source_language,'auto');
  assert.match(app.nodes['language-status'].textContent,/自动识别为日语/);
  await app.click('save-settings');
  assert.equal(app.controls.settings.source_language,'auto');
  const custom = await setup({target_language:'es-MX'});
  assert.equal(custom.nodes['target-language'].value,'__custom__');
  assert.equal(custom.nodes['custom-target-language'].value,'es-MX');
  assert.equal(custom.nodes['custom-target-language'].hidden,false);
});

test('修改语言后必须应用再翻译或安装，失败时仍保留旧项目', async () => {
  const app = await setup();
  app.controls.catalog.translated = 1;
  await app.drag({kind:'drop',paths:['Game.exe']});
  app.nodes['target-language'].value = 'fr';
  app.nodes['target-language'].events.change();
  assert.equal(app.nodes['apply-languages'].hidden,false);
  assert.equal(app.nodes['translate-button'].disabled,true);
  assert.equal(app.nodes['install-button'].disabled,true);
  app.controls.scanError = '加载失败';
  await app.click('apply-languages');
  assert.equal(app.controls.catalog.target_language,'zh-CN');
  assert.equal(app.nodes['apply-languages'].hidden,false);
  app.controls.scanError = '';
  await app.click('apply-languages');
  assert.equal(app.controls.catalog.target_language,'fr');
  assert.equal(app.nodes['apply-languages'].hidden,true);
  assert.match(app.nodes['language-status'].textContent,/法语/);
});

test('完整文本进度仍提示图片范围，并区分游戏内译文是否同步', async () => {
  const app = await setup({api_key:'key',model:'test-model'});
  assert.equal(app.nodes['connection-settings'].open,false);
  app.controls.catalog.translated = 1;
  app.controls.catalog.installed = true;
  app.controls.catalog.installation = {translated:1,language:'zh-CN',current:true};
  await app.drag({kind:'drop',paths:['Game.exe']});
  assert.match(app.nodes['coverage-title'].textContent,/图片文字不计入进度/);
  assert.equal(app.nodes['install-button'].textContent,'已安装最新译文');
  assert.equal(app.nodes['install-button'].disabled,true);
  app.controls.catalog.installation.current = false;
  await app.poll();
  assert.equal(app.nodes['install-button'].disabled,false);
  assert.match(app.nodes['install-hint'].textContent,/点击更新/);
  app.controls.catalog.failed = 1;
  await app.poll();
  await app.click('filter-failed');
  assert.equal(app.nodes.filter.value,'failed');
});

test('术语表读取、校验和保存，错误输入不关闭对话框', async () => {
  const app = await setup();
  await app.drag({kind:'drop',paths:['Game.exe']});
  app.controls.terms = [{source:'結衣',target:'结衣'}];
  await app.click('glossary-button');
  assert.equal(app.nodes['glossary-dialog'].open,true);
  assert.equal(app.nodes['glossary-text'].value,'結衣 = 结衣');
  app.nodes['glossary-text'].value = '格式错误';
  await app.nodes['glossary-form'].events.submit({preventDefault(){}});
  assert.equal(app.nodes['glossary-dialog'].open,true);
  assert.match(app.nodes['glossary-error'].textContent,/原文 = 译文/);
  app.nodes['glossary-text'].value = '結衣 = 结衣\nPotion = 药水';
  await app.nodes['glossary-form'].events.submit({preventDefault(){}});
  assert.equal(app.nodes['glossary-dialog'].open,false);
  assert.deepEqual(app.controls.terms,[{source:'結衣',target:'结衣'},{source:'Potion',target:'药水'}]);
});

test('本页多选重译携带补充要求，缓存偏好可保存', async () => {
  const app = await setup({use_cache:false,model:'test-model'});
  await app.drag({kind:'drop',paths:['Game.exe']});
  assert.equal(app.nodes['use-cache'].checked,false);
  app.nodes['select-page'].checked = true;
  app.nodes['select-page'].events.change();
  assert.equal(app.nodes['selection-count'].textContent,'已选 1 条');
  await app.click('retranslate-button');
  assert.equal(app.nodes['retranslate-dialog'].open,true);
  app.nodes['retranslate-instructions'].value = '保留敬语';
  await app.nodes['retranslate-form'].events.submit({preventDefault(){}});
  const request = app.calls.find(call => call.url === '/api/translate').body;
  assert.deepEqual(request.ids,['one']);
  assert.equal(request.instructions,'保留敬语');
  assert.equal(request.use_cache,false);
  assert.equal(app.controls.settings.use_cache,false);
  assert.equal(app.nodes['retranslate-dialog'].open,false);
  assert.equal(app.nodes['selection-count'].textContent,'已选 0 条');
});

test('重译失败仍保留对话框和选择，语言未应用时阻止重译', async () => {
  const app = await setup({model:'test-model'});
  await app.drag({kind:'drop',paths:['Game.exe']});
  app.nodes['select-page'].checked = true; app.nodes['select-page'].events.change();
  await app.click('retranslate-button');
  app.controls.translateError = '服务不可用';
  await app.nodes['retranslate-form'].events.submit({preventDefault(){}});
  assert.equal(app.nodes['retranslate-dialog'].open,true);
  assert.equal(app.nodes['retranslate-error'].textContent,'服务不可用');
  assert.equal(app.nodes['selection-count'].textContent,'已选 1 条');
  app.nodes['target-language'].value = 'ko'; app.nodes['target-language'].events.change();
  assert.equal(app.nodes['retranslate-button'].disabled,true);
  assert.equal(app.nodes['glossary-button'].disabled,true);
});

test('译文检查显示人工确认条目，更换游戏清空选择', async () => {
  const app = await setup();
  app.controls.entries[0].issues = ['疑似残留日文'];
  await app.drag({kind:'drop',paths:['Game.exe']});
  await app.click('review-button');
  assert.equal(app.nodes.filter.value,'review');
  assert.match(app.nodes.notice.textContent,/1 条需人工确认/);
  const target = app.nodes['text-rows'].children[0].children[1];
  assert.equal(target.children.some(child => child.textContent === '疑似残留日文'),true);
  app.nodes['select-page'].checked = true; app.nodes['select-page'].events.change();
  await app.click('change-game');
  assert.equal(app.nodes['selection-count'].textContent,'已选 0 条');
});
