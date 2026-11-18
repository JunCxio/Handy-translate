'use strict';
const $ = id => document.getElementById(id);
const fragment = location.hash.slice(1);
if (fragment) { sessionStorage.setItem('session-token', fragment); history.replaceState(null, '', '/'); }
const token = sessionStorage.getItem('session-token') || '';
let state = null, page = 0, requestSequence = 0, lastProgress = '', searching;
let entered = false, loading = false;
const settingFields = { preset:'service-preset', provider:'provider', auth_mode:'auth-mode', base_url:'base-url', model:'model', api_key:'api-key', source_language:'source-language', target_language:'target-language', concurrency:'concurrency', batch_size:'batch-size' };
let previousProvider = '';
let modelOptions = [];
let configurationBusy = false;
let loadedLanguages = null;
const selectedEntries = new Set();
let visibleEntries = [], rowControls = [], retranslateIds = [];
const languages = [['zh-CN','简体中文'],['zh-TW','繁体中文'],['en','英语'],['ja','日语'],['ko','韩语'],['fr','法语'],['de','德语'],['es','西班牙语'],['pt','葡萄牙语'],['pt-BR','葡萄牙语（巴西）'],['it','意大利语'],['ru','俄语'],['uk','乌克兰语'],['pl','波兰语'],['nl','荷兰语'],['cs','捷克语'],['sv','瑞典语'],['fi','芬兰语'],['da','丹麦语'],['nb','挪威语'],['tr','土耳其语'],['vi','越南语'],['th','泰语'],['id','印度尼西亚语'],['ar','阿拉伯语'],['el','希腊语'],['he','希伯来语'],['hi','印地语'],['ro','罗马尼亚语'],['hu','匈牙利语']];
const languageName = code => languages.find(([value]) => value === code)?.[1] || code;
function languageValue(id) { return $(id).value === '__custom__' ? $('custom-' + id).value.trim() : $(id).value; }
function setLanguage(id, value) {
  const known = languages.some(([code]) => code === value) || (id === 'source-language' && value === 'auto');
  $(id).value = known ? value : '__custom__';
  $('custom-' + id).value = known ? '' : value;
}
for (const id of ['source-language','target-language']) {
  const choices = id === 'source-language' ? [['auto','自动识别（推荐）'], ...languages] : languages;
  for (const [value,text] of [...choices,['__custom__','其他语言…']]) {
    const option = document.createElement('option'); option.value = value; option.textContent = text; $(id).append(option);
  }
  setLanguage(id, id === 'source-language' ? 'auto' : 'zh-CN');
}
function languageChanges() {
  return !!(entered && loadedLanguages && (loadedLanguages.source !== languageValue('source-language') || loadedLanguages.target !== languageValue('target-language')));
}
function languageStatus() {
  const c = entered ? state?.catalog : null, dirty = languageChanges();
  for (const id of ['source-language','target-language']) {
    $('custom-' + id).hidden = $(id).value !== '__custom__';
    $('custom-' + id).required = ! $('custom-' + id).hidden;
  }
  $('apply-languages').hidden = !dirty;
  $('apply-languages').disabled = loading || !!state?.busy;
  $('language-status').textContent = dirty ? '语言已更改。点击“应用语言”切换项目；已有译文会保留。' : c ? `${loadedLanguages?.source === 'auto' ? '自动识别为' : '源语言：'}${languageName(c.source_language)}，目标：${languageName(c.target_language)}。` : '源语言在本地识别；目标语种是否可用取决于翻译服务。';
  $('language-status').classList.toggle('changed', dirty);
  $('translate-button').disabled = dirty || !!state?.busy || configurationBusy || !c || c.translated === c.entries;
  $('restore-button').disabled = dirty || !!state?.busy || loading;
  $('install-button').disabled = dirty || !!state?.busy || !c?.translated || !!c?.installation?.current;
  translationTools();
}
function translationTools() {
  const blocked = !entered || !state?.catalog || !!state?.busy || loading || languageChanges() || configurationBusy;
  $('glossary-button').disabled = blocked;
  $('glossary-button').textContent = state?.catalog?.glossary_terms ? `术语表 ${state.catalog.glossary_terms}` : '术语表';
  $('review-button').disabled = blocked;
  $('retranslate-button').disabled = blocked || !selectedEntries.size;
  $('clear-selection').disabled = blocked || !selectedEntries.size;
  $('select-page').disabled = blocked || !visibleEntries.length;
  const count = visibleEntries.filter(id => selectedEntries.has(id)).length;
  $('select-page').checked = !!count && count === visibleEntries.length;
  $('select-page').indeterminate = count > 0 && count < visibleEntries.length;
  $('selection-count').textContent = `已选 ${selectedEntries.size} 条`;
  for (const [checkbox, retranslate, id] of rowControls) {
    checkbox.checked = selectedEntries.has(id); checkbox.disabled = blocked; retranslate.disabled = blocked;
  }
}
const isLocal = () => ['codex','claude_code'].includes($('service-preset').value);
const needsModel = () => ['openai','responses','anthropic'].includes($('provider').value);

