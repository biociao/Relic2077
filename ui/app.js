'use strict';

// No remote assets or client-side copy of the vault. Every memory is read from
// the local API; localStorage holds only the appearance preference.
const $ = (selector, root = document) => root.querySelector(selector);
const main = $('#main');
const kinds = { knowledge: '知识', decision: '决策', pattern: '模式', lesson: '经验', reflection: '反思' };
const statuses = { active: '活跃', fading: '渐淡', superseded: '已替代', archived: '已归档' };
const views = { history: '历史会话提炼', graph: '关系网络', overview: '记忆总览', memories: '记忆库', health: '健康检查', automation: '自动化与同步', settings: '知识库配置' };
const paths = {
  overview: '<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/>',
  memories: '<rect x="4" y="3" width="16" height="18" rx="2"/><path d="M8 7h8M8 11h8M8 15h5"/>',
  health: '<path d="M3 12h4l3-8 4 16 3-8h4"/>',
  settings: '<path d="M4 7h9m4 0h3M4 17h3m4 0h9"/><circle cx="15" cy="7" r="2"/><circle cx="9" cy="17" r="2"/>',
  shield: '<path d="m12 3 8 3v6c0 5-8 9-8 9s-8-4-8-9V6l8-3Z"/><path d="m8 12 3 3 5-6"/>',
  sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2m0 16v2M2 12h2m16 0h2M5 5l1 1m12 12 1 1M5 19l1-1M18 6l1-1"/>',
  moon: '<path d="M20 14A8 8 0 0 1 10 4a8 8 0 1 0 10 10Z"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  refresh: '<path d="M20 7v5h-5M4 17v-5h5"/><path d="M6 7a7 7 0 0 1 12-1l2 3M4 15l2 3a7 7 0 0 0 12-1"/>',
  arrow: '<path d="M5 12h14m-5-5 5 5-5 5"/>',
  search: '<circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 5 5"/>',
  check: '<path d="m5 12 4 4L19 6"/>',
  clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
  close: '<path d="m6 6 12 12M6 18 18 6"/>',
  edit: '<path d="m14 5 5 5M4 20l5-1L20 8a2 2 0 0 0-5-5L4 14v6Z"/>',
  info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v6m0-10v1"/>',
  database: '<ellipse cx="12" cy="5" rx="8" ry="3"/><path d="M4 5v14c0 4 16 4 16 0V5M4 12c0 4 16 4 16 0"/>',
  graph: '<circle cx="6" cy="7" r="2.5"/><circle cx="18" cy="6" r="2.5"/><circle cx="12" cy="17" r="2.5"/><path d="m8 8 8-1M7 9l4 6M17 8l-4 7"/>',
};
function icon(name) { return `<svg class="icon" viewBox="0 0 24 24" aria-hidden="true">${paths[name] || paths.memories}</svg>`; }
function escapeHtml(value) { return String(value ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c])); }
function badge(status) { const safe = Object.hasOwn(statuses, status) ? status : 'archived'; return `<span class="badge ${safe}">${escapeHtml(statuses[status] || status)}</span>`; }
function pct(value) { return Number.isFinite(value) ? `${Math.round(value * 100)}%` : '—'; }
function number(value) { return value == null ? '—' : new Intl.NumberFormat('zh-CN').format(value); }
function date(value, full = false) { const d = new Date(value); return Number.isNaN(d.getTime()) ? '—' : new Intl.DateTimeFormat('zh-CN', full ? { dateStyle: 'medium', timeStyle: 'short' } : { month: '2-digit', day: '2-digit' }).format(d); }
function tags(items, limit = 4) { return (items || []).slice(0, limit).map(tag => `<span class="tag">${escapeHtml(tag)}</span>`).join(''); }
function empty(title, description, action = '') { return `<div class="empty-state">${icon('memories')}<h3>${escapeHtml(title)}</h3><p>${escapeHtml(description)}</p>${action}</div>`; }
function createButton() { return `<button class="button primary" data-action="create">${icon('plus')}新建记忆</button>`; }
function heading(view, eyebrow, description, actions = '') { return `<div class="page-heading"><div><p class="eyebrow">${eyebrow}</p><h1>${views[view]}</h1><p>${escapeHtml(description)}</p></div><div class="heading-actions">${actions}</div></div>`; }
function refreshButton() { return `<button class="button ghost" data-action="refresh">${icon('refresh')}刷新</button>`; }
function errorBanner(message) { return `<div class="error-banner" role="alert">${escapeHtml(message)}</div>`; }
let currentView = 'overview', overviewData = null, configData = null, configDirty = false, editorDirty = false;
let currentEntry = null, editingEntry = null, pageRequest = 0, listRequest = 0, toastTimer, searchTimer, detailRequest = 0;
let filters = { q: '', kind: '', status: '', tag: '', source_agent: '', min_confidence: '', offset: 0, limit: 15 };
let listTotal = 0, refreshing = false;
let historyDefaults=null, historyHost='codex', historyRoot='', historyData=null, historyFilter='', historyBusy=false, historyNotice='';
const historySelected=new Set();
const historyExcluded=new Set();
let editorSaving = false, configSaving = false, openingEditor = false;

function busyForm(form, busy) {
  form.setAttribute('aria-busy', String(busy));
  form.querySelectorAll('input,textarea,select,button').forEach(control => { control.disabled = busy; });
}

async function api(path, options = {}) {
  const headers = { Accept: 'application/json', ...options.headers };
  if (options.body !== undefined) { headers['Content-Type'] = 'application/json'; headers['x-relic-ui'] = '1'; }
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), options.timeout || 20000);
  try {
    const response = await fetch(path, { ...options, headers, signal: controller.signal, cache: 'no-store', body: options.body === undefined ? undefined : JSON.stringify(options.body) });
    let data;
    try { data = await response.json(); } catch { throw new Error('服务返回了无法读取的响应，请检查终端中的 Relic 服务。'); }
    if (!response.ok) {
      const error = new Error(response.status === 409 ? `内容已被其他窗口或 Agent 修改。请重新加载后再保存。${data.error ? `（${data.error}）` : ''}` : data.error || `请求失败 (${response.status})`);
      error.status = response.status;
      throw error;
    }
    return data;
  } catch (error) {
    if (error.name === 'AbortError') throw new Error('请求超时。若正在保存，请先刷新确认结果后再重试。');
    if (error instanceof TypeError) throw new Error('无法连接本地 Relic 服务，请确认 relic ui 仍在运行。');
    throw error;
  } finally { clearTimeout(timer); }
}

function toast(message) { clearTimeout(toastTimer); $('#toast').textContent = message; $('#toast').classList.add('visible'); toastTimer = setTimeout(() => $('#toast').classList.remove('visible'), 4200); }
function connection(online) { $('#connection-dot').classList.toggle('online', online); $('#connection-label').classList.toggle('online', online); $('#connection-label').textContent = online ? '本地服务已连接' : '连接已断开'; }
function applyBars() {
  document.querySelectorAll('[data-width]').forEach(el => { el.style.width = `${Math.max(0, Math.min(100, Number(el.dataset.width) || 0))}%`; });
  document.querySelectorAll('[data-weight]').forEach(el => { el.style.flex = String(Math.max(0, Number(el.dataset.weight) || 0)); });
}
// The distribution view is a separate asset that draws its own markup; it needs
// the same deferred sizing helper the dashboard cards use.
window.RelicApp = { applyBars };
async function loadOverview() {
  try {
    const data = await api('/api/overview'); overviewData = data;
    // The brain banner reads its own projection of the vault. Refresh it at the
    // same moments the dashboard refreshes, never on a timer of its own.
    window.RelicBrain?.load().catch(() => { /* The banner shows its own error state. */ });
    connection(true);
    $('#vault-name').textContent = data.vault.name || 'Relic Vault';
    $('#vault-name').title = data.vault.path;
    $('#nav-count').textContent = number(data.stats?.total);
    $('#version').textContent = data.vault.version == null ? 'SCHEMA ?' : `SCHEMA ${data.vault.version}`;
    $('#last-updated').textContent = `更新于 ${new Date().toLocaleTimeString('zh-CN', { hour12: false })}`;
    return data;
  } catch (error) { connection(false); throw error; }
}

