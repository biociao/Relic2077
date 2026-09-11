'use strict';

// The distribution view of the memory library. It is derived entirely from
// GET /api/stats, which answers the same filters as the list below it, so every
// chart describes exactly the memories on screen. Nothing here is persisted and
// nothing is fetched from outside the local server.
(function () {
  const STATUS_LABELS = { active: '活跃', fading: '渐淡', superseded: '已替代', archived: '已归档' };
  const KIND_LABELS = { knowledge: '知识', decision: '决策', pattern: '模式', lesson: '经验', reflection: '反思' };
  const SLICES = 6;
  const SERIES = 6;

  const text = value => String(value ?? '');
  const number = value => new Intl.NumberFormat('zh-CN').format(value ?? 0);
  const percent = value => Number.isFinite(value) ? `${Math.round(value * 100)}%` : '—';
  const fixed = (value, digits = 2) => Number.isFinite(value) ? value.toFixed(digits) : '—';
  const safe = value => text(value).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  // Labels come from stored tags and agents: bound what a chart has to draw.
  const short = (value, limit = 18) => { const label = text(value); return label.length > limit ? `${label.slice(0, limit - 1)}…` : label; };

  const total = items => (items || []).reduce((sum, item) => sum + (item.count || 0), 0);

  function arcPath(cx, cy, radius, inner, from, to) {
    if (to - from >= Math.PI * 2 - 1e-6) {
      // A single category is a full ring; one arc cannot close on itself.
      return `M ${cx} ${cy - radius} A ${radius} ${radius} 0 1 1 ${cx} ${cy + radius} A ${radius} ${radius} 0 1 1 ${cx} ${cy - radius} Z `
        + `M ${cx} ${cy - inner} A ${inner} ${inner} 0 1 0 ${cx} ${cy + inner} A ${inner} ${inner} 0 1 0 ${cx} ${cy - inner} Z`;
    }
    const point = (r, angle) => `${(cx + r * Math.cos(angle)).toFixed(2)} ${(cy + r * Math.sin(angle)).toFixed(2)}`;
    const large = to - from > Math.PI ? 1 : 0;
    return `M ${point(radius, from)} A ${radius} ${radius} 0 ${large} 1 ${point(radius, to)} `
      + `L ${point(inner, to)} A ${inner} ${inner} 0 ${large} 0 ${point(inner, from)} Z`;
  }

  /// Ranked frequency chart. Ranking is the point, so magnitudes are drawn as
  /// proportional widths while the count keeps its exact value as a label.
  function rankChart(items, emptyNote, options = {}) {
    const labels = options.labels || {};
    const ranked = (items || []).filter(item => item.count > 0).sort((a, b) => b.count - a.count || text(a.name).localeCompare(text(b.name)));
    if (!ranked.length) return `<p class="stats-empty">${safe(emptyNote)}</p>`;
    const shown = ranked.slice(0, SERIES);
    const rest = ranked.slice(SERIES);
    const scale = Math.max(1, shown[0].count);
    const rowHeight = 26, x = 0, width = 260;
    const rows = shown.map((item, index) => {
      const y = index * rowHeight;
      const value = item.count / scale * width;
      // Short bars keep their count outside, so a number never outgrows the bar
      // that explains it.
      const inside = value > 62;
      const point = inside
        ? { cx: x + value - 7, cy: y + 27 }
        : { cx: x + Math.max(value, 2) + 4, cy: y + 27 };
      const label = short(labels[item.name] || item.name);
      return `<g class="rank-row"><title>${safe(item.name)}：${number(item.count)} 条${total(items) ? ` · ${percent(item.count / total(items))}` : ''}</title>`
        + `<text class="rank-name" x="${x}" y="${y + 12}">${safe(label)}</text>`
        + `<rect class="rank-track" x="${x}" y="${y + 14}" width="${width}" height="4" rx="2"/>`
        + `<rect class="rank-fill fill-${index % 6}" x="${x}" y="${y + 14}" width="${value.toFixed(2)}" height="4" rx="2"/>`
        + `<text class="rank-value" x="${x + width}" y="${y + 12}" text-anchor="end">${number(item.count)}</text>`
        + `<text class="rank-out${inside ? ' hidden' : ''}" x="${point.cx.toFixed(1)}" y="${point.cy}">${number(item.count)}</text>`
        + `</g>`;
    }).join('');
    const overflow = rest.length
      ? `<text class="rank-rest" x="0" y="${shown.length * rowHeight + 4}">其他 ${rest.length} 项 · 共 ${number(total(rest))} 条</text>`
      : '';
    const height = shown.length * rowHeight + (rest.length ? 16 : 2);
    return `<div class="chart rank-chart"><svg viewBox="0 0 260 ${height}" role="img" aria-label="${safe(options.aria || '分布')}：${safe(shown.map(item => `${item.name} ${item.count} 条`).join('，'))}">${rows}${overflow}</svg></div>`;
  }

  /// Lifecycle as a single normalized band plus exact rows: the stored status is
  /// authoritative, so it is never inferred from a confidence threshold.
  function lifecycleChart(items, scope) {
    const ranked = (items || []).filter(item => item.count > 0).sort((a, b) => b.count - a.count);
    if (!ranked.length) return `<p class="stats-empty">${safe('当前范围内没有记忆。')}</p>`;
    const sum = total(ranked);
    const order = ['active', 'fading', 'superseded', 'archived'];
    const band = order
      .map(key => { const item = ranked.find(entry => entry.name === key); return item ? `<span class="band-${key}" data-width="${item.count / sum * 100}" title="${safe(STATUS_LABELS[key] || key)} ${number(item.count)} 条"></span>` : ''; })
      .join('');
    const rows = order
      .map(key => { const item = ranked.find(entry => entry.name === key); return item ? { name: key, count: item.count } : null; })
      .filter(Boolean)
      .concat(ranked.filter(item => !order.includes(item.name)))
      .map(item => `<div class="stats-row"><i class="band-${item.name}"></i><span>${safe(STATUS_LABELS[item.name] || item.name)}</span><strong>${number(item.count)}</strong><span class="stats-share">${percent(item.count / sum)}</span></div>`)
      .join('');
    return `<div class="lifecycle-stats"><div class="segment-bar" aria-hidden="true">${band}</div>${rows}</div>`
      + `<p class="stats-note">范围内共 ${number(sum)} / ${number(scope?.vault_total ?? sum)} 条记忆</p>`;
  }

  /// Catmull-Rom through the bin centres, clamped to the drawable area. A bin
  /// density can be effectively zero next to a tall spike; without the clamp
  /// the curve would leave the viewBox and be cropped mid-stroke.
  function smoothPath(points, clamp) {
    if (points.length < 2) return '';
    const at = (x, y) => [Math.min(clamp.x1, Math.max(clamp.x0, x)), Math.min(clamp.y1, Math.max(clamp.y0, y))];
    let path = `M ${at(points[0][0], points[0][1]).map(value => value.toFixed(2)).join(' ')}`;
    for (let index = 0; index < points.length - 1; index += 1) {
      const previous = points[index - 1] || points[index];
      const current = points[index];
      const next = points[index + 1];
      const after = points[index + 2] || next;
      const first = at(current[0] + (next[0] - previous[0]) / 6, current[1] + (next[1] - previous[1]) / 6);
      const second = at(next[0] - (after[0] - current[0]) / 6, next[1] - (after[1] - current[1]) / 6);
      const end = at(next[0], next[1]);
      path += ` C ${first[0].toFixed(2)} ${first[1].toFixed(2)} ${second[0].toFixed(2)} ${second[1].toFixed(2)} ${end[0].toFixed(2)} ${end[1].toFixed(2)}`;
    }
    return path;
  }

  /// Density plus histogram plus cut-off markers. A mean alone cannot show
  /// whether trust is concentrated or spread, and decay moves it over time.
  ///
  /// Two observations share the plot: the histogram is real counts and the
  /// curve is a kernel density. Each is normalized to its own peak so the shape
  /// stays readable next to a single tall spike, and both axes are labelled so
  /// neither is misread as the other.
  function densityChart(confidence, scope) {
    const bins = confidence?.bins || [];
    if (!scope?.total || !bins.length) return `<p class="stats-empty">${safe('当前范围内没有可统计的置信度。')}</p>`;
    const width = 620, height = 212, left = 26, right = 612, top = 26, bottom = 166;
    const x = value => left + (right - left) * value;
    const peakDensity = Math.max(1e-9, ...bins.map(bin => bin.density));
    const peakCount = Math.max(1, ...bins.map(bin => bin.count));
    const y = value => bottom - (bottom - top) * (value / (peakDensity * 1.1));
    const yCount = value => bottom - (bottom - top) * (value / (peakCount * 1.1));
    const step = (right - left) / bins.length;
    const bars = bins.map((bin, index) => {
      const barX = left + index * step + 2;
      return `<rect class="density-bar" x="${barX.toFixed(1)}" y="${yCount(bin.count).toFixed(1)}" width="${(step - 4).toFixed(1)}" height="${Math.max(0, bottom - yCount(bin.count)).toFixed(1)}" rx="2"><title>${safe(bin.label)}：${number(bin.count)} 条 · ${percent(scope.total ? bin.count / scope.total : 0)}</title></rect>`;
    }).join('');
    const points = [[left, y(bins[0]?.density || 0)]];
    bins.forEach((bin, index) => points.push([left + (index + 0.5) * step, y(bin.density)]));
    points.push([right, y(bins[bins.length - 1]?.density || 0)]);
    const clamp = { x0: left, x1: right, y0: top, y1: bottom };
    const line = smoothPath(points, clamp);
    const area = line ? `${line} L ${right.toFixed(2)} ${bottom.toFixed(2)} L ${left.toFixed(2)} ${bottom.toFixed(2)} Z` : '';
    const ticks = [0, 0.2, 0.4, 0.6, 0.8, 1].map(value => `<line class="gridline" x1="${x(value).toFixed(1)}" x2="${x(value).toFixed(1)}" y1="${top}" y2="${bottom}"/><text x="${x(value).toFixed(1)}" y="183" text-anchor="middle">${value.toFixed(1)}</text>`).join('');
    const mean = Number.isFinite(confidence.mean) && confidence.mean > 0 ? confidence.mean : null;
    const mark = value => `<line class="density-mark" x1="${x(value).toFixed(1)}" x2="${x(value).toFixed(1)}" y1="${top}" y2="${bottom}"/>`;
    const marker = mean == null ? '' : mark(mean) + `<text class="density-mark-label" x="${x(mean).toFixed(1)}" y="${top - 15}" text-anchor="${mean > 0.85 ? 'end' : mean < 0.15 ? 'start' : 'middle'}">均值 ${fixed(mean)}</text>`;
    const threshold = Number.isFinite(confidence.decay_threshold) && confidence.decay_threshold > 0 && confidence.decay_threshold < 1
      ? mark(confidence.decay_threshold) + `<text class="density-mark-label threshold" x="${x(confidence.decay_threshold).toFixed(1)}" y="${bottom + 15}" text-anchor="middle">复核阈值 ${fixed(confidence.decay_threshold)}</text>`
      : '';
    return `<div class="chart density-chart"><svg viewBox="0 0 ${width} ${height}" role="img" aria-label="有效置信度分布：均值 ${fixed(mean)}，中位数 ${fixed(confidence.median)}，标准差 ${fixed(confidence.standard_deviation, 3)}">${ticks}${bars}${area ? `<path class="density-area" d="${area}"/>` : ''}${line ? `<path class="density-line" d="${line}"/>` : ''}${marker}${threshold}<text class="axis-label" x="${left}" y="${height - 10}">柱=条数，峰值 ${number(peakCount)} 条</text><text class="axis-label" x="${(left + right) / 2}" y="${height - 10}" text-anchor="middle">曲线=核密度估计（带宽 ${fixed(confidence.bandwidth, 3)}）</text><text class="axis-label" x="${right}" y="${height - 10}" text-anchor="end">有效置信度（已计入衰减）</text></svg></div>`;
  }

  /// The question the view exists to answer: how many memories survive each
  /// minimum-confidence cut-off, and how that compares with the scope.
  function thresholdSection(confidence, scope) {
    const levels = confidence?.thresholds || [];
    if (!scope?.total || !levels.length) return `<p class="stats-empty">${safe('当前范围内没有记忆。')}</p>`;
    return `<div class="threshold-grid">${levels.map(level => {
      const share = scope.total ? level.above / scope.total : 0;
      return `<div class="threshold-card${level.level === confidence.decay_threshold ? ' current' : ''}"><div class="threshold-top"><span>≥ ${percent(level.level)}</span><strong>${number(level.above)}</strong></div><div class="mini-bar"><span data-width="${(share * 100).toFixed(2)}"></span></div><p>满足 ${percent(share)} · 低于此值 ${number(level.below)} 条${level.level === confidence.decay_threshold ? ' · 当前复核阈值' : ''}</p></div>`;
    }).join('')}</div>`;
  }

  function kindChart(items, scope) {
    const ranked = (items || []).filter(item => item.count > 0).sort((a, b) => b.count - a.count);
    if (!ranked.length) return `<p class="stats-empty">${safe('当前范围内没有记忆。')}</p>`;
    const sum = total(ranked);
    const cx = 100, cy = 92, radius = 74, inner = 47;
    let angle = -Math.PI / 2;
    const slices = ranked.slice(0, SLICES).map((item, index) => {
      const sweep = sum ? item.count / sum * Math.PI * 2 : 0;
      const path = `<path class="slice slice-${index % SLICES}" d="${arcPath(cx, cy, radius, inner, angle, angle + sweep)}"><title>${safe(KIND_LABELS[item.name] || item.name)}：${number(item.count)} 条 · ${percent(sum ? item.count / sum : 0)}</title></path>`;
      const middle = angle + sweep / 2;
      const label = sweep > 0.35
        ? `<text x="${(cx + (radius + 12) * Math.cos(middle)).toFixed(1)}" y="${(cy + (radius + 12) * Math.sin(middle) + 3).toFixed(1)}" text-anchor="${Math.cos(middle) > 0.2 ? 'start' : Math.cos(middle) < -0.2 ? 'end' : 'middle'}" class="slice-label">${safe(KIND_LABELS[item.name] || short(item.name, 8))}</text>`
        : '';
      angle += sweep;
      return path + label;
    }).join('');
    const legend = ranked.slice(0, SLICES).map((item, index) => `<div class="stats-row"><i class="slice-${index % SLICES}"></i><span>${safe(KIND_LABELS[item.name] || item.name)}</span><strong>${number(item.count)}</strong><span class="stats-share">${percent(sum ? item.count / sum : 0)}</span></div>`).join('');
    const rest = ranked.length > SLICES ? `<p class="stats-note">其他 ${ranked.length - SLICES} 类 · ${number(total(ranked.slice(SLICES)))} 条</p>` : '';
    return `<div class="donut-wrap"><div class="chart donut-chart"><svg viewBox="0 0 196 184" role="img" aria-label="记忆类型分布：${safe(ranked.map(item => `${item.name} ${item.count} 条`).join('，'))}">${slices}<text class="donut-value" x="${cx}" y="${cy - 2}" text-anchor="middle">${number(sum)}</text><text class="donut-caption" x="${cx}" y="${cy + 14}" text-anchor="middle">条记忆</text></svg></div><div class="donut-legend">${legend}${rest}</div></div>`;
  }

  const chart = (title, note, body, extra = '') => `<div class="stats-chart"><div class="stats-chart-head"><h3>${safe(title)}</h3><span class="stats-chart-note">${safe(note)}</span></div>${body}${extra}</div>`;

  function render(data) {
    const scope = data?.scope || {};
    const distribution = data?.distribution || {};
    const confidence = data?.confidence || {};
    const note = scope.filtered ? '当前筛选结果' : '全部记忆';
    return `<div class="stats-grid">`
      + chart('记忆类型', note, kindChart(distribution.by_kind, scope))
      + chart('生命周期', '当前状态', lifecycleChart(distribution.by_status, scope))
      + chart('置信度分布', `n = ${number(scope.total)}`, densityChart(confidence, scope))
      + chart('最低置信度下的分布', '≥ 阈值的记忆', thresholdSection(confidence, scope))
      + chart('高频标签', `${number(distribution.by_tag?.length || 0)} 个标签`, rankChart(distribution.by_tag, '当前范围内没有标签。', { aria: '标签分布' }))
      + chart('来源 Agent', `${number(distribution.by_agent?.length || 0)} 个来源`, rankChart(distribution.by_agent, '当前范围内没有来源信息。', { aria: '来源 Agent 分布' }))
      + `</div>`;
  }

  function renderShell(state) {
    return `<div class="stats-panel${state.collapsed ? ' collapsed' : ''}" id="memory-stats">`
      + `<div class="panel-header"><div><h2>记忆分布统计</h2><p id="stats-scope">按记忆类型、生命周期、标签、来源与置信度统计当前筛选结果</p></div>`
      + `<div class="stats-head-actions"><span class="subtle-label" id="stats-scope-tag">读取中</span><button class="button ghost small" data-action="toggle-stats" aria-expanded="${state.collapsed ? 'false' : 'true'}" aria-controls="stats-body">${state.collapsed ? '展开统计' : '收起统计'}</button></div></div>`
      + `<div id="stats-body">${state.collapsed ? '' : `<div class="stats-loading"><span class="spinner"></span>正在统计…</div>`}</div></div>`;
  }

  const state = { collapsed: false, load: 0, data: null };

  function paint(container) {
    if (!container || !state.data) return;
    // Keep the shell (and the toggle) in place; only the body is redrawn.
    const body = container.querySelector('#stats-body');
    const scope = state.data.scope || {};
    if (body) body.innerHTML = render(state.data);
    const tag = container.querySelector('#stats-scope-tag');
    if (tag) tag.textContent = scope.filtered ? `筛选后 ${number(scope.total)} / ${number(scope.vault_total)} 条` : `${number(scope.total)} 条记忆`;
    const caption = container.querySelector('#stats-scope');
    if (caption) caption.textContent = scope.filtered
      ? '统计当前筛选结果的构成；清空筛选可查看整个知识库的分布'
      : '整个知识库的构成：类型、生命周期、标签、来源与置信度';
    window.RelicApp?.applyBars?.(container);
  }

  const api = {
    collapsed: () => state.collapsed,

    /// Rebuild the shell for a view change, preserving the collapse preference.
    reset(container, { collapsed }) {
      if (!container) return;
      state.collapsed = Boolean(collapsed);
      state.data = null;
      container.innerHTML = renderShell(state);
    },

    toggle(container, collapsed) {
      if (!container) return;
      state.collapsed = collapsed == null ? !state.collapsed : Boolean(collapsed);
      if (state.collapsed) {
        const panel = container.querySelector('.stats-panel');
        panel?.classList.add('collapsed');
        const body = container.querySelector('#stats-body');
        if (body) body.innerHTML = '';
      } else {
        container.innerHTML = renderShell(state);
        paint(container);
      }
      const button = container.querySelector('[data-action="toggle-stats"]');
      if (button) {
        button.textContent = state.collapsed ? '展开统计' : '收起统计';
        button.setAttribute('aria-expanded', String(!state.collapsed));
      }
      return state.collapsed;
    },

    /// Fetch the distribution for the current filters and paint it. A failure
    /// leaves the memory list untouched and states the reason in place.
    async load(container, params) {
      if (!container) return null;
      if (state.collapsed) return null;
      const request = ++state.load;
      try {
        const response = await fetch(`/api/stats?${params}`, { headers: { Accept: 'application/json' }, cache: 'no-store' });
        const data = await response.json();
        if (!response.ok) throw new Error(data.error || `统计请求失败 (${response.status})`);
        if (request !== state.load) return null;
        state.data = data;
        paint(container);
        return data;
      } catch (error) {
        if (request !== state.load) return null;
        const body = container.querySelector('#stats-body');
        if (body) body.innerHTML = `<div class="error-banner" role="alert">${safe(error.message)}</div><button class="button small" data-action="reload-stats">重新统计</button>`;
        const tag = container.querySelector('#stats-scope-tag');
        if (tag) tag.textContent = '统计不可用';
        return null;
      }
    },
  };

  window.RelicStats = api;
})();