function notice(message, error = false) { $('notice').textContent = message; $('notice').classList.toggle('error', error); $('notice').hidden = !message; }
async function api(path, body, raw = false) {
  const response = await fetch('/api/' + path, { method: body === undefined ? 'GET' : 'POST', headers: { 'X-Session-Token': token, ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) }, ...(body === undefined ? {} : { body: JSON.stringify(body) }) });
  if (!response.ok) { const error = await response.json().catch(() => ({})); throw new Error(error.error || '操作失败，请重试或重新启动应用。'); }
  return raw ? response : response.json();
}
function serviceConfig() {
  return { provider:$('provider').value, auth_mode:$('auth-mode').value, base_url:$('base-url').value.trim(), model:$('model').value.trim(), api_key:$('api-key').value, concurrency:Number($('concurrency').value), batch_size:Number($('batch-size').value) };
}
async function remember() { return api('settings', { ...serviceConfig(), preset:$('service-preset').value, source_language:languageValue('source-language'), target_language:languageValue('target-language'), remember_key:!isLocal() && $('remember-key').checked, model_options:modelOptions, use_cache:$('use-cache').checked }); }
function configStatus(text, error = false) { $('config-status').textContent = text; $('config-status').classList.toggle('entry-error', error); if (error) $('connection-settings').open = true; }
function speedSummary() { $('speed-summary').textContent = `翻译速度：${$('concurrency').value} 个并发，每批 ${$('batch-size').value} 条`; }
function modelSelection() {
  const custom = $('model-presets').value === '__custom__';
  $('custom-model-field').hidden = !custom;
  $('model').required = custom && needsModel();
  if (!custom) $('model').value = $('model-presets').value;
}
function renderModels(models = []) {
  modelOptions = models;
  const selected = $('model').value.trim(), select = $('model-presets');
  select.replaceChildren();
  const choices = new Map([['','选择模型']]);
  if (selected && !models.includes(selected)) choices.set(selected,`当前模型：${selected}`);
  for (const model of models) choices.set(model, model);
  choices.set('__custom__','手动填写模型名称');
  for (const [value,text] of choices) { const option = document.createElement('option'); option.value = value; option.textContent = text; select.append(option); }
  select.value = selected || (models.length === 1 ? models[0] : '');
  modelSelection();
}
function providerFields(reset = false) {
  const provider = $('provider').value;
  const modelProvider = needsModel();
  $('model-field').hidden = !modelProvider; $('model-presets').required = modelProvider;
  $('model').required = modelProvider && $('model-presets').value === '__custom__';
  $('api-key').required = provider === 'deepl' || provider === 'anthropic' || $('service-preset').value === 'deepseek';
  $('auth-field').hidden = provider !== 'anthropic';
  if (reset && !(['openai','responses'].includes(provider) && ['openai','responses'].includes(previousProvider))) {
    $('base-url').value = { openai: 'https://api.openai.com/v1', responses: 'https://api.openai.com/v1', anthropic:'https://api.anthropic.com', deepl: 'https://api-free.deepl.com/v2', libretranslate: 'http://127.0.0.1:5000' }[provider];
    $('auth-mode').value = 'api_key';
    $('api-key').value = ''; $('remember-key').checked = false; $('model').value = ''; renderModels();
  }
  $('url-hint').textContent = { openai: '填写基础地址（通常以 /v1 结尾）。本地模型需要先启动服务。', responses: '填写支持 Responses 的基础地址或完整 /responses 地址。', anthropic:'填写 Anthropic 或兼容服务的基础地址，程序自动处理 /v1/messages。', deepl: '免费 API 使用 api-free.deepl.com/v2，专业版请填写对应地址。', libretranslate: '填写已部署的 LibreTranslate 服务地址。' }[provider];
  previousProvider = provider;
  configControls();
}
function configControls() {
  const busy = !!state?.busy || configurationBusy, local = isLocal(), preset = $('service-preset').value;
  for (const id of ['service-preset','provider','auth-mode','base-url','model','model-presets','api-key','remember-key','concurrency','batch-size','use-cache','save-settings','test-service','fetch-models','read-local-config','source-language','target-language','custom-source-language','custom-target-language']) $(id).disabled = busy;
  $('provider').disabled = busy || preset !== 'custom';
  $('auth-mode').disabled = busy || local;
  $('base-url').readOnly = preset !== 'custom';
  $('api-key').readOnly = local;
  $('local-config-field').hidden = !local;
  $('remember-key-field').hidden = local;
  $('key-hint').textContent = local ? '从本机配置读取' : (preset === 'deepseek' ? '填写 DeepSeek 密钥' : '本地服务可留空');
  if (local) $('remember-key').checked = false;
  $('service-readiness').textContent = local && $('api-key').value ? '本机配置已读取' : $('model').value ? '设置会随翻译保存' : '选择服务与模型';
  languageStatus();
  if (typeof window.imageWorkspaceRefresh === 'function') window.imageWorkspaceRefresh();
}
async function readLocalConfig() {
  configurationBusy = true; configControls(); configStatus('正在读取本机 API 配置…');
  try {
    const result = await api('local-config', { preset:$('service-preset').value });
    for (const key of ['provider','auth_mode','base_url','model','api_key']) $(settingFields[key]).value = result[key];
    providerFields(); renderModels(result.model_options);
    $('connection-settings').open = false;
    configStatus('已读取本机 API 配置。可获取模型并测试连接；保存后下次自动恢复。');
  } catch (e) { $('api-key').value = ''; configStatus(e.message, true); }
  finally { configurationBusy = false; configControls(); }
}
async function presetChanged() {
  $('api-key').value = ''; $('remember-key').checked = false; configStatus('');
  $('connection-settings').open = true;
  const preset = $('service-preset').value;
  if (isLocal()) {
    $('provider').value = preset === 'codex' ? 'responses' : 'anthropic';
    $('base-url').value = preset === 'codex' ? 'https://api.openai.com/v1' : 'https://api.anthropic.com';
    $('auth-mode').value = 'api_key'; $('model').value = ''; renderModels(); providerFields();
    await readLocalConfig();
  } else if (preset === 'deepseek') {
    $('provider').value = 'openai'; $('auth-mode').value = 'api_key';
    $('base-url').value = 'https://api.deepseek.com'; $('model').value = 'deepseek-flash';
    renderModels(['deepseek-flash','deepseek-v4-pro']); providerFields();
    configStatus('已填入 DeepSeek 官方地址。填写密钥后测试连接，勾选记住密钥可在下次恢复。');
  } else { providerFields(); }
}
providerFields();
$('service-preset').addEventListener('change', presetChanged);
$('read-local-config').addEventListener('click', readLocalConfig);
$('provider').addEventListener('change', () => providerFields(true));
$('base-url').addEventListener('change', () => renderModels());
$('model-presets').addEventListener('change', modelSelection);
for (const id of ['concurrency','batch-size']) $(id).addEventListener('input', speedSummary);
for (const [id, route] of [['save-settings','settings'],['test-service','test-service'],['fetch-models','models']]) {
  $(id).addEventListener('click', async () => {
    configurationBusy = true; configControls(); configStatus(id === 'fetch-models' ? '正在获取模型列表…' : '正在处理…');
    try {
      const result = route === 'settings' ? await remember() : await api(route, serviceConfig());
      if (route === 'models') {
        renderModels(result.models);
        configStatus(`已获取 ${result.models.length} 个模型，请从下拉框选择。保存配置后，下次会恢复列表和选择。`);
      } else configStatus(result.message);
    } catch (e) { configStatus(e.message, true); }
    finally { configurationBusy = false; configControls(); }
  });
}