function activityChart(activity) {
  if (!activity?.length) return empty('暂无活动数据', '修复健康检查中报告的问题后重新加载。');
  const max = Math.max(1, ...activity.map(day => day.count));
  const width = 620, height = 162, top = 20, left = 25, bottom = 132, step = 40;
  const grid = [0, .5, 1].map(n => `<line class="gridline" x1="${left}" x2="615" y1="${bottom - n * (bottom - top)}" y2="${bottom - n * (bottom - top)}"/><text x="0" y="${bottom - n * (bottom - top) + 3}">${Number((max * n).toFixed(1))}</text>`).join('');
  const bars = activity.map((day, i) => {
    const h = day.count / max * (bottom - top);
    return `<rect class="bar" x="${left + 12 + i * step}" y="${bottom - h}" width="18" height="${h}" rx="3"><title>${escapeHtml(day.date)}：${day.count} 条记忆</title></rect>${(i % 3 === 0 && i < activity.length - 2) || i === activity.length - 1 ? `<text x="${left + 21 + i * step}" y="154" text-anchor="middle">${escapeHtml(day.date.slice(5).replace('-', '/'))}</text>` : ''}`;
  }).join('');
  return `<div class="chart"><svg viewBox="0 0 ${width} ${height}" role="img" aria-label="近14天记忆最后更新日期分布：${escapeHtml(activity.map(d => `${d.date} ${d.count}条`).join('，'))}">${grid}${bars}</svg></div>`;
}
function breakdown(values, labels = {}) {
  const items = Object.entries(values || {}).sort((a, b) => b[1] - a[1]);
  const max = Math.max(1, ...items.map(item => item[1]));
  return items.length ? `<div class="breakdown">${items.map(([key, value]) => `<div class="breakdown-row"><span class="breakdown-name" title="${escapeHtml(key)}">${escapeHtml(labels[key] || key)}</span><span class="mini-bar"><span data-width="${value / max * 100}"></span></span><span class="mono">${number(value)}</span></div>`).join('')}</div>` : '<div class="note">暂无记录</div>';
}
function recentEntry(entry) {
  return `<button class="recent-entry" data-entry="${escapeHtml(entry.meta.id)}"><span class="entry-kind-icon">${icon(entry.meta.type === 'decision' ? 'shield' : 'memories')}</span><span class="recent-entry-text"><span class="entry-title">${escapeHtml(entry.meta.title)}</span><span class="entry-subtitle"><span>${escapeHtml(kinds[entry.meta.type] || entry.meta.type)}</span><span class="separator"></span><span>${escapeHtml(entry.meta.source_agents.join(' · ') || '未标注来源')}</span><span class="separator"></span><span>${date(entry.meta.updated)}</span></span></span><span class="confidence-number ${entry.effective_confidence < .3 ? 'low' : ''}">${pct(entry.effective_confidence)}</span></button>`;
}
const metric = (label, value, foot, image, cls = '') => `<div class="metric ${cls}"><div class="metric-top"><span>${label}</span>${icon(image)}</div><div class="metric-number">${value}</div><div class="metric-foot"><span class="dot"></span>${foot}</div></div>`;
function renderOverview(data) {
  const s = data.stats, n = value => number(s?.[value]);
  const total = s?.total || 0;
  const errors = data.health.filter(item => item.status === 'error');
  main.innerHTML = heading('overview', 'YOUR MEMORY, CONNECTED', '让积累的知识保持清晰、可信、可追溯。', refreshButton() + createButton())
    + (errors.length ? errorBanner(`有 ${errors.length} 项检查需要处理，请前往「健康检查」查看详情。`) : '')
    + `<div class="metrics">${metric('记忆总量', n('total'), '所有类型的记忆条目', 'database')}${metric('活跃记忆', n('active'), total ? `占全部记忆 ${pct(s.active / total)}` : '等待第一条记忆', 'health', 'accent')}${metric('平均置信度', s ? pct(s.average_confidence) : '—', '已计入时间衰减与过期', 'shield')}${metric('待复核', n('needs_review'), '渐淡或低于复核阈值的记忆', 'clock', s?.needs_review ? 'warning' : '')}</div>
    <div class="dashboard-grid"><section class="panel"><div class="panel-header"><div><h2>记忆活动</h2><p>按条目的最后更新日期统计</p></div><span class="subtle-label">近 14 天 · UTC</span></div>${activityChart(data.activity)}<div class="chart-legend"><span class="legend-key">更新的记忆</span><span>共 ${number((data.activity || []).reduce((sum, day) => sum + day.count, 0))} 条</span></div></section>
    <section class="panel"><div class="panel-header"><h2>记忆生命周期</h2><span class="subtle-label">当前状态</span></div><div class="lifecycle"><div class="segment-bar" aria-hidden="true">${Object.keys(statuses).filter(key => s?.[key]).map(key => `<span data-weight="${s[key]}" class="${key}"></span>`).join('')}</div>${Object.entries(statuses).map(([key, label]) => `<div class="lifecycle-row"><i></i><span>${label}</span><strong>${n(key)}</strong><span>${total ? pct(s[key] / total) : '—'}</span></div>`).join('')}</div></section>
    <section class="panel"><div class="panel-header"><div><h2>最近更新</h2><p>每一次积累，都有迹可循</p></div><a class="text-button" href="#memories">全部记忆 ${icon('arrow')}</a></div>${data.recent.length ? `<div class="recent-list">${data.recent.map(recentEntry).join('')}</div>` : empty(s ? '你的记忆库，从这里开始' : '暂时无法读取记忆', s ? '添加一条知识、决策或经验，让未来的你和 Agent 可以再次找到它。' : '前往健康检查查看读取失败的原因。', s ? createButton() : '<a class="button" href="#health">查看健康检查</a>')}<div class="panel-footer">置信度显示为考虑衰减后的当前值</div></section>
    <section class="panel"><div class="panel-header"><h2>知识构成</h2><span class="subtle-label">类型</span></div>${breakdown(data.by_kind, kinds)}<hr class="section-rule"><div class="panel-header"><h2>记忆来源</h2><span class="subtle-label">Agent</span></div>${breakdown(data.by_agent)}<div class="panel-footer">来源来自记忆记录，同一条记忆可有多个来源。</div></section></div>`;
  applyBars();
}

