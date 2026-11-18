'use strict';
(() => {
  let active = false, data = null, selected = new Set(), current = '', draft = [], dirty = false, fetching = false, localBusy = false, sourceUrl = '', renderedUrl = '', signature = '', previewSignature = '';
  let ocr = null, checkingOcr = false;
  const asset = () => data?.project?.assets.find(a => a.id === current);
  const blocked = () => localBusy || !!state?.busy || !!ocr?.running || loading || languageChanges();
  function draftGuard() { if (dirty) { notice('请先保存当前图片的文字区域。', true); return false; } return true; }
  function controls() {
    const busy = blocked(), count = selected.size, a = asset();
    for (const id of ['image-scan','image-trace','image-uninstall','image-import','image-add-region','image-ocr-mode','image-cache','image-select-visible','image-clear-selection','image-text-only']) $(id).disabled = busy;
    for (const id of ['image-recognize','image-translate','image-render']) $(id).disabled = busy || !count || dirty;
    $('image-auto').disabled = busy || dirty || !data?.project?.assets.length;
    $('image-auto').textContent = data?.project ? `一键处理全部（${data.project.assets.length} 张）` : '一键处理全部候选';
    $('image-save').disabled = busy || !dirty;
    $('image-install').disabled = busy || dirty || !data?.project?.assets.some(a => a.rendered);
    $('image-uninstall').hidden = !data?.installation?.installed;
    $('image-trace').textContent = data?.installation?.record ? '停止素材记录' : '开启素材记录';
    $('image-selection-count').textContent = `已选 ${count} 张`;
    if (state?.catalog?.engine === 'Ren’Py（可读脚本）') { $('image-scan').disabled = true; $('image-trace').disabled = true; $('image-summary').textContent = '当前图片模块支持 RPG Maker MV / MZ，Ren’Py 可继续使用文本翻译。'; }
    $('image-settings').textContent = $('model').value ? `服务：${$('model').value}` : '翻译服务设置';
    $('image-edit-status').textContent = dirty ? '有未保存的区域修改' : a ? '区域已保存；译图生成后才能安装' : '';
    for (const input of $('image-regions').querySelectorAll('input,textarea,button')) input.disabled = busy;
    for (const input of $('image-list').querySelectorAll('input')) input.disabled = busy;
    const p = data?.progress;
    $('image-progress-area').hidden = !p?.message;
    $('image-progress').max = p?.total || 1; $('image-progress').value = p?.processed || 0;
    $('image-progress-text').textContent = p?.message ? `${p.message}${p.cache_hits ? ` · 缓存复用 ${p.cache_hits} 项` : ''}` : '';
    $('image-stop').hidden = !p?.running;
    $('image-ocr-package').hidden = $('image-ocr-mode').value !== 'local' || (!ocr && !checkingOcr);
    $('image-ocr-status').textContent = checkingOcr ? '正在检测本地 OCR 组件…' : ocr?.message || '';
    $('image-ocr-package').classList.toggle('ready', !!ocr?.available);
    $('image-ocr-check').disabled = busy || checkingOcr;
    $('image-ocr-install').hidden = !ocr?.installable;
    $('image-ocr-install').disabled = busy || checkingOcr;
    $('image-ocr-install').textContent = ocr?.running ? '正在安装…' : '安装日语 OCR 包';
  }
  async function checkOcr() {
    if (checkingOcr || !active || $('image-ocr-mode').value !== 'local') return;
    const language = state?.catalog?.source_language;
    checkingOcr = true; controls();
    try {
      const next = await api('images/ocr');
      if (active && language === state?.catalog?.source_language) ocr = next;
    } catch (error) { notice(error.message, true); }
    finally { checkingOcr = false; controls(); }
  }
  function ocrHint() { $('image-ocr-hint').textContent = $('image-ocr-mode').value === 'vision' ? '图片会发送到当前视觉模型并产生识别用量；仅建议用于本地识别不理想时的所选图片。文字仍会汇总去重、优先复用缓存，再批量翻译。' : '推荐先本地识别：图片不上传，无视觉模型调用。缺少源语言 OCR 包时会在开始前提示。识别出的文字汇总去重并查缓存，仅剩余文字调用翻译服务。'; }
  function revoke() { for (const url of [sourceUrl, renderedUrl]) if (url) URL.revokeObjectURL(url); sourceUrl = renderedUrl = ''; }
  function list() {
    const items = data?.project?.assets || [], query = $('image-search').value.trim().toLowerCase();
    const matches = items.filter(a => (!query || a.path.toLowerCase().includes(query)) && (!$('image-loaded-only').checked || a.loaded) && (!$('image-text-only').checked || a.regions.length));
    $('image-list').replaceChildren();
    for (const a of matches.slice(0, 200)) {
      const row = document.createElement('div'); row.className = 'image-item'; row.classList.toggle('selected', a.id === current);
      const check = document.createElement('input'); check.type = 'checkbox'; check.checked = selected.has(a.id); check.disabled = blocked(); check.setAttribute('aria-label', `选择 ${a.path}`);
      check.addEventListener('change', () => { if (check.checked) selected.add(a.id); else selected.delete(a.id); controls(); });
      const button = document.createElement('button'); button.type = 'button'; button.className = 'quiet';
      const name = document.createElement('strong'); name.textContent = a.path.replace(/^img\//, '');
      const status = document.createElement('span'); status.textContent = [a.loaded ? '加载过' : '引用候选', a.rendered ? '已生成译图' : a.regions.length ? `译文 ${a.regions.filter(r => r.translation).length}/${a.regions.length}` : a.recognized ? '未识别到文字' : '待识别', a.error ? '处理失败' : ''].filter(Boolean).join(' · ');
      button.append(name, status); button.addEventListener('click', () => choose(a.id).catch(e => notice(e.message, true))); row.append(check, button); $('image-list').append(row);
    }
    const hint = document.createElement('p'); hint.className = 'image-list-hint'; hint.textContent = !items.length ? '尚无候选图片，请先扫描引用素材。' : `符合条件 ${matches.length} 张${matches.length > 200 ? '，显示前 200 张，请搜索缩小范围' : ''}`; $('image-list').append(hint);
    const generated = items.filter(a => a.rendered).length, empty = items.filter(a => a.recognized && !a.regions.length && !a.error).length;
    $('image-scan-warnings').hidden = !data?.project?.warnings?.length; $('image-scan-warnings').textContent = (data?.project?.warnings || []).join(' ');
    $('image-summary').textContent = data?.project ? `引用候选 ${items.length} 张 · 加载过 ${items.filter(a => a.loaded).length} 张 · 未检出文字 ${empty} 张 · 已生成译图 ${generated} 张` : '尚未扫描素材。此模块首版支持 RPG Maker MV / MZ。';
    const i = data?.installation;
    $('image-install-status').textContent = i?.error || (i?.installed ? `游戏内安装了 ${i.images} 张译图${i.record ? '，素材记录已开启' : ''}。生成新译图后需再次安装，并重启游戏。` : '图片处理结果会自动保存；安装后完全退出并重启游戏。');
  }
  async function refreshImages(force = false) {
    if (!active || fetching || !entered || !state?.catalog) return;
    fetching = true;
    try {
      const next = await api('images/state');
      if (!active) return;
      data = next;
      if (next.ocr?.language === state?.catalog?.source_language) ocr = next.ocr;
      const nextSignature = JSON.stringify(next);
      if (force || signature !== nextSignature) {
        signature = nextSignature; list();
        if (current && !dirty) { const a = asset(); if (a) await show(a); }
      }
      controls();
    } finally { fetching = false; }
  }
  function mode(images) {
    if (!draftGuard()) return;
    active = images;
    $('image-workspace').hidden = !images; $('text-workspace').hidden = images; $('text-workflow').hidden = images;
    $('text-mode').setAttribute('aria-pressed', String(!images)); $('image-mode').setAttribute('aria-pressed', String(images));
    if (images) { refreshImages(true).catch(e => notice(e.message, true)); checkOcr(); }
  }
  async function choose(id) { if (!draftGuard()) return; current = id; previewSignature = ''; list(); await show(asset()); controls(); }
  async function show(a) {
    if (!a) return;
    $('image-empty').hidden = true; $('image-editor').hidden = false;
    $('image-path').textContent = a.path; $('image-references').textContent = a.references.join('；');
    $('image-error').hidden = !a.error; $('image-error').textContent = a.error || '';
    draft = a.regions.map(r => ({...r})); regionEditor();
    const key = `${a.id}/${a.hash}/${a.rendered}`;
    if (previewSignature === key) return;
    previewSignature = key; const id = current; revoke();
    $('image-source').removeAttribute('src'); $('image-rendered').removeAttribute('src'); $('image-rendered').hidden = true; $('image-rendered-empty').hidden = false;
    const source = await (await api(`images/preview?id=${encodeURIComponent(id)}`, undefined, true)).blob();
    if (id !== current || !active) return;
    sourceUrl = URL.createObjectURL(source); $('image-source').src = sourceUrl;
    if (a.rendered) {
      const rendered = await (await api(`images/preview?id=${encodeURIComponent(id)}&kind=rendered`, undefined, true)).blob();
      if (id !== current || !active) return;
      renderedUrl = URL.createObjectURL(rendered); $('image-rendered').src = renderedUrl; $('image-rendered').hidden = false; $('image-rendered-empty').hidden = true;
    }
  }
  function regionEditor() {
    $('image-regions').replaceChildren();
    draft.forEach((r, index) => {
      const row = document.createElement('fieldset'); row.className = 'image-region';
      const legend = document.createElement('legend'); legend.textContent = `区域 ${index + 1}`; row.append(legend);
      function field(key, title, type) {
        const label = document.createElement('label'); label.textContent = title;
        const input = document.createElement(type === 'text' ? 'textarea' : 'input');
        if (type === 'text') input.rows = 2; else input.type = type;
        input.value = r[key] ?? ''; if (type === 'number') { input.min = key === 'width' || key === 'height' ? '1' : '0'; input.step = '1'; }
        input.addEventListener('input', () => { draft[index][key] = type === 'number' ? Number(input.value) : (key === 'translation' ? input.value || null : input.value); dirty = true; controls(); });
        label.append(input); return label;
      }
      const texts = document.createElement('div'); texts.className = 'image-region-texts'; texts.append(field('original','原文','text'), field('translation','译文','text'));
      const bounds = document.createElement('div'); bounds.className = 'image-region-bounds';
      for (const [key,title] of [['x','X'],['y','Y'],['width','宽度'],['height','高度']]) bounds.append(field(key,title,'number'));
      bounds.append(field('background','底色','color'), field('foreground','文字色','color'));
      const remove = document.createElement('button'); remove.type = 'button'; remove.className = 'quiet'; remove.textContent = '删除区域'; remove.addEventListener('click', () => { draft.splice(index,1); dirty = true; regionEditor(); controls(); });
      row.append(texts,bounds,remove); $('image-regions').append(row);
    });
  }
  async function action(task) { if (blocked() || !draftGuard()) return; localBusy = true; controls(); notice(''); try { await task(); } catch (e) { notice(e.message, true); } finally { localBusy = false; await refresh(); await refreshImages(true); controls(); } }
  for (const type of ['recognize','translate','render']) $('image-' + type).addEventListener('click', () => action(async () => {
    if ((type === 'translate' || (type === 'recognize' && $('image-ocr-mode').value === 'vision')) && !$('service-form').reportValidity()) { mode(false); notice('请先完成翻译服务与模型配置。',true); return; }
    if (type === 'translate' || (type === 'recognize' && $('image-ocr-mode').value === 'vision')) await remember();
    await api('images/process', {ids:[...selected], action:type, mode:$('image-ocr-mode').value, service:serviceConfig(), use_cache:$('image-cache').checked});
  }));
  $('image-auto').addEventListener('click', () => action(async () => {
    if (!$('service-form').reportValidity()) { mode(false); notice('请先完成翻译服务与模型配置。', true); return; }
    await remember();
    await api('images/process', {scope:'all', action:'auto', mode:$('image-ocr-mode').value, service:serviceConfig(), use_cache:$('image-cache').checked});
    notice('已开始处理全部候选：先识别图片，再汇总文字、精确去重并查缓存，只分批翻译剩余文字；已有结果保留，完成后检查译图并单独安装。');
  }));
  $('text-mode').addEventListener('click', () => mode(false)); $('image-mode').addEventListener('click', () => mode(true)); $('image-settings').addEventListener('click', () => mode(false));
  $('image-scan').addEventListener('click', () => action(async () => { const r = await api('images/scan',{}); notice(`已扫描 ${r.count} 张引用候选素材，尚未执行文字识别。`); }));
  $('image-ocr-check').addEventListener('click', () => { if (!blocked()) return checkOcr(); });
  $('image-ocr-install').addEventListener('click', () => action(async () => {
    await api('images/ocr/install', {});
    notice('已提交日语 OCR 安装。系统可能请求管理员授权，下载来源为 Windows 更新，不调用模型。');
  }));
  $('image-stop').addEventListener('click', () => api('images/stop',{}).catch(e => notice(e.message,true)));
  $('image-install').addEventListener('click', () => action(async () => notice((await api('images/install',{})).message)));
  $('image-uninstall').addEventListener('click', () => action(async () => notice((await api('images/uninstall',{})).message)));
  $('image-trace').addEventListener('click', () => action(async () => notice((await api('images/trace',{enabled:!data?.installation?.record})).message)));
  $('image-text-only').addEventListener('change', list); $('image-search').addEventListener('input', list); $('image-loaded-only').addEventListener('change', list); $('image-ocr-mode').addEventListener('change', () => { ocrHint(); controls(); checkOcr(); });
  $('image-select-visible').addEventListener('click', () => { for (const check of $('image-list').querySelectorAll('input')) { if (selected.size >= 100) break; check.checked = true; check.dispatchEvent(new Event('change')); } controls(); });
  $('image-clear-selection').addEventListener('click', () => { selected.clear(); list(); controls(); });
  $('image-add-region').addEventListener('click', () => { draft.push({x:0,y:0,width:Math.min(120,$('image-source').naturalWidth || 120),height:Math.min(40,$('image-source').naturalHeight || 40),original:'',translation:null,background:'#ffffff',foreground:'#202020'}); dirty = true; regionEditor(); controls(); });
  $('image-save').addEventListener('click', async () => {
    if (blocked() || !current) return; localBusy = true; controls();
    try { await api('images/edit',{id:current,regions:draft}); dirty = false; notice('文字区域已保存，请重新生成译图后安装。'); } catch (e) { notice(e.message,true); }
    finally { localBusy = false; await refreshImages(true); controls(); }
  });
  $('image-import').addEventListener('click', () => { if (draftGuard()) $('image-import-file').click(); });
  $('image-import-file').addEventListener('change', () => action(async () => {
    const file = $('image-import-file').files[0]; if (!file) return;
    try { if (file.size > 10 * 1024 * 1024) throw new Error('替换 PNG 不能超过 10 MB。'); const bytes = new Uint8Array(await file.arrayBuffer()); let binary = ''; for (let i=0;i<bytes.length;i+=8192) binary += String.fromCharCode(...bytes.subarray(i,i+8192)); await api('images/import',{id:current,png:btoa(binary)}); notice('替换 PNG 已导入，可单独安装到游戏。'); } finally { $('image-import-file').value = ''; }
  }));
  window.resetImageWorkspace = () => { active = false; data = null; ocr = null; selected.clear(); current = ''; draft = []; dirty = false; signature = previewSignature = ''; revoke(); $('image-editor').hidden = true; $('image-empty').hidden = false; $('image-search').value = ''; $('image-loaded-only').checked = false; $('image-text-only').checked = false; $('image-ocr-mode').value = 'local'; ocrHint(); mode(false); };
  window.imageWorkspaceRefresh = () => { if (active) controls(); };
  setInterval(() => { if (active && !document.hidden) refreshImages().catch(e => notice(e.message,true)); },1500);
  ocrHint();
})();