async function refresh() {
  state = await api('state');
  const c = entered ? state.catalog : null, p = state.progress, busy = state.busy;
  $('welcome').hidden = !!c; $('workbench').hidden = !c;
  $('game-heading').textContent = c ? c.title : '选择要翻译的游戏';
  $('game-description').textContent = c ? `${languageName(c.source_language)}译为${languageName(c.target_language)} · 翻译记录保存在本地` : '加载原文，连接翻译服务，然后将译文安装到游戏。';
  $('engine-badge').hidden = !c; $('engine-badge').textContent = c?.engine || '';
  $('counts').textContent = c ? `已保存 ${c.translated} / ${c.entries} 条，待翻译 ${c.entries - c.translated} 条${c.failed ? `，失败 ${c.failed} 条` : ''}` : '加载游戏后，原文会显示在这里。';
  setLoading(loading);
  $('translate-button').disabled = busy || configurationBusy || !c || c.translated === c.entries;
  $('translate-button').textContent = c?.translated ? '继续翻译未完成文本' : '开始翻译';
  if (c && c.translated === c.entries) $('translate-button').textContent = '已提取文本翻译完成';
  $('stop-button').hidden = !p.running;
  configControls();
  $('import-button').disabled = busy || !c; $('export-button').disabled = !c;
  $('install-button').disabled = busy || !c || !c.translated;
  $('install-button').textContent = c?.installation?.current ? '已安装最新译文' : c?.installed ? '更新游戏内译文' : '安装译文';
  $('uninstall-button').hidden = !c?.installed; $('uninstall-button').disabled = busy;
  $('launch-button').disabled = busy || !c?.can_launch;
  const installation = c?.installation;
  $('restore-button').hidden = !(c?.installed && installation?.language === c.target_language && installation?.translated && c.translated < c.entries);
  $('restore-button').disabled = busy || loading || languageChanges();
  $('install-heading').textContent = c?.installed ? (installation?.current ? '游戏内译文已同步' : '有译文或适配器需要更新') : '将已保存译文安装到游戏';
  const installText = installation?.error || (c?.installed ? `游戏内已安装 ${installation?.translated ?? 0} 条${installation?.language ? `（${languageName(installation.language)}）` : ''}。${installation?.current ? '请完全退出并重启游戏。' : '点击更新，将本地译文同步到游戏。'}` : '已保存译文尚未安装，游戏仍使用原文。');
  $('install-hint').textContent = installText;
  $('installation-status').textContent = installText;
  $('coverage-title').textContent = c && c.translated === c.entries ? '已提取文本已全部翻译；图片文字不计入进度' : '进度仅统计已提取文本；图片文字需另外处理';
  $('filter-pending').textContent = `待翻译 ${c ? c.entries - c.translated : 0}`;
  $('filter-failed').hidden = !c?.failed; $('filter-failed').textContent = `失败 ${c?.failed || 0}`;
  for (const [id, value] of [['filter-pending','pending'],['filter-failed','failed']]) $(id).setAttribute('aria-pressed', String($('filter').value === value));
  $('stage-load-status').textContent = c ? `已提取 ${c.entries} 条原文` : '提取原文与识别语言';
  $('stage-translate-status').textContent = c ? `${c.translated} / ${c.entries} 条已保存` : '完成批次自动保存';
  $('stage-install-status').textContent = c?.installed ? `游戏内 ${installation?.translated ?? 0} 条${installation?.current ? '，已同步' : '，待更新'}` : '尚未安装译文';
  $('stage-load').classList.toggle('complete', !!c);
  $('stage-translate').classList.toggle('complete', !!c && c.translated === c.entries);
  $('stage-translate').classList.toggle('active', !!c && c.translated < c.entries);
  $('stage-install').classList.toggle('complete', !!installation?.current);
  $('stage-install').classList.toggle('active', !!c?.translated && !installation?.current);
  $('progress-area').hidden = !c;
  $('progress').max = c?.entries || 1; $('progress').value = c?.translated || 0;
  $('progress-text').textContent = p.message; $('progress-number').textContent = c ? `本地已保存 ${c.translated} / ${c.entries}${p.running ? ` · ${p.in_flight || 0} 个请求进行中` : ''}` : '';
  $('game-warnings').hidden = !c;
  $('warning-list').replaceChildren();
  for (const text of c?.warnings || []) { const item = document.createElement('li'); item.textContent = text; $('warning-list').append(item); }
  const signature = `${p.running}/${p.processed}/${c?.translated}/${c?.failed}`;
  if (c && signature !== lastProgress) { lastProgress = signature; await loadEntries(); }
  languageStatus();
}