function options(values, selected, allLabel) {
  const entries = Array.isArray(values) ? values.map(value => [value, value]) : Object.entries(values);
  return `<option value="">${allLabel}</option>` + entries.map(([value, label]) => `<option value="${escapeHtml(value)}" ${String(value) === String(selected) ? 'selected' : ''}>${escapeHtml(label)}</option>`).join('');
}
// The distribution view collapses to a single header line; the preference is a
// view preference like the theme, never vault data, so it stays on the device.
let statsCollapsed = false;
try { statsCollapsed = localStorage.getItem('relic-stats-collapsed') === '1'; } catch { statsCollapsed = false; }
function saveStatsCollapsed(collapsed) { statsCollapsed = collapsed; try { localStorage.setItem('relic-stats-collapsed', collapsed ? '1' : '0'); } catch { /* Device preference is optional. */ } }
function statsParams() {
  // Pagination never changes a distribution, so it is not part of the scope.
  return new URLSearchParams(Object.entries(filters).filter(([key, value]) => key !== 'offset' && key !== 'limit' && value !== '').map(([key, value]) => [key, String(value)]));
}
function renderMemoriesShell() {
  main.innerHTML = heading('memories', 'THE KNOWLEDGE VAULT', '浏览、检索和整理你的长期记忆。', createButton())
    + `<section class="filter-panel" aria-label="搜索筛选"><div class="search-wrap">${icon('search')}<input id="memory-search" type="search" placeholder="搜索标题、内容、标签或记忆 ID…" aria-label="搜索记忆" value="${escapeHtml(filters.q)}" autocomplete="off"></div><div class="filters"><label>记忆类型<select name="kind">${options(kinds, filters.kind, '全部类型')}</select></label><label>生命周期<select name="status">${options(statuses, filters.status, '全部状态')}</select></label><label>标签<select name="tag" id="tag-filter">${options(filters.tag ? [filters.tag] : [], filters.tag, '全部标签')}</select></label><label>来源 Agent<select name="source_agent" id="agent-filter">${options(filters.source_agent ? [filters.source_agent] : [], filters.source_agent, '全部来源')}</select></label><label>最低置信度<select name="min_confidence">${options({ '0.3': '30%', '0.5': '50%', '0.7': '70%', '0.9': '90%' }, filters.min_confidence, '不限')}</select></label><button class="button ghost small" data-action="clear-filters">重置</button></div></section><div id="memory-stats"></div><div id="memory-results" aria-live="polite"><div class="loading-state"><span class="spinner"></span>读取记忆…</div></div>`;
  window.RelicStats?.reset($('#memory-stats'), { collapsed: statsCollapsed });
}
async function loadMemories() {
  const request = ++listRequest, snapshot = { ...filters };
  const params = new URLSearchParams(Object.entries(snapshot).filter(([, value]) => value !== '').map(([key, value]) => [key, String(value)]));
  try {
    const data = await api(`/api/entries?${params}`);
    if (request !== listRequest || currentView !== 'memories') return;
    if (data.total > 0 && snapshot.offset >= data.total) { filters.offset = Math.floor((data.total - 1) / filters.limit) * filters.limit; return loadMemories(); }
    connection(true); listTotal = data.total;
    $('#tag-filter').innerHTML = options([...new Set([...data.tags, ...(filters.tag ? [filters.tag] : [])])], filters.tag, '全部标签');
    $('#agent-filter').innerHTML = options([...new Set([...data.agents, ...(filters.source_agent ? [filters.source_agent] : [])])], filters.source_agent, '全部来源');
    $('#memory-results').innerHTML = `<div class="list-summary"><span>找到 <strong>${number(data.total)}</strong> 条记忆</span><span>最近更新优先</span></div><section class="panel">${data.entries.length ? `<table class="memory-table"><thead><tr><th scope="col">记忆</th><th scope="col">类型</th><th scope="col">状态</th><th scope="col">置信度</th><th scope="col">更新</th></tr></thead><tbody>${data.entries.map(entry => `<tr><td><button class="entry-link" data-entry="${escapeHtml(entry.meta.id)}"><span class="entry-title">${escapeHtml(entry.meta.title)}</span></button><div class="tags">${tags(entry.meta.tags, 3)}</div></td><td>${escapeHtml(kinds[entry.meta.type] || entry.meta.type)}</td><td>${badge(entry.meta.status)}</td><td><div class="confidence-cell"><span class="confidence-number ${entry.effective_confidence < .3 ? 'low' : ''}">${pct(entry.effective_confidence)}</span><span class="mini-bar"><span data-width="${entry.effective_confidence * 100}"></span></span></div></td><td class="mono">${date(entry.meta.updated)}</td></tr>`).join('')}</tbody></table>` : empty('没有找到记忆', Object.values(snapshot).some((v, i) => i < 6 && v !== '') ? '试试不同的关键词，或重置筛选条件。' : '记下一个值得再次使用的经验，开始构建你的长期记忆。', createButton())}<div class="pagination"><span>${data.total ? `${snapshot.offset + 1}–${Math.min(snapshot.offset + snapshot.limit, data.total)} / ${number(data.total)}` : '0 条记忆'}</span><div class="button-row"><button class="button small ghost" data-action="prev-page" ${snapshot.offset === 0 ? 'disabled' : ''}>上一页</button><span class="mono">${Math.floor(snapshot.offset / snapshot.limit) + 1} / ${Math.max(1, Math.ceil(data.total / snapshot.limit))}</span><button class="button small ghost" data-action="next-page" ${snapshot.offset + snapshot.limit >= data.total ? 'disabled' : ''}>下一页</button></div></div></section>`;
    applyBars();
    loadMemoryStats();
  } catch (error) {
    if (request === listRequest && currentView === 'memories') $('#memory-results').innerHTML = errorBanner(error.message) + '<button class="button" data-action="reload-list">重新加载</button>';
  }
}
/// The charts describe the same filter scope as the list, so they are fetched
/// beside it and on the same events — never on a timer of their own.
function loadMemoryStats() {
  if (currentView !== 'memories' || statsCollapsed) return null;
  const container = $('#memory-stats');
  return container ? window.RelicStats?.load(container, statsParams().toString()) : null;
}

function healthMessage(item) {
  if (item.name === 'git' && item.status === 'warning') return '尚未检测到 Git 版本库。可在知识库目录运行 git init 启用版本历史。';
  if (item.name === 'index' && item.status === 'warning') {
    if (item.message.includes('missing')) return '搜索索引尚不存在，可通过下方按钮从记忆文件重建。';
    if (item.message.includes('out of date')) return '记忆内容已变化，搜索索引需要重建。';
    if (item.message.includes('memory errors')) return '索引可以读取，但记忆文件存在错误，暂时无法确认是否一致。';
  }
  if (item.status !== 'ok') return item.message;
  return { config: '配置格式与字段校验通过。', entries: `${number(overviewData?.stats?.total)} 条 Markdown 记忆可正常读取。`, index: '本地搜索索引可读取，且与记忆更新记录一致。', git: '已检测到 Git 版本库。同步状态请通过 Git 或 Relic CLI 查看。' }[item.name] || item.message;
}
// The relation view renders the derived graph itself. It is a window onto the
// same data the CLI and MCP tools serve, so the shell only supplies the heading
// and a container; ui/graph.js draws the network inside it.
function renderGraphShell(view) {
  main.innerHTML = heading(view, 'RELATION NETWORK', '记忆之间带证据的关系网络：显式链接、版本替代、共同主题、语义相近、结论冲突与结论互证。') + '<div id="graph-view"><div class="loading-state"><span class="spinner"></span>正在推导关系网络…</div></div>';
  return $('#graph-view');
}

// Clicking a node in the relation view opens that memory, exactly as a click in
// the library list would. The graph is an entry point, not a parallel data path.
window.RelicGraphOpen = (id) => { openEntry(id); };
window.RelicGraphToast = (message) => { toast(message); };

function renderHealth(data) {  const names = { config: '知识库配置', entries: 'Markdown 记忆文件', index: '搜索索引', git: 'Git 版本历史' };
  main.innerHTML = heading('health', 'VAULT DIAGNOSTICS', '检查本地记忆、配置与搜索索引的可用性。', refreshButton()) + `<section class="panel"><div class="panel-header"><h2>检查结果</h2><span class="subtle-label">${date(new Date(), true)}</span></div><div class="health-list">${data.health.map(item => `<div class="health-row"><span class="health-status-icon ${item.status === 'ok' ? '' : item.status === 'warning' ? 'warning' : 'error'}">${icon(item.status === 'ok' ? 'check' : 'info')}</span><div class="health-copy"><strong>${escapeHtml(names[item.name] || item.name)}</strong><p>${escapeHtml(healthMessage(item))}</p></div><span class="badge ${item.status === 'ok' ? 'ok' : item.status === 'warning' ? 'warning' : 'error'}">${{ ok: '正常', warning: '待处理', error: '异常' }[item.status] || '未知'}</span></div>`).join('')}</div></section><section class="panel health-information"><div><strong>重建搜索索引</strong><p>从 Markdown 重新生成 SQLite 搜索索引。记忆文件保持原样。</p></div><button class="button" data-action="reindex">${icon('refresh')}重建索引</button></section><section class="panel health-information"><div><strong>当前知识库</strong><p class="mono">${escapeHtml(data.vault.path)}</p></div><a class="button ghost" href="#settings">查看配置 ${icon('arrow')}</a></section>`;
}
function renderSettings(data) {
  configData = data; configDirty = false;
  const c = data.config;
  const pair = (label, value) => `<div class="detail-pair"><dt>${label}</dt><dd>${escapeHtml(value)}</dd></div>`;
  main.innerHTML = heading('settings', 'VAULT PREFERENCES', '管理知识库、置信度与同步设置。') + (data.error ? errorBanner(`配置无法通过校验，可在下方修复后保存：${data.error}`) : '')
    + `<div class="settings-grid"><section class="panel"><div class="panel-header"><div><h2>配置文件</h2><p class="mono">.relic/config.yaml</p></div><span class="subtle-label">YAML</span></div><form id="config-form" class="config-editor"><label class="skip-label" for="config-raw">配置原文</label><textarea id="config-raw" name="raw" spellcheck="false" aria-describedby="config-hint">${escapeHtml(data.raw)}</textarea><p class="inline-note" id="config-hint">保存前会校验格式与字段；注释和扩展字段按原文保留。</p><p id="config-error" class="form-error" role="alert"></p><div class="editor-actions"><span id="config-state">与磁盘内容一致</span><div class="button-row"><button type="button" class="button ghost" data-action="reload-config">重新加载</button><button class="button primary" type="submit" id="save-config" disabled>保存配置</button></div></div></form></section><div class="settings-side"><section class="panel"><div class="panel-header"><h2>当前生效配置</h2></div><dl class="config-details">${c ? pair('知识库名称', c.vault.name) + pair('默认置信度', pct(c.vault.default_confidence)) + pair('默认年衰减率', c.vault.default_decay_rate) + pair('复核阈值', pct(c.evolution.fading_threshold)) + pair('同步模式', c.sync.mode === 'auto' ? '自动（配置值）' : '手动') + pair('同步远端', c.sync.remotes.map(remote => `${remote.name} · ${remote.url}`).join('\n') || '尚未配置') : pair('状态', '配置无效，等待修复')}</dl></section><section class="panel settings-note"><div class="panel-header"><h2>配置说明</h2></div><div class="note"><p>默认值用于新建记忆；调整衰减率不会改写已有条目。</p><p>复核阈值用于提示需要关注的记忆，不会自动更改已存储的生命周期状态。</p><p>设置 <code>sync.mode: auto</code> 并配置远端后，由独立运行的 <code>relic daemon --vault 路径</code> 自动同步。请在自动化页面查看状态。</p><p>如其他窗口或 Agent 修改了配置，保存会提示冲突。请复制你的修改后重新加载，再合并保存。</p></div></section></div></div>`;
}

// The memory brain is a dashboard element: it belongs to the overview and is not
// rendered on the other views, so it never competes with list or editor work.
function showBrain(visible) {
  const banner = document.querySelector('#memory-brain');
  if (!banner) return;
  banner.hidden = !visible;
  // Shown again: the canvas needs a fresh measurement and, on first entry, its
  // own data. The banner never loads while it is hidden.
  if (visible) window.RelicBrain?.show();
}

// A brain node is a shortcut into the memory library's existing filters, never a
// separate data path: same filters, same list, same Markdown files.
async function applyBrainFilter(spec) {
  if (!spec) return;
  filters = { ...filters, ...spec, offset: 0 };
  if (currentView === 'memories') { renderMemoriesShell(); await loadMemories(); }
  else await navigate('memories');
  const what = spec.kind ? `类型「${kinds[spec.kind] || spec.kind}」`
    : spec.tag ? `标签「${spec.tag}」`
      : spec.source_agent ? `来源「${spec.source_agent}」`
        : `「${spec.q}」`;
  toast(`已按${what}筛选记忆`);
}

async function navigate(view, force = false) {
  if (view === 'main') { history.replaceState(null, '', `#${currentView}`); main.focus(); return; }
  if (!Object.hasOwn(views, view)) { location.hash = 'overview'; return; }
  if (configSaving) { history.replaceState(null, '', `#${currentView}`); toast('配置正在保存，请稍候。'); return; }
  if (configDirty && currentView === 'settings' && view !== 'settings' && !force && !confirm('配置尚未保存。离开并放弃这些修改？')) { history.replaceState(null, '', '#settings'); return; }
  if (view !== 'settings') configDirty = false;
  if (view !== currentView) window.scrollTo(0, 0);
  currentView = view;
  const request = ++pageRequest; ++listRequest;
  document.querySelectorAll('[data-view]').forEach(link => { const active = link.dataset.view === view; link.classList.toggle('active', active); if (active) link.setAttribute('aria-current', 'page'); else link.removeAttribute('aria-current'); });
  showBrain(view === 'overview');
  $('#breadcrumb-current').textContent = views[view]; document.title = `${views[view]} · Relic`;
  if (view === 'memories') { renderMemoriesShell(); await loadMemories(); return; }
  main.innerHTML = '<div class="loading-state"><span class="spinner"></span>正在读取知识库…</div>';
  try {
    if (view === 'history') { historyDefaults ||= await api('/api/history/defaults'); if (request === pageRequest) renderHistory(); return; }
    if (view === 'graph') { const host = renderGraphShell(view); const graph = await api('/api/graph'); if (request === pageRequest && host) window.RelicGraph?.render(host, graph); return; }
    const data = view === 'settings' ? await api('/api/config') : view === 'automation' ? await api('/api/automation') : await loadOverview();
    if (request !== pageRequest) return;
    if (view === 'automation') renderAutomation(data); else if (view === 'settings') renderSettings(data); else if (view === 'health') renderHealth(data); else renderOverview(data);
  } catch (error) { if (request === pageRequest) main.innerHTML = heading(view, 'RELIC WORKSPACE', '暂时无法读取数据。') + errorBanner(error.message) + '<button class="button" data-action="refresh">重新连接</button>'; }
}