function setLoading(value) {
  loading = value;
  for (const id of ['scan-button','pick-button','welcome-pick','change-game']) $(id).disabled = value || !!state?.busy;
  $('welcome-title').textContent = value ? '正在读取游戏' : '将 Game.exe 拖到这里';
  $('scan-button').textContent = value ? '正在提取' : '重新扫描';
  languageStatus();
}

async function loadGame(path) {
  if (loading) return;
  if (state?.busy) { notice('正在处理翻译任务，请停止后再加载其他游戏。', true); return; }
  setLoading(true); notice('');
  try {
    const source = languageValue('source-language'), target = languageValue('target-language');
    const scanned = await api('scan', { path, source_language: source, target_language: target });
    loadedLanguages = {source, target};
    entered = true; $('game-path').value = path;
    if (typeof window.resetImageWorkspace === 'function') window.resetImageWorkspace();
    selectedEntries.clear(); visibleEntries = []; rowControls = [];
    page = 0; lastProgress = ''; requestSequence++;
    $('search').value = ''; $('filter').value = '';
    await refresh();
    notice(scanned.restored_from_game ? `已从游戏恢复 ${scanned.restored_from_game} 条已安装译文并保存，无需重复翻译。` : state.catalog.translated === state.catalog.entries ? `已从本地恢复 ${state.catalog.translated} 条译文。${state.catalog.installation?.current ? '游戏内译文已同步。' : '已提取文本已全部完成，可安装到游戏。'}` : state.catalog.translated ? `已从本地恢复 ${state.catalog.translated} 条译文。未安装译文也会保留进度，只需继续未完成的文本。` : `已提取 ${state.catalog.entries} 条原文。每批译文完成后会自动保存，关闭后可继续。`);
  } catch (e) { notice(e.message, true); }
  finally { setLoading(false); }
}

async function chooseGame() {
  if (loading || state?.busy) return;
  let path;
  setLoading(true);
  try { path = (await api('pick-game', {})).path; }
  catch (e) { notice(e.message, true); }
  finally { setLoading(false); }
  if (path) await loadGame(path);
}

window.addEventListener('game-file-drag', event => {
  const detail = event.detail;
  const dragging = detail.kind === 'enter' && !loading && !state?.busy;
  document.body.classList.toggle('file-dragging', dragging);
  $('drop-overlay').hidden = !dragging || !entered;
  if (detail.kind !== 'drop') return;
  if (!Array.isArray(detail.paths) || detail.paths.length !== 1) {
    notice('每次请只拖入一个游戏程序或游戏文件夹。', true); return;
  }
  loadGame(detail.paths[0]);
});

function button(label, className, action) {
  const node = document.createElement('button'); node.type = 'button'; node.textContent = label; node.className = className; node.addEventListener('click', action); return node;
}
async function loadEntries() {
  if (!entered || !state?.catalog) return;
  const sequence = ++requestSequence;
  const data = await api('entries?' + new URLSearchParams({ page, query: $('search').value, filter: $('filter').value }));
  if (sequence !== requestSequence) return;
  page = data.page;
  $('empty').hidden = !!data.entries.length; $('table-wrap').hidden = !data.entries.length;
  if (!data.entries.length) { $('empty').querySelector('h3').textContent = '没有匹配的文本'; $('empty').querySelector('p').textContent = '请调整搜索内容或翻译状态筛选。'; }
  $('text-rows').replaceChildren();
  rowControls = [];
  for (const entry of data.entries) {
    const row = document.createElement('tr'), source = document.createElement('td'), target = document.createElement('td');
    const sourceLine = document.createElement('div'); sourceLine.className = 'source-line';
    const checkbox = document.createElement('input'); checkbox.type = 'checkbox'; checkbox.setAttribute('aria-label', `选择文本：${entry.original.slice(0, 35)}`);
    checkbox.addEventListener('change', () => { if (checkbox.checked) selectedEntries.add(entry.id); else selectedEntries.delete(entry.id); translationTools(); });
    const original = document.createElement('div'); original.className = 'original'; original.textContent = entry.original; sourceLine.append(checkbox, original); source.append(sourceLine);
    const details = document.createElement('details'), summary = document.createElement('summary'), context = document.createElement('div');
    summary.textContent = '查看来源'; context.textContent = entry.contexts.join(' / '); details.append(summary, context); source.append(details);
    const input = document.createElement('textarea'); input.value = entry.translation || ''; input.placeholder = '等待翻译，也可在此填写'; input.setAttribute('aria-label', `修改译文：${entry.original.slice(0, 35)}`); input.disabled = state.busy;
    const actions = document.createElement('div'); actions.className = 'entry-actions';
    const label = document.createElement('span'); label.className = 'state' + (entry.translation ? ' complete' : ''); label.textContent = entry.translation ? '已保存' : (entry.error ? '处理失败' : '未翻译');
    const save = button('保存', 'quiet', async () => {
      save.disabled = true;
      try { const result = await api('entry', { id: entry.id, translation: input.value }); notice(result.message); await refresh(); await loadEntries(); } catch (e) { notice(e.message, true); save.disabled = false; }
    });
    const clear = button('清除', 'quiet', async () => {
      try { const result = await api('entry', { id: entry.id, translation: null }); notice(result.message); await refresh(); await loadEntries(); } catch (e) { notice(e.message, true); }
    });
    save.disabled = state.busy; clear.disabled = state.busy || !entry.translation;
    input.addEventListener('input', () => { label.textContent = '尚未保存'; label.classList.remove('complete'); });
    const retranslate = button('重译', 'quiet', () => openRetranslate([entry.id]));
    rowControls.push([checkbox, retranslate, entry.id]);
    actions.append(label, retranslate, clear, save); target.append(input, actions);
    for (const message of entry.issues || []) { const issue = document.createElement('div'); issue.className = 'entry-issue'; issue.textContent = message; target.append(issue); }
    if (entry.error) { const error = document.createElement('div'); error.className = 'entry-error'; error.textContent = entry.error; target.append(error); }
    row.append(source, target); $('text-rows').append(row);
  }
  $('page-info').textContent = `第 ${page + 1} / ${Math.max(1, Math.ceil(data.total / 50))} 页，共 ${data.total} 条`;
  $('previous').disabled = page === 0; $('next').disabled = (page + 1) * 50 >= data.total;
  visibleEntries = data.entries.map(entry => entry.id);
  translationTools();
}