// Render a small safe Markdown subset. Escape first; no HTML, image loads, or
// executable URL schemes from memory content are ever inserted into the DOM.
function markdown(body) {
  let fenced = false, code = [], output = [], paragraph = [], list = [];
  const inline = value => escapeHtml(value).replace(/`([^`]+)`/g, '<code>$1</code>').replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  const flush = () => { if (paragraph.length) { output.push(`<p>${paragraph.map(inline).join('<br>')}</p>`); paragraph = []; } if (list.length) { output.push(`<ul>${list.map(line => `<li>${inline(line)}</li>`).join('')}</ul>`); list = []; } };
  for (const line of String(body).split('\n')) {
    if (line.trim().startsWith('```')) { flush(); if (fenced) { output.push(`<pre><code>${escapeHtml(code.join('\n'))}</code></pre>`); code = []; } fenced = !fenced; continue; }
    if (fenced) { code.push(line); continue; }
    const h = line.match(/^(#{1,3})\s+(.+)$/), li = line.match(/^\s*[-*]\s+(.+)$/);
    if (h) { flush(); output.push(`<h${h[1].length}>${inline(h[2])}</h${h[1].length}>`); }
    else if (li) { if (paragraph.length) flush(); list.push(li[1]); }
    else if (!line.trim()) flush();
    else { if (list.length) flush(); paragraph.push(line); }
  }
  flush(); if (fenced) output.push(`<pre><code>${escapeHtml(code.join('\n'))}</code></pre>`);
  return output.join('');
}
function renderEntry(entry) {
  currentEntry = entry;
  const m = entry.meta;
  $('#entry-dialog').innerHTML = `<div class="dialog-header"><div><p class="eyebrow">${escapeHtml(kinds[m.type] || m.type)} / MEMORY DETAIL</p><h2 id="entry-dialog-title">${escapeHtml(m.title)}</h2></div><button class="icon-button" data-action="close-entry" aria-label="关闭详情">${icon('close')}</button></div><div class="dialog-body"><dl class="entry-meta-grid"><div><dt>生命周期</dt><dd>${badge(m.status)}</dd></div><div><dt>当前 / 原始置信度</dt><dd class="confidence-number">${pct(entry.effective_confidence)} <span class="muted">/ ${pct(m.confidence)}</span></dd></div><div><dt>来源 Agent</dt><dd>${escapeHtml(m.source_agents.join(' · ') || '未标注')}</dd></div><div><dt>创建时间</dt><dd class="mono">${date(m.created, true)}</dd></div><div><dt>最近更新</dt><dd class="mono">${date(m.updated, true)}</dd></div><div><dt>最近验证</dt><dd class="mono">${date(m.last_verified, true)}</dd></div><div><dt>年衰减率</dt><dd class="mono">${m.decay_rate}</dd></div><div><dt>过期时间</dt><dd class="mono">${m.expires ? date(m.expires, true) : '无'}</dd></div></dl><div class="tags">${tags(m.tags, 100)}</div><article class="entry-document">${markdown(entry.body)}</article>${m.superseded_by ? `<div class="entry-relations">被替代为 <button class="text-button" data-entry="${escapeHtml(m.superseded_by)}">${escapeHtml(m.superseded_by)}</button></div>` : ''}${m.supersedes?.length ? `<div class="entry-relations">替代了 ${m.supersedes.map(id => `<button class="text-button" data-entry="${escapeHtml(id)}">${escapeHtml(id)}</button>`).join(' · ')}</div>` : ''}${m.links?.length ? `<div class="entry-relations">关联记忆 ${m.links.map(id => `<button class="text-button" data-entry="${escapeHtml(id)}">${escapeHtml(id)}</button>`).join(' · ')}</div>` : ''}<div class="entry-path">${escapeHtml(m.id)}<br>${escapeHtml(entry.path)}</div></div><div class="dialog-footer"><span>Markdown 是记忆的唯一事实来源</span><button class="button" data-action="edit-entry">${icon('edit')}编辑记忆</button></div>`;
}
async function openEntry(id) {
  const request = ++detailRequest, dialog = $('#entry-dialog');
  dialog.innerHTML = '<div class="dialog-header"><h2 id="entry-dialog-title">记忆详情</h2><button class="icon-button" data-action="close-entry" aria-label="关闭详情">' + icon('close') + '</button></div><div class="loading-state"><span class="spinner"></span>读取记忆…</div>';
  if (!dialog.open) dialog.showModal();
  try { const data = await api(`/api/entries/${encodeURIComponent(id)}`); if (request === detailRequest && dialog.open) { renderEntry(data.entry); $('[data-action=close-entry]', dialog).focus(); } }
  catch (error) { if (request === detailRequest && dialog.open) $('.loading-state', dialog).outerHTML = `<div class="dialog-body">${errorBanner(error.message)}</div>`; }
}
async function openEditor(entry = null) {
  if (openingEditor || editorSaving || $('#editor-dialog').open) return;
  openingEditor = true;
  editingEntry = entry; editorDirty = false;
  let defaultConfidence = .7;
  if (!entry) { try { const data = await api('/api/config'); if (!data.config) throw new Error('请先在知识库配置中修复配置文件。'); defaultConfidence = data.config.vault.default_confidence; } catch (error) { openingEditor = false; toast(error.message); return; } }
  const m = entry?.meta;
  $('#editor-dialog').innerHTML = `<div class="dialog-header"><div><p class="eyebrow">${entry ? 'EDIT MEMORY' : 'A NEW PIECE OF KNOWLEDGE'}</p><h2 id="editor-title">${entry ? '编辑记忆' : '新建记忆'}</h2></div><button class="icon-button" data-action="close-editor" aria-label="关闭编辑器">${icon('close')}</button></div><form id="entry-form" class="editor-form"><label class="field"><span>标题</span><input name="title" value="${escapeHtml(m?.title || '')}" required placeholder="这条记忆值得记住的是什么？"></label><div class="field-grid">${entry ? `<label class="field"><span>生命周期</span><select name="status">${Object.entries(statuses).map(([value, label]) => `<option value="${value}" ${value === m.status ? 'selected' : ''}>${label}</option>`).join('')}</select></label>` : `<label class="field"><span>类型</span><select name="kind">${Object.entries(kinds).map(([value, label]) => `<option value="${value}">${label}</option>`).join('')}</select></label>`}<label class="field"><span>原始置信度（0–1）</span><input name="confidence" type="number" min="0" max="1" step="any" required value="${m?.confidence ?? defaultConfidence}"><small>${entry ? '更改此值会将最近验证时间更新为现在。' : '使用知识库的默认置信度。'}</small></label></div><label class="field"><span>标签 · 用逗号分隔</span><input name="tags" value="${escapeHtml(m?.tags.join(', ') || '')}" placeholder="例如：架构, 工作流程, 已验证"></label><label class="field"><span>${entry ? '内容 · Markdown 原文' : '内容 · 支持 Markdown'}</span><textarea name="content" spellcheck="false" placeholder="记录结论、背景与适用条件…">${escapeHtml(entry?.body || '')}</textarea></label><p class="form-error" id="entry-error" role="alert"></p><div class="dialog-footer"><span>${entry ? '保留记忆 ID、来源与关联信息' : '来源记录为 relic-ui'}</span><div class="button-row"><button type="button" class="button ghost" data-action="close-editor">取消</button><button type="submit" class="button primary" id="save-entry">${entry ? '保存修改' : '创建记忆'}</button></div></div></form>`;
  $('#editor-dialog').showModal();
  openingEditor = false;
  $('[name=title]', $('#editor-dialog')).focus();
}
function closeEditor() { if (editorSaving) { toast('记忆正在保存，请稍候。'); return false; } if (editorDirty && !confirm('记忆尚未保存。放弃这些修改？')) return false; editorDirty = false; $('#editor-dialog').close(); return true; }
async function saveEntry(form) {
  const fields = new FormData(form), button = $('#save-entry');
  if (button.disabled || editorSaving) return;
  const title = String(fields.get('title')).trim();
  if (new TextEncoder().encode(title).length > 240) { $('#entry-error').textContent = '标题过长，请控制在 240 字节以内（约 80 个汉字）。'; return; }
  const payload = { title, content: String(fields.get('content')), tags: [...new Set(String(fields.get('tags')).split(/[,，]/).map(t => t.trim()).filter(Boolean))] };
  const confidence = Number(fields.get('confidence'));
  if (editingEntry) {
    payload.status = fields.get('status'); payload.expected_updated = editingEntry.meta.updated;
    payload.expected_revision = editingEntry.revision;
    // Sending the unchanged confidence would incorrectly re-verify the memory.
    if (confidence !== editingEntry.meta.confidence) payload.confidence = confidence;
  } else { payload.confidence = confidence; payload.kind = fields.get('kind'); payload.source_agent = 'relic-ui'; }
  editorSaving = true; busyForm(form, true); $('#entry-error').textContent = '';
  try {
    const data = await api(editingEntry ? `/api/entries/${encodeURIComponent(editingEntry.meta.id)}` : '/api/entries', { method: editingEntry ? 'PATCH' : 'POST', body: payload });
    editorDirty = false; $('#editor-dialog').close();
    renderEntry(data.entry); if (!$('#entry-dialog').open) $('#entry-dialog').showModal();
    $('[data-action=close-entry]', $('#entry-dialog')).focus();
    toast(editingEntry ? '记忆已更新' : '新记忆已保存到知识库');
    await refreshAfterWrite();
  } catch (error) { $('#entry-error').textContent = `${error.message}${error.status >= 500 ? ' 保存可能已写入文件，请先检查记忆库再重试。' : ''}`; }
  finally { editorSaving = false; busyForm(form, false); }
}
async function saveConfig() {
  const button = $('#save-config'), form = $('#config-form'); if (button.disabled || configSaving) return;
  const raw = $('#config-raw').value, expected = configData.raw;
  configSaving = true; busyForm(form, true); $('#config-error').textContent = '';
  try {
    const data = await api('/api/config', { method: 'PUT', body: { raw, expected_raw: expected } });
    configSaving = false; renderSettings(data); toast('配置已保存');
    loadOverview().catch(() => { /* Saving succeeded; connection status reflects refresh failure. */ });
  } catch (error) { $('#config-error').textContent = error.message; }
  finally { configSaving = false; if (form.isConnected) { busyForm(form, false); button.disabled = !configDirty; } }
}
async function refreshAfterWrite() {
  try { if (currentView === 'automation') { if (document.activeElement?.closest('.setup-panel')) return; const request = pageRequest; const data = await api('/api/automation'); if (request === pageRequest && currentView === 'automation') renderAutomation(data); return; } const data = await loadOverview(); if (currentView === 'overview') renderOverview(data); else if (currentView === 'health') renderHealth(data); else if (currentView === 'memories') await loadMemories(); }
  catch (error) { toast(`已保存；刷新失败：${error.message}`); }
}