function openRetranslate(ids) {
  if (state?.busy || languageChanges() || configurationBusy) return;
  retranslateIds = ids;
  $('retranslate-summary').textContent = `将重新翻译 ${ids.length} 条文本，请先保存尚未保存的手动修改。`;
  $('retranslate-error').hidden = true;
  $('retranslate-instructions').disabled = !needsModel();
  if (!needsModel()) $('retranslate-instructions').value = '';
  $('retranslate-dialog').showModal();
}
$('select-page').addEventListener('change', () => {
  for (const id of visibleEntries) { if ($('select-page').checked) selectedEntries.add(id); else selectedEntries.delete(id); }
  translationTools();
});
$('clear-selection').addEventListener('click', () => { selectedEntries.clear(); translationTools(); });
$('retranslate-button').addEventListener('click', () => openRetranslate([...selectedEntries]));
$('retranslate-cancel').addEventListener('click', () => $('retranslate-dialog').close());
$('retranslate-form').addEventListener('submit', async event => {
  event.preventDefault();
  $('retranslate-submit').disabled = true; $('retranslate-cancel').disabled = true;
  configurationBusy = true; configControls();
  try {
    if (languageChanges()) throw new Error('请先应用语言，再重译文本。');
    await remember();
    await api('translate', { ...serviceConfig(), ids:retranslateIds, instructions:$('retranslate-instructions').value.trim(), use_cache:$('use-cache').checked });
    selectedEntries.clear(); $('retranslate-dialog').close(); notice('重译已开始，原译文会保留到新译文成功保存。');
  } catch (e) { $('retranslate-error').textContent = e.message; $('retranslate-error').hidden = false; }
  finally { configurationBusy = false; $('retranslate-submit').disabled = false; $('retranslate-cancel').disabled = false; await refresh(); }
});
$('glossary-button').addEventListener('click', async () => {
  try {
    const result = await api('glossary');
    $('glossary-text').value = result.terms.map(term => `${term.source} = ${term.target}`).join('\n');
    $('glossary-error').hidden = true; $('glossary-dialog').showModal();
  } catch (e) { notice(e.message, true); }
});
$('glossary-cancel').addEventListener('click', () => $('glossary-dialog').close());
$('glossary-form').addEventListener('submit', async event => {
  event.preventDefault(); $('glossary-save').disabled = true;
  try {
    const terms = $('glossary-text').value.split(/\r?\n/).map(line => line.trim()).filter(Boolean).map((line, index) => {
      const separator = line.indexOf('=');
      if (separator <= 0 || !line.slice(separator + 1).trim()) throw new Error(`第 ${index + 1} 项需要填写“原文 = 译文”。`);
      return { source:line.slice(0, separator).trim(), target:line.slice(separator + 1).trim() };
    });
    const result = await api('glossary', {terms});
    $('glossary-dialog').close(); notice(result.message); await refresh(); await loadEntries();
  } catch (e) { $('glossary-error').textContent = e.message; $('glossary-error').hidden = false; }
  finally { $('glossary-save').disabled = false; }
});
$('review-button').addEventListener('click', async () => {
  $('review-button').disabled = true;
  try {
    const result = await api('review');
    $('filter').value = result.flagged ? 'review' : result.pending ? 'pending' : ''; page = 0;
    await loadEntries();
    notice(`已检查 ${result.checked} 条译文，${result.flagged} 条需人工确认，另有 ${result.pending} 条未翻译。检查不会改写译文。`);
  } catch (e) { notice(e.message, true); }
  finally { translationTools(); }
});