document.addEventListener('click', async event => {
  const target = event.target.closest('[data-action],[data-entry]'); if (!target || target.disabled) return;
  if (target.dataset.entry) { await openEntry(target.dataset.entry); return; }
  if (target.dataset.brain) {
    await applyBrainFilter(window.RelicBrain?.filterFor(target.dataset.brain, 'topic', target.dataset.brainLabel));
    return;
  }
  switch (target.dataset.action) {
    case 'history-select-all': case 'history-clear': case 'history-project-select': case 'history-project-exclude': {
      if(historyBusy || !historyData) break;
      const action=target.dataset.action, project=target.dataset.project;
      if(action==='history-clear') historySelected.clear();
      else if(action==='history-project-exclude') {
        if(historyExcluded.has(project)) historyExcluded.delete(project);
        else {historyExcluded.add(project); for(const row of historyData.sessions) if(row.project===project)historySelected.delete(row.session_id);}
      } else {
        for(const row of historyVisibleRows()) if(historyEligible(row) && (action==='history-select-all' || row.project===project)) historySelected.add(row.session_id);
      }
      renderHistoryRows(); syncHistorySelection(); break;
    }
    case 'history-scan': await scanHistory(); break;
    case 'history-import': await importHistory(); break;
    case 'setup-host': {
      const project = $('#setup-project').value.trim(), dsh_home = $('#setup-dsh-home').value.trim(), host = target.dataset.host;
      target.disabled = true;
      try {
        const data = await api('/api/automation/setup', {method:'POST',body:{host,project,dsh_home}});
        toast(host === 'codex' ? '配置已安装，请在 Codex 完成信任审核并开始新任务' : host === 'dsh' ? '配置已安装，请重启 DSH' : host === 'stop' ? '已请求停止，当前处理完成后退出' : '后台已启动');
        if (currentView === 'automation') renderAutomation(data);
      } catch (error) { toast(error.message); }
      finally { target.disabled = false; }
      break;
    }
    case 'sync-retry': case 'queue-work': case 'capture-retry': case 'capture-accept': case 'capture-reject': {
      let path, body = {};
      const action = target.dataset.action, id = target.dataset.capture;
      if (action === 'sync-retry') path = '/api/automation/retry';
      else if (action === 'queue-work') path = '/api/captures/work';
      else if (action === 'capture-retry') path = `/api/captures/${encodeURIComponent(id)}/retry`;
      else {
        const reason = prompt(action === 'capture-accept' ? '请先核对候选内容及依据。填写审核依据后，将写入正式记忆：' : '填写拒绝原因：');
        if (!reason?.trim()) break;
        path = `/api/captures/${encodeURIComponent(id)}/review`;
        body = { decision: action === 'capture-accept' ? 'accept' : 'reject', reason, knowledge: null };
      }
      target.disabled = true;
      try { await api(path, { method: 'POST', body }); toast(action === 'sync-retry' ? '已安排重试，需要后台 worker 运行' : '操作已完成'); if (currentView === 'automation') await navigate('automation'); }
      catch (error) { toast(error.message); }
      finally { target.disabled = false; }
      break;
    }
    case 'create': await openEditor(); break;
    case 'refresh': await navigate(currentView); break;
    case 'clear-filters': filters = { q: '', kind: '', status: '', tag: '', source_agent: '', min_confidence: '', offset: 0, limit: 15 }; clearTimeout(searchTimer); renderMemoriesShell(); await loadMemories(); break;
    case 'reload-list': await loadMemories(); break;
    case 'toggle-stats': {
      const collapsed = window.RelicStats?.toggle($('#memory-stats'), target.getAttribute('aria-expanded') === 'true');
      saveStatsCollapsed(Boolean(collapsed));
      if (!collapsed) await loadMemoryStats();
      break;
    }
    case 'reload-stats': await loadMemoryStats(); break;
    case 'prev-page': filters.offset = Math.max(0, filters.offset - filters.limit); await loadMemories(); break;
    case 'next-page': if (filters.offset + filters.limit < listTotal) { filters.offset += filters.limit; await loadMemories(); } break;
    case 'close-entry': ++detailRequest; $('#entry-dialog').close(); break;
    case 'edit-entry': await openEditor(currentEntry); break;
    case 'close-editor': closeEditor(); break;
    case 'reload-config': if (!configDirty || confirm('重新加载将放弃尚未保存的配置修改。继续？')) { configDirty = false; await navigate('settings', true); } break;
    case 'reindex':
      target.disabled = true;
      try { const result = await api('/api/reindex', { method: 'POST', body: {} }); toast(`索引已重建，共 ${number(result.count)} 条记忆`); await navigate('health'); }
      catch (error) { toast(error.message); }
      finally { target.disabled = false; } break;
  }
});
document.addEventListener('relic:brain-node', async event => {
  // Canvas clicks travel as an event so the banner keeps no navigation state.
  const detail = event.detail || {};
  await applyBrainFilter(window.RelicBrain?.filterFor(detail.id, detail.group, detail.label));
});
document.addEventListener('input', event => {
  if (event.target.id === 'history-root') { historyRoot=event.target.value; historyData=null; historySelected.clear(); historyExcluded.clear(); $('#history-results').innerHTML='目录已改变，请重新扫描。'; $('#history-import').disabled=true; }
  if (event.target.id === 'history-filter') { historyFilter=event.target.value; renderHistoryRows(); }
  if (event.target.id === 'memory-search') { filters.q = event.target.value; filters.offset = 0; ++listRequest; clearTimeout(searchTimer); searchTimer = setTimeout(loadMemories, 250); }
  if (event.target.id === 'config-raw') { configDirty = event.target.value !== configData.raw; $('#config-state').textContent = configDirty ? '有尚未保存的修改' : '与磁盘内容一致'; $('#save-config').disabled = configSaving || !configDirty; }
  if (event.target.closest('#entry-form')) editorDirty = true;
});
document.addEventListener('change', event => {
  if (event.target.id === 'history-host') { historyHost=event.target.value; historyRoot=''; historyData=null; historySelected.clear(); historyExcluded.clear(); renderHistory(); }
  if (event.target.matches('[data-history-select]')) { const key=event.target.dataset.historySelect; const row=historyData?.sessions.find(s=>s.session_id===key); if(historyBusy || !row || !historyEligible(row))return; if(event.target.checked) historySelected.add(key); else historySelected.delete(key); renderHistoryRows(); }
 if (event.target.closest('.filters') && event.target.name) { filters[event.target.name] = event.target.value; filters.offset = 0; clearTimeout(searchTimer); loadMemories(); } });
document.addEventListener('submit', event => { if (event.target.id === 'entry-form') { event.preventDefault(); saveEntry(event.target); } if (event.target.id === 'config-form') { event.preventDefault(); saveConfig(); } });
$('#editor-dialog').addEventListener('cancel', event => { event.preventDefault(); closeEditor(); });
$('#entry-dialog').addEventListener('cancel', () => { ++detailRequest; });
window.addEventListener('beforeunload', event => { if (configDirty || editorDirty || configSaving || editorSaving) { event.preventDefault(); event.returnValue = ''; } });
window.addEventListener('hashchange', () => { clearTimeout(searchTimer); navigate(location.hash.slice(1) || 'overview'); });
function setTheme(theme) { document.documentElement.dataset.theme = theme; const label = theme === 'dark' ? '切换浅色外观' : '切换深色外观'; const image = icon(theme === 'dark' ? 'sun' : 'moon'); $('#theme-toggle').innerHTML = `${image}<span>${label}</span>`; $('#compact-theme-toggle').innerHTML = image; $('#compact-theme-toggle').setAttribute('aria-label', label); try { localStorage.setItem('relic-theme', theme); } catch { /* Device preference is optional. */ } }
$('#theme-toggle').addEventListener('click', () => setTheme(document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark'));
$('#compact-theme-toggle').addEventListener('click', () => setTheme(document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark'));
document.querySelectorAll('[data-icon]').forEach(el => { el.innerHTML = icon(el.dataset.icon); });
try { setTheme(localStorage.getItem('relic-theme') === 'light' ? 'light' : 'dark'); } catch { setTheme('dark'); }
$('#today').textContent = new Date().toLocaleDateString('en-CA').replaceAll('-', '.');
async function start() { try { await loadOverview(); } catch { /* The active view presents a recoverable connection error. */ } await window.RelicBrain?.init(); await navigate(location.hash.slice(1) || 'overview'); }
start();
setInterval(async () => {
  if (currentView === 'history' || historyBusy) return;
  if (document.hidden || configDirty || editorDirty || refreshing || main.contains(document.activeElement) || $('#entry-dialog').open || $('#editor-dialog').open) return;
  refreshing = true;
  try { if (currentView === 'automation') { if (document.activeElement?.closest('.setup-panel')) return; const request = pageRequest; const data = await api('/api/automation'); if (request === pageRequest && currentView === 'automation') renderAutomation(data); return; } const data = await loadOverview(); if (currentView === 'overview') renderOverview(data); else if (currentView === 'health') renderHealth(data); }
  catch { /* Preserve the last useful result; the connection indicator shows failure. */ }
  finally { refreshing = false; }
}, 30000);

function renderAutomation(data) {
  const opened = new Set([...main.querySelectorAll("details[open][data-capture-detail]")].map(el => el.dataset.captureDetail));
  const w = data.worker, queue = data.queue, counts = key => queue.filter(q => q.status === key).length;
  const time = value => value ? date(value * 1000, true) : '尚无记录';
  const names = {pending:'等待处理',processing:'处理中',needs_review:'待审核',accepted:'已采纳',rejected:'已拒绝',failed:'失败'};
  main.innerHTML = heading('automation', 'MEMORY PIPELINE', '任务前检索 · 轮次结束采集 · 审核后沉淀 · 后台同步', refreshButton())
    + renderSetup(data.setup)
    + `<div class="metrics">${metric('后台活动', data.setup?.worker_running ? '运行中' : '未运行', time(w.heartbeat), 'clock')}${metric('等待处理', counts('pending') + counts('processing'), '已持久保存，可恢复处理', 'database')}${metric('待审核候选', counts('needs_review'), '审核前不会成为正式记忆', 'shield')}${metric('同步模式', data.mode === 'auto' ? (w.paused ? '已暂停' : '自动') : '手动', time(w.last_sync), 'refresh')}</div>`
    + (w.last_error ? errorBanner(w.last_error) : '') + (w.queue_error ? errorBanner(w.queue_error) : '')
    + `<section class="panel health-information"><div><strong>独立后台处理</strong><p>关闭 Agent 后可继续处理。每 5 秒检查队列，修改稳定 15 秒后同步，持续修改最多等待 60 秒。冲突需要先在知识库解决，再重试。</p><p>后台命令：<code>relic daemon --vault 知识库路径</code> · 下次同步：${time(w.next_sync)}</p></div><div class="button-row"><button class="button" data-action="queue-work">处理队列</button><button class="button" data-action="sync-retry">重试同步</button></div></section>`
    + `<section class="panel"><div class="panel-header"><h2>采集与审核</h2><span>最近 50 条 / 共 ${queue.length} 条</span></div><div class="note">${queue.slice().sort((a,b) => b.updated_at.localeCompare(a.updated_at)).slice(0,50).map(q => `<details class="capture-detail" data-capture-detail="${q.id}" ${opened.has(q.id) ? 'open' : ''}><summary>${q.origin === 'manual_history' ? '【手动历史提炼】' : q.origin === 'automatic' ? '【实时采集】' : ''}${escapeHtml(q.candidate?.knowledge.title || q.id)} · ${names[q.status] || escapeHtml(q.status)} · ${date(q.updated_at, true)}</summary>${q.session_id ? `<p class="inline-note">${escapeHtml(q.source_agent)} · ${escapeHtml(q.project)} · ${escapeHtml(q.session_id)}</p>` : ''}${q.last_error ? errorBanner(q.last_error) : ''}${q.candidate?.distillation ? `<p class="inline-note">模型建议：${escapeHtml(({new: '新增', update: '更新已有知识', skip: '跳过'})[q.candidate.distillation.recommendation] || q.candidate.distillation.recommendation)} · ${escapeHtml(q.candidate.distillation.rationale)}（证据仍需复核）</p>` : ''}<div class="markdown-body">${markdown(q.candidate?.knowledge.content || '等待后台生成候选。')}</div><div class="button-row">${q.status === 'needs_review' ? `<button class="button primary" data-action="capture-accept" data-capture="${q.id}">审核并采纳</button><button class="button" data-action="capture-reject" data-capture="${q.id}">拒绝</button>` : q.status === 'failed' ? `<button class="button" data-action="capture-retry" data-capture="${q.id}">重试采集</button>` : ''}${q.entry_id ? `<button class="button" data-entry="${escapeHtml(q.entry_id)}">查看正式记忆</button>` : ''}</div></details>`).join('') || '<p>暂无采集。接入钩子后，轮次摘要会出现在这里。</p>'}</div></section>`
    + `<section class="panel"><div class="panel-header"><h2>最近钩子事件</h2></div><div class="health-list">${data.events.slice().reverse().map(e => `<div class="health-row"><div class="health-copy"><strong>${escapeHtml(e.host)} · ${escapeHtml(e.event)}</strong><p>${date(e.at, true)} · ${escapeHtml(({ok:'成功',error:'失败',skipped_no_response:'无回复内容，已跳过'})[e.status] || e.status)}${e.error ? ' · ' + escapeHtml(e.error) : ''}</p></div></div>`).join('') || '<div class="note">暂无事件。安装钩子并完成宿主信任确认后，开始新的任务。</div>'}</div></section>`;
}

function renderSetup(setup) {
  if (!setup) return '';
  const labels = {not_installed:'未安装',awaiting_event:'已安装 · 等待首次触发',triggered:'已收到事件',error:'需要处理'};
  const hostRow = (name,key,hint) => `<div class="health-row"><div class="health-copy"><strong>${name} · ${escapeHtml(labels[setup[key].state])}</strong><p>${hint}</p><p class="mono">${escapeHtml(setup[key].path)}</p>${setup[key].error ? errorBanner(setup[key].error) : ''}</div><button class="button" data-action="setup-host" data-host="${key}">${setup[key].installed ? '重新安装' : '安装接入'}</button></div>`;
  const project = $('#setup-project')?.value || setup.project, home = $('#setup-dsh-home')?.value || setup.dsh_home;
  return `<section class="panel setup-panel"><div class="panel-header"><h2>接入与启用</h2><span>安装配置 → 宿主加载 → 首次触发</span></div><div class="note"><div class="form-grid"><label class="field"><span>接入项目 · 绝对路径</span><input id="setup-project" value="${escapeHtml(project)}"></label><label class="field"><span>DSH 数据目录 · 绝对路径</span><input id="setup-dsh-home" value="${escapeHtml(home)}"></label></div><p>安装会保存原配置备份。采集本项目的提示与最终回复，作为待审核来源；来源会随知识库同步。</p></div><div class="health-list">${hostRow('Codex','codex','项目级钩子。安装后需在 Codex 完成信任审核并开始新任务；配置存在不代表已经获得信任。')}${hostRow('DSH','dsh','全局加载、仅对上方项目及其子目录生效。安装后重启 DSH；此界面无法代替宿主加载插件。')}<div class="health-row"><div class="health-copy"><strong>后台处理 · ${setup.worker_running ? '运行中' : '未运行'}</strong><p>独立进程，关闭此页面后继续处理。系统重启后需要再次启动。</p></div><button class="button" data-action="setup-host" data-host="${setup.worker_running ? 'stop' : 'worker'}">${setup.worker_running ? '停止后台' : '启动后台'}</button></div></div></section>`;
}


function renderHistory() {
  main.innerHTML=heading('history','MANUAL HISTORY','历史会话仅在手动选择后提炼，与实时采集分开处理。')
    + `<section class="panel"><div class="panel-header"><h2>选择历史来源</h2><span class="tag">仅手动触发</span></div><div class="note"><p>扫描只读取本地会话并检查已有记录，不调用模型、不创建记忆。选择后生成待审核候选，审核通过才正式收录。</p><div class="form-grid"><label class="field"><span>会话来源</span><select id="history-host" ${historyBusy?'disabled':''}><option value="codex" ${historyHost==='codex'?'selected':''}>Codex</option><option value="dsh" ${historyHost==='dsh'?'selected':''}>DSH</option></select></label><label class="field"><span>历史会话目录</span><input id="history-root" value="${escapeHtml(historyRoot || historyDefaults[historyHost])}" ${historyBusy?'disabled':''}></label></div><div class="button-row"><button class="button" data-action="history-scan" ${historyBusy?'disabled':''}>扫描历史会话</button><button class="button primary" id="history-import" data-action="history-import" ${historyBusy || !historySelected.size?'disabled':''}>${historyData?.backend.backend==='command'?'用模型提炼所选会话':'生成所选模板候选'}</button><span id="history-count">已选择 ${historySelected.size} 个会话</span></div><p>已采集、待审核、已收录、已拒绝或失败的会话不会再次导入。旧会话若检测到 Relic 写入调用，会单独标记为需核对；无法保证识别没有任何来源关联的旧知识。</p><p id="history-notice" role="status">${escapeHtml(historyNotice)}</p></div></section>`
    + `<section class="panel"><div class="panel-header"><h2>历史会话</h2><label class="field"><span class="sr-only">筛选会话</span><input id="history-filter" placeholder="按项目、标题或会话 ID 筛选" value="${escapeHtml(historyFilter)}"></label></div><div id="history-results" class="note"></div></section>`;
  renderHistoryRows();
}
function historyEligible(s) { return s.status==='available' && !historyExcluded.has(s.project); }
function historyVisibleRows() { return (historyData?.sessions || []).filter(s=>`${s.title} ${s.project} ${s.session_id}`.toLowerCase().includes(historyFilter.toLowerCase())); }
function syncHistorySelection() {
  if($('#history-import')) $('#history-import').disabled=historyBusy || !historySelected.size;
  if($('#history-count')) $('#history-count').textContent=`已选择 ${historySelected.size} 个会话 · 本次排除 ${historyExcluded.size} 个项目`;
}
function renderHistoryRows() {
  const region=$('#history-results'); if(!region)return;
  if(!historyData) {region.innerHTML=empty('尚未扫描','点击“扫描历史会话”查看可手动处理的记录。'); return;}
  const opened=new Set([...region.querySelectorAll('details[open][data-history-project]')].map(el=>el.dataset.historyProject));
  const labels={available:'可手动处理',recorded:'已关联正式记忆',captured:'已有采集记录',check_required:'旧记忆关联需核对',active:'未完成 / 中断',empty:'无完成轮次'};
  const rows=historyVisibleRows(), groups=new Map();
  for(const row of rows) { if(!groups.has(row.project))groups.set(row.project,[]); groups.get(row.project).push(row); }
  const renderRow=s=>`<div class="health-row history-row"><label class="history-choice"><input type="checkbox" data-history-select="${escapeHtml(s.session_id)}" aria-label="选择 ${escapeHtml(s.title)}" ${historySelected.has(s.session_id)?'checked':''} ${!historyEligible(s)||historyBusy?'disabled':''}><span class="health-copy"><strong>${escapeHtml(s.title || s.session_id)}</strong><span class="history-meta">${escapeHtml(s.project)}</span><span class="history-meta mono">${escapeHtml(historyHost)} · ${escapeHtml(s.session_id)} · ${s.turns} 个完成轮次</span><span class="history-meta">${escapeHtml(s.detail)}</span></span></label><div class="history-status"><span class="tag">${labels[s.status] || escapeHtml(s.status)}</span>${s.captures.map(c=>`<span class="history-meta">${escapeHtml(({pending:'等待处理',processing:'处理中',needs_review:'待审核',accepted:'已采纳',rejected:'已拒绝',failed:'失败'})[c.status] || c.status)}</span>`).join('')}${[...new Set([...s.entries,...s.captures.map(c=>c.entry_id).filter(Boolean)])].map(id=>`<button class="button" data-entry="${escapeHtml(id)}">查看正式记忆</button>`).join('')}${s.captures.length?'<a href="#automation">查看采集与审核</a>':''}</div></div>`;
  region.innerHTML=`<div class="history-bulk"><div class="button-row"><button class="button" data-action="history-select-all" ${historyBusy || !rows.some(historyEligible)?'disabled':''}>全选当前筛选中的可用会话</button><button class="button" data-action="history-clear" ${historyBusy || !historySelected.size?'disabled':''}>清空全部选择</button></div><p>按项目分组 · 当前显示 ${rows.length} / ${historyData.sessions.length} 个会话。全选会跳过已处理会话和排除的项目。项目排除仅作用于本次手动操作，不改变实时采集。</p><p>${historyData.backend.backend==='command'?'模型已配置：手动提交后调用模型，提炼可复用知识，结果仍需审核。':'模型未配置：扫描、去重和分组无需 LLM；提交只生成原文模板候选，不会自动总结知识。要自动提炼经验，需要先配置 LLM。'}</p></div>`
    + (historyData.truncated?errorBanner('已达到扫描上限，请选择更具体的会话子目录继续扫描。'):'')
    + historyData.errors.map(e=>errorBanner(e)).join('')
    + ([...groups].sort(([a],[b])=>a.localeCompare(b)).map(([project,items])=>{
      const excluded=historyExcluded.has(project), eligible=items.filter(historyEligible), selected=items.filter(s=>historySelected.has(s.session_id)).length;
      return `<section class="history-project ${excluded?'excluded':''}"><div class="history-project-heading"><div><h3>${escapeHtml(project)}</h3><p>当前显示 ${items.length} 个 · 可选 ${eligible.length} 个 · 已选 ${selected} 个 ${excluded?'· 已排除':''}</p></div><div class="button-row"><button class="button" data-action="history-project-select" data-project="${escapeHtml(project)}" ${historyBusy || !eligible.length?'disabled':''}>选中本组</button><button class="button" data-action="history-project-exclude" data-project="${escapeHtml(project)}" ${historyBusy?'disabled':''}>${excluded?'恢复项目':'排除整个项目'}</button></div></div><details data-history-project="${escapeHtml(project)}" ${opened.has(project)?'open':''}><summary>查看 ${items.length} 个会话</summary>${items.map(renderRow).join('')}</details></section>`;
    }).join('') || empty('没有匹配会话','调整筛选条件或更换会话目录。'));
  syncHistorySelection();
}

async function scanHistory() {
  if(historyBusy)return;
  historyRoot=$('#history-root').value.trim(); historyHost=$('#history-host').value;
  historyBusy=true;historySelected.clear();historyData=null;historyNotice='正在扫描本地会话，不会触发提炼…';renderHistory();
  try {historyData=await api('/api/history/scan',{method:'POST',body:{host:historyHost,root:historyRoot},timeout:120000});historyNotice='扫描完成。可全选、按项目选择或排除项目，再手动提交。';}
  catch(e){historyNotice=e.message;}
  finally{historyBusy=false;if(currentView==='history')renderHistory();}
}
async function importHistory() {
  if(historyBusy || !historyData)return;
  const chosen=historyData.sessions.filter(s=>historyEligible(s) && historySelected.has(s.session_id));
  if(!chosen.length)return;
  historyBusy=true;historyNotice=`正在手动处理 ${chosen.length} 个会话…`;renderHistory();
  let done=0;const errors=[];
  for(const session of chosen) {
    try {
      const item=await api('/api/history/import',{method:'POST',body:{host:historyHost,root:historyRoot,file:session.file,fingerprint:session.fingerprint},timeout:120000});
      session.status='captured';session.captures=[item];session.detail='已手动提交，后续进度见“自动化与同步 → 采集与审核”。';historySelected.delete(session.session_id);
      done++;
      const result=await api(`/api/history/${encodeURIComponent(item.id)}/prepare`,{method:'POST',body:{},timeout:90000});
      session.captures=[result.item];
      if(result.report.failed)errors.push(`${session.title}：候选生成失败，请在采集与审核中查看原因并重试。`);
    } catch(e){errors.push(`${session.title}：${e.message}。如请求超时，请重新扫描确认状态。`);}
  }
  historyBusy=false;historyNotice=`已提交 ${done} 个会话，候选需审核后收录。${errors.join(' ')}`;
  if(currentView==='history')renderHistory();
}