$('game-form').addEventListener('submit', async event => {
  event.preventDefault(); await loadGame($('game-path').value);
});
for (const id of ['source-language','target-language','custom-source-language','custom-target-language']) $(id).addEventListener(id.startsWith('custom-') ? 'input' : 'change', languageStatus);
$('apply-languages').addEventListener('click', async () => { if (languageChanges()) await loadGame($('game-path').value); });
$('pick-button').addEventListener('click', chooseGame);
$('welcome-pick').addEventListener('click', chooseGame);
$('change-game').addEventListener('click', async () => {
  if (loading || state?.busy) return;
  entered = false; selectedEntries.clear(); visibleEntries = []; rowControls = []; requestSequence++; lastProgress = ''; notice('');
  try { await refresh(); } catch (e) { notice(e.message, true); }
});
$('service-form').addEventListener('submit', async event => {
  event.preventDefault(); if (configurationBusy) return; $('translate-button').disabled = true;
  try { if (languageChanges()) throw new Error('请先点击“应用语言”，再开始翻译。'); await remember(); await api('translate', { ...serviceConfig(), use_cache:$('use-cache').checked }); configStatus('设置已保存。每批译文完成后会自动保存。'); notice(''); await refresh(); }
  catch (e) { notice(e.message, true); await refresh(); }
});
$('stop-button').addEventListener('click', async () => { try { const result = await api('stop', {}); notice(result.message); } catch (e) { notice(e.message, true); } });
$('restore-button').addEventListener('click', async () => {
  if (state?.busy || loading || languageChanges()) return;
  $('restore-button').disabled = true;
  try {
    const result = await api('restore-installed', {});
    $('filter').value = ''; $('search').value = ''; page = 0;
    notice(result.message); await refresh(); await loadEntries();
  } catch (e) { notice(e.message, true); await refresh(); }
});
for (const [id, endpoint] of [['install-button','install'],['uninstall-button','uninstall'],['launch-button','launch']]) {
  $(id).addEventListener('click', async () => {
    $(id).disabled = true;
    try { const result = await api(endpoint, {}); notice(result.message || '游戏已启动。'); await refresh(); }
    catch (e) { notice(e.message, true); await refresh(); }
  });
}
$('export-button').addEventListener('click', async () => {
  try { const response = await api('export', undefined, true); const url = URL.createObjectURL(await response.blob()); const a = document.createElement('a'); a.href = url; a.download = 'translations.json'; a.click(); setTimeout(() => URL.revokeObjectURL(url), 3000); }
  catch (e) { notice(e.message, true); }
});
$('import-button').addEventListener('click', () => $('import-file').click());
$('import-file').addEventListener('change', async () => {
  const file = $('import-file').files[0]; if (!file) return;
  try { if (file.size > 16 * 1024 * 1024) throw new Error('译文文件超过 16 MB。'); const result = await api('import', JSON.parse(await file.text())); notice(`已导入 ${result.count} 条译文。${result.warning || ''}`); await refresh(); await loadEntries(); }
  catch (e) { notice(e.message, true); } finally { $('import-file').value = ''; }
});
$('search').addEventListener('input', () => { clearTimeout(searching); searching = setTimeout(() => { page = 0; loadEntries().catch(e => notice(e.message, true)); }, 250); });
$('filter').addEventListener('change', () => { page = 0; loadEntries().catch(e => notice(e.message, true)); });
for (const [id, filter] of [['filter-pending','pending'],['filter-failed','failed']]) $(id).addEventListener('click', async () => {
  $('filter').value = $('filter').value === filter ? '' : filter; page = 0;
  await loadEntries(); await refresh();
});
$('previous').addEventListener('click', () => { page--; loadEntries().catch(e => notice(e.message, true)); });
$('next').addEventListener('click', () => { page++; loadEntries().catch(e => notice(e.message, true)); });

(async () => {
  try {
    const saved = await api('settings');
    for (const [key,id] of Object.entries(settingFields)) if (saved[key] !== undefined) {
      if (id === 'source-language' || id === 'target-language') setLanguage(id, saved[key]);
      else $(id).value = saved[key];
    }
    $('remember-key').checked = !!saved.remember_key;
    $('use-cache').checked = saved.use_cache !== false;
    providerFields(); speedSummary();
    renderModels(saved.model_options || []);
    $('connection-settings').open = !saved.api_key || (needsModel() && !saved.model) || !!saved.config_warning;
    if (saved.config_warning) configStatus(saved.config_warning, true);
    else if (isLocal() && saved.api_key) configStatus('已恢复配置，并从本机重新读取 API 密钥。');
    else if (saved.remember_key && saved.api_key) configStatus('已恢复配置和 Windows 加密保存的密钥。');
  } catch (e) { configStatus(e.message, true); }
  try { await refresh(); }
  catch (e) { notice(e.message, true); }
  setInterval(() => { if (!document.hidden) refresh().catch(e => notice(e.message, true)); }, 1500);
})();
