'use strict';

// Relic memory brain: a dependency-free canvas 2D pseudo-3D knowledge graph.
//
// The banner is a second reading of /api/brain, never a second source of truth.
// Every number shown here comes from that payload (stored Markdown memories);
// nothing is invented locally and no third-party runtime is loaded, which keeps
// the dashboard working with the strict same-origin CSP and with no network.
//
// Accessibility: the canvas is decorative. One overlay button per primary node
// carries the same information for keyboard, screen-reader and touch use, and
// the tooltip is rendered from those same fields.
window.RelicBrain = (function () {
  const TAU = Math.PI * 2;
  const ENTITY_LABEL = { knowledge: '知识', decision: '决策', pattern: '模式', lesson: '经验', reflection: '反思' };
  const NODE_BUDGET = { maxTopics: 7, maxTags: 26, maxKeywords: 20, maxAgents: 8 };
  const SURFACE_POINTS = 300;
  const CORE_POINTS = 140;
  // World size of the sphere (SHAPE_RADIUS) plus the perspective and drag
  // headroom the camera fit must allow for on both axes.
  const FIT_WIDTH = 232;
  const FIT_HEIGHT = 216;
  // The volume may slightly overflow the frame: culled edges read as a sphere
  // larger than the banner, while the fit keeps every node on screen.
  const FIT_FILL = 1.06;
  // Membrane points live on this sphere; satellite anchors are scaled into the
  // same volume, so the picture is a sphere inside a shell of orbiting nodes.
  const SHAPE_RADIUS = 100;
  const REST_PITCH = -0.12;
  // Canvas cannot resolve CSS custom properties inside a font shorthand, so the
  // family list is stated once here and kept equal to --sans in styles.css.
  const FONT_UI = 'Inter,-apple-system,BlinkMacSystemFont,"Segoe UI","PingFang SC","Microsoft YaHei",sans-serif';
  const HINT_DEFAULT = '拖动球形旋转 · 悬停查看主题与关键词 · 点击查看该类记忆';

  // A sphere, with a hair of deterministic surface relief so the membrane reads
  // as a volume rather than a flat disc.
  function shape(x, y, z) {
    const relief = 1 + 0.025 * Math.sin(x * 0.21) * Math.cos(y * 0.17) * Math.sin(z * 0.13);
    return { x: x * relief, y: y * relief, z: z * relief, fissure: 0 };
  }

  function hash(text) {
    let value = 2166136261;
    for (let index = 0; index < text.length; index += 1) {
      value ^= text.charCodeAt(index);
      value = Math.imul(value, 16777619);
    }
    return (value >>> 0) / 4294967295;
  }

  function cloud(count, inner) {
    const points = [];
    const golden = Math.PI * (3 - Math.sqrt(5));
    for (let index = 0; index < count; index += 1) {
      // Fibonacci sphere keeps an even membrane instead of polar clumps.
      const y = 1 - (index + 0.5) / count * 2;
      const ring = Math.sqrt(Math.max(0, 1 - y * y));
      const theta = index * golden;
      let x = Math.cos(theta) * ring;
      let z = Math.sin(theta) * ring;
      const normalized = Math.hypot(x, y, z) || 1;
      x /= normalized; z /= normalized;
      if (inner) {
        const shrink = 0.34 + 0.5 * hash(`${index}:r`);
        x *= shrink; z *= shrink;
      }
      const point = shape(x * SHAPE_RADIUS, y * SHAPE_RADIUS, z * SHAPE_RADIUS);
      // Same field names as every other node: one projection path for the
      // membrane, the satellites and the primary nodes.
      points.push({
        anchorX: point.x,
        anchorY: point.y,
        anchorZ: point.z,
        inner: Boolean(inner),
        seed: hash(`${index}:s${inner ? 'i' : 'o'}`),
        fissure: point.fissure,
      });
    }
    return points;
  }

  // Topic anchors on the sphere surface. Topics arrive ranked by memory count and
  // are placed in rank order, so the largest cluster takes the front position.
  const TOPIC_ANCHORS = [
    [-62, 52, 50],
    [66, 46, 42],
    [0, 78, -30],
    [-70, -34, 40],
    [64, -40, -34],
    [4, -66, 46],
    [-18, 10, -92],
  ];
  // Source agents orbit outside the sphere as a halo, clear of everything else.
  const AGENT_HALO = 132;

  function rgba(color, alpha) {
    const [r, g, b] = color;
    return `rgba(${r},${g},${b},${Math.max(0, Math.min(1, alpha)).toFixed(3)})`;
  }
  function mix(a, b, t) { return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]; }
  function cssColor(value, fallback) {
    const text = String(value || '').trim();
    if (!text) return fallback;
    const short = text.match(/^#([0-9a-f]{3})$/i);
    if (short) return [0, 1, 2].map(index => parseInt(short[1][index] + short[1][index], 16));
    const long = text.match(/^#([0-9a-f]{6})$/i);
    if (long) return [0, 2, 4].map(index => parseInt(long[1].slice(index, index + 2), 16));
    const rgb = text.match(/^rgba?\(([^)]+)\)$/i);
    if (rgb) {
      const parts = rgb[1].split(/[,/\s]+/).filter(Boolean).slice(0, 3).map(Number);
      if (parts.length === 3 && parts.every(Number.isFinite)) return parts;
    }
    return fallback;
  }
  const pct = value => Number.isFinite(value) ? `${Math.round(value * 100)}%` : '—';
  const count = value => Number.isFinite(value) ? new Intl.NumberFormat('zh-CN').format(value) : '—';
  const short = (text, limit) => {
    const value = String(text ?? '');
    return value.length > limit ? `${value.slice(0, limit - 1)}…` : value;
  };

  let elements = { banner: null, canvas: null, overlay: null, tooltip: null, stage: null, toggle: null };
  let graph = null;
  let nodeModel = [];
  let cloudModel = null;
  let palette = null;
  let pointer = { dragging: false, lastX: 0 };
  let hovered = null;
  let focused = null;
  let yaw = -1.05;
  let yawVelocity = 0;
  let yawBase = 0.22;
  let pitch = REST_PITCH;
  let idleSince = 0;
  let lastFrame = 0;
  let lastOverlay = { yaw: NaN, pitch: NaN };
  let centerY = 0.5;
  let width = 0, height = 0, dpr = 1, scale = 6;
  let running = false;
  let visible = true;
  let reduced = false;
  let lastError = '';

  function readPalette() {
    const styles = getComputedStyle(document.documentElement);
    const accent = cssColor(styles.getPropertyValue('--accent'), [126, 218, 244]);
    const core = cssColor(styles.getPropertyValue('--core'), [255, 96, 118]);
    return {
      accent,
      core,
      wire: mix(accent, core, 0.4).map(Math.round),
      muted: cssColor(styles.getPropertyValue('--muted'), [142, 164, 184]),
      faint: cssColor(styles.getPropertyValue('--faint'), [112, 137, 158]),
      text: cssColor(styles.getPropertyValue('--text'), [220, 231, 240]),
      line: cssColor(styles.getPropertyValue('--line'), [42, 60, 76]),
    };
  }

  function buildGraph(payload) {
    graph = payload;
    const topics = Array.isArray(payload?.topics) ? payload.topics : [];
    const tags = Array.isArray(payload?.tags) ? payload.tags : [];
    const keywords = Array.isArray(payload?.keywords) ? payload.keywords : [];
    const agents = Array.isArray(payload?.agents) ? payload.agents : [];
    // Shared denominator, so "share" stays a fraction of the whole vault and
    // not of the truncated node list.
    const total = Number(payload?.stats?.total) || topics.reduce((sum, topic) => sum + (Number(topic.count) || 0), 0) || 0;
    const maxTopic = Math.max(1, ...topics.map(topic => Number(topic.count) || 0));
    const maxSatellite = Math.max(1, ...[...tags, ...keywords, ...agents].map(node => Number(node.count) || 0));
    nodeModel = [];

    topics.slice(0, NODE_BUDGET.maxTopics).forEach((topic, index) => {
      const anchor = TOPIC_ANCHORS[index % TOPIC_ANCHORS.length];
      const point = shape(anchor[0], anchor[1], anchor[2]);
      const cluster = {
        id: String(topic.id || `topic:${index}`),
        group: 'topic',
        kind: '',
        basis: String(topic.basis || 'subject'),
        label: short(String(topic.label || `主题 ${index + 1}`), 22),
        fullLabel: String(topic.label || `主题 ${index + 1}`),
        count: Number(topic.count) || 0,
        share: Number.isFinite(topic.share) ? topic.share : (total ? (Number(topic.count) || 0) / total : 0),
        confidence: Number.isFinite(topic.average_confidence) ? topic.average_confidence : null,
        statuses: topic.statuses || {},
        kinds: topic.kinds || {},
        tagsOfTopic: Array.isArray(topic.tags) ? topic.tags : [],
        keywordsOfTopic: Array.isArray(topic.keywords) ? topic.keywords : [],
        agentsOfTopic: Array.isArray(topic.agents) ? topic.agents : [],
        examples: Array.isArray(topic.examples) ? topic.examples : [],
        updated: topic.updated || '',
        radius: 6.4 + 7.2 * Math.sqrt((Number(topic.count) || 0) / maxTopic),
        anchorX: point.x * 0.99,
        anchorY: point.y * 0.99,
        anchorZ: point.z * 0.99,
      };
      nodeModel.push(cluster);
    });

    const topicFor = id => nodeModel.find(node => node.group === 'topic' && node.id === id);
    // Satellites orbit the topic they were attached to in the payload, scaled into
    // the same volume: tags and keywords inside the sphere, agents outside it.
    const attach = (list, group, budget) => {
      list.slice(0, budget).forEach((entry, index) => {
        const id = String(entry.id);
        const edge = (payload.edges || []).find(item => item.to === `${group}:${id}`) || null;
        const parent = (edge && topicFor(edge.from)) || nodeModel[index % Math.max(1, nodeModel.length)];
        if (!parent) return;
        const angle = hash(id) * TAU + index * 1.618;
        const vertical = 1 - (index % 3) * 0.26;
        const reach = group === 'agent' ? 1.34 : 0.58;
        const radius = group === 'tag'
          ? 1.7 + 2.6 * Math.sqrt((Number(entry.count) || 0) / maxSatellite)
          : group === 'keyword'
            ? 2.0 + 2.8 * Math.sqrt((Number(entry.count) || 0) / maxSatellite)
            : 2.6 + 3.4 * Math.sqrt((Number(entry.count) || 0) / maxSatellite);
        nodeModel.push({
          id,
          group,
          label: short(entry.label ?? id, 12),
          fullLabel: String(entry.label ?? id),
          count: Number(entry.count) || 0,
          kinds: entry.kinds || {},
          example: entry.example || null,
          radius,
          parentId: parent.id,
          anchorX: parent.anchorX * reach + Math.cos(angle) * 26 * vertical,
          anchorY: parent.anchorY * reach + Math.sin(angle) * 20,
          anchorZ: parent.anchorZ * reach + Math.sin(angle * 0.7) * 26,
        });
      });
    };
    attach(tags, 'tag', NODE_BUDGET.maxTags);
    attach(keywords, 'keyword', NODE_BUDGET.maxKeywords);

    // Source agents describe where memories came from, so they orbit the whole
    // volume as an outer halo instead of joining the tag swarm: two nodes at the
    // same place would make one unreachable and its tooltip wrong.
    agents.slice(0, NODE_BUDGET.maxAgents).forEach((entry, index) => {
      const id = String(entry.id);
      const edge = (payload.edges || []).find(item => item.to === `agent:${id}`) || null;
      const parent = edge ? topicFor(edge.from) : null;
      const theta = Math.PI * (0.3 + 0.4 * hash(`agent-shell:${index}`));
      const phi = hash(`agent-phi:${id}`) * TAU;
      const push = AGENT_HALO * (0.72 + 0.28 * hash(`agent-radius:${id}`));
      nodeModel.push({
        id,
        group: 'agent',
        label: short(entry.label ?? id, 12),
        fullLabel: String(entry.label ?? id),
        count: Number(entry.count) || 0,
        kinds: entry.kinds || {},
        example: entry.example || null,
        radius: 2.6 + 3.4 * Math.sqrt((Number(entry.count) || 0) / maxSatellite),
        parentId: parent ? parent.id : null,
        anchorX: Math.cos(phi) * Math.sin(theta) * push,
        anchorY: Math.cos(theta) * push * 0.55,
        anchorZ: Math.sin(phi) * Math.sin(theta) * push,
      });
    });

    separateNodes();
    recentre();

    const ids = new Set(nodeModel.map(node => node.id));
    const seen = new Set();
    graph.hidden = {
      topics: Number(payload.truncated?.topics) || 0,
      tags: Number(payload.truncated?.tags) || 0,
      keywords: Number(payload.truncated?.keywords) || 0,
      agents: Number(payload.truncated?.agents) || 0,
    };
    graph.links = (payload.edges || [])
      .filter(edge => ids.has(String(edge.from)) && ids.has(String(edge.to)))
      .map(edge => ({ from: String(edge.from), to: String(edge.to), weight: Number(edge.weight) || 1 }))
      .filter(edge => { const key = `${edge.from}>${edge.to}`; if (seen.has(key)) return false; seen.add(key); return true; });
    const maxWeight = Math.max(1, ...graph.links.map(edge => edge.weight));
    graph.links.forEach(edge => { edge.strength = 0.34 + 0.66 * (edge.weight / maxWeight); });
    if (!cloudModel) cloudModel = { surface: cloud(SURFACE_POINTS, false), core: cloud(CORE_POINTS, true) };
    renderOverlay();
    syncStats();
  }

  /// Collapse the pile-ups that a rotation-invariant layout always produces:
  /// walk the anchors apart until satellites are visually separable. Anchors are
  /// 3D and only the spatial envelope is corrected, so this cannot invent data.
  const SEPARATION_PX = 17;
  /// Spread colliding satellites until they are individually reachable.
  ///
  /// Accumulated (Jacobi) displacement rather than pairwise pushing: pairwise
  /// pushes oscillate and never settle on a fixed primary node. Anchors are 3D
  /// and only their positions are corrected — no counts, labels, or edges are
  /// invented, so the picture stays a faithful reading of /api/brain.
  function separateNodes() {
    const minimum = SEPARATION_PX;
    const spin = () => ({ yaw: 0, pitch: REST_PITCH, cx: 0, cy: 0, time: 0 });
    const shift = nodeModel.map(() => ({ x: 0, y: 0, z: 0, count: 0 }));
    for (let pass = 0; pass < 40; pass += 1) {
      // Compare the way the reader sees it: depth changes the effective spacing,
      // so a world-space check over-separates near nodes and leaves far ones on
      // top of each other.
      const projected = nodeModel.map(node => project(node, spin()));
      let worst = 0;
      for (let index = 0; index < nodeModel.length; index += 1) {
        shift[index].x = 0; shift[index].y = 0; shift[index].z = 0; shift[index].count = 0;
      }
      for (let a = 0; a < nodeModel.length; a += 1) {
        for (let b = a + 1; b < nodeModel.length; b += 1) {
          const first = nodeModel[a], second = nodeModel[b];
          const sx = projected[b].x - projected[a].x;
          const sy = projected[b].y - projected[a].y;
          const screenDistance = Math.hypot(sx, sy);
          // Primary nodes carry a real label, so satellites keep further clear of
          // them than they do of each other.
          const reach = minimum + (first.group === 'topic' || second.group === 'topic' ? 11 : 0);
          if (screenDistance >= reach) continue;
          worst = Math.max(worst, reach - screenDistance);
          // Push along the screen-space direction, expressed back in world units.
          const push = (reach - screenDistance) / Math.max(0.35, scale);
          let ux, uy, uz;
          if (screenDistance < 0.5) {
            // Exact ties must break deterministically, or the pass never settles.
            const angle = hash(`${first.id}|${second.id}`) * TAU;
            ux = Math.cos(angle); uy = Math.sin(angle) * 0.4; uz = Math.sin(angle);
          } else {
            ux = sx / screenDistance; uy = sy / screenDistance; uz = (sx / screenDistance) * 0.4;
          }
          if (first.group !== 'topic') { shift[a].x -= ux * push; shift[a].y -= uy * push; shift[a].z -= uz * push; shift[a].count += 1; }
          if (second.group !== 'topic') { shift[b].x += ux * push; shift[b].y += uy * push; shift[b].z += uz * push; shift[b].count += 1; }
        }
      }
      if (worst === 0) break;
      for (let index = 0; index < nodeModel.length; index += 1) {
        if (!shift[index].count) continue;
        const damp = 0.6 / shift[index].count;
        nodeModel[index].anchorX += shift[index].x * damp;
        nodeModel[index].anchorY += shift[index].y * damp;
        nodeModel[index].anchorZ += shift[index].z * damp;
      }
    }
  }

  function recentre() {
    if (!nodeModel.length) return;
    const mean = nodeModel.reduce((sum, node) => sum + node.anchorY, 0) / nodeModel.length;
    centerY = Math.max(0.36, Math.min(0.62, 0.5 - (mean * scale) / Math.max(1, height)));
    lastOverlay = { yaw: NaN, pitch: NaN };
  }

  function syncStats() {
    const stats = graph?.stats || {};
    const hidden = graph?.hidden || { tags: 0, agents: 0 };
    const hint = document.querySelector('#brain-hint');
    if (hint) {
      const parts = [];
      if (hidden.topics) parts.push(`${hidden.topics} 个小主题`);
      if (hidden.tags) parts.push(`${hidden.tags} 个标签`);
      if (hidden.keywords) parts.push(`${hidden.keywords} 个关键词`);
      if (hidden.agents) parts.push(`${hidden.agents} 个来源`);
      hint.textContent = parts.length
        ? `未入图：${parts.join('、')}（按记忆数排序，完整清单见记忆库）`
        : HINT_DEFAULT;
    }
    const set = (selector, value) => { const node = document.querySelector(selector); if (node) node.textContent = value; };
    set('#brain-total', count(stats.total));
    set('#brain-active', count(stats.active));
    set('#brain-confidence', Number.isFinite(stats.average_confidence) ? pct(stats.average_confidence) : '—');
    set('#brain-review', count(stats.needs_review));
    const wrap = document.querySelector('#brain-review-wrap');
    if (wrap) wrap.classList.toggle('alert', Boolean(stats.needs_review));
    const signal = document.querySelector('#brain-signal');
    if (signal) signal.title = graph?.vault?.name ? `知识库：${graph.vault.name}` : '';
  }

  // Keyboard and touch travel this path instead of the canvas hit test.
  function renderOverlay() {
    if (!elements.overlay) return;
    elements.overlay.textContent = '';
    nodeModel.filter(node => node.group === 'topic').forEach(node => {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'brain-node-button';
      button.dataset.brain = node.id;
      button.dataset.basis = node.basis || '';
      // The click handler must not have to look the label up again.
      button.dataset.brainLabel = node.fullLabel || node.label;
      button.dataset.action = 'brain-filter';
      button.textContent = node.label;
      button.setAttribute('aria-label', describe(node, true));
      elements.overlay.appendChild(button);
      button.addEventListener('focus', () => { focused = node.id; showTooltip(node); idleSince = performance.now(); });
      button.addEventListener('blur', () => { focused = null; hideTooltip(); });
    });
    if (!elements.overlay.childElementCount) {
      const note = document.createElement('span');
      note.className = 'brain-node-button empty';
      note.textContent = '知识库还没有记忆，图谱会在第一条记忆写入后出现。';
      elements.overlay.appendChild(note);
    }
  }

  function describe(node, verbose = false) {
    if (node.group === 'topic') {
      const parts = [`主题「${node.fullLabel}」：${count(node.count)} 条记忆`, `占全部 ${pct(node.share)}`];
      if (Number.isFinite(node.confidence)) parts.push(`平均置信度 ${pct(node.confidence)}`);
      const kinds = Object.entries(node.kinds || {}).sort((a, b) => b[1] - a[1])
        .map(([key, value]) => `${ENTITY_LABEL[key] || key} ${value}`).join(' · ');
      if (kinds) parts.push(`类型构成 ${kinds}`);
      if (node.keywordsOfTopic?.length) parts.push(`关键词 ${node.keywordsOfTopic.slice(0, 4).join('、')}`);
      if (node.examples[0]) parts.push(`例：${short(node.examples[0].title, 34)}`);
      return parts.join('，');
    }
    const groupLabel = { tag: '标签', keyword: '关键词', agent: '来源 Agent' }[node.group] || '节点';
    const kinds = Object.entries(node.kinds || {}).sort((a, b) => b[1] - a[1])
      .map(([key, value]) => `${ENTITY_LABEL[key] || key} ${value}`).join(' · ');
    const parts = [`${groupLabel}「${node.fullLabel}」：${count(node.count)} 条记忆`];
    if (kinds) parts.push(kinds);
    if (node.example?.title) parts.push(`例：${short(node.example.title, 34)}`);
    if (verbose && node.group === 'agent') parts.push('来源为记忆记录，不代表该 Agent 当前在线');
    return parts.join('，');
  }

  const statusLabel = key => ({ active: '活跃', fading: '渐淡', superseded: '已替代', archived: '已归档' }[key] || key);

  function tooltipHtml(node) {
    const basisLabel = { subject: '标题主题', tag: '标签聚类', kind: '按类型归类', keyword: '关键词聚类' };
    const group = node.group === 'topic'
      ? `记忆主题 · ${basisLabel[node.basis] || '聚类'}`
      : { tag: '标签', keyword: '关键词', agent: '来源 Agent' }[node.group] || '节点';
    const rows = [];
    rows.push(`<span class="brain-tt-count">${count(node.count)}<small>条记忆</small></span>`);
    if (node.group === 'topic') {
      rows.push(`<span class="brain-tt-share">占全部 ${pct(node.share)}</span>`);
      if (Number.isFinite(node.confidence)) rows.push(`<span class="brain-tt-share">平均置信度 ${pct(node.confidence)}</span>`);
      const statuses = Object.entries(node.statuses || {});
      if (statuses.length) {
        rows.push(`<span class="brain-tt-bar" aria-hidden="true">${statuses.map(([key, value]) => `<i class="${key}" style="flex:${Math.max(1, value)}"></i>`).join('')}</span>`);
        rows.push(`<span class="brain-tt-legend">${statuses.map(([key, value]) => `${statusLabel(key)} ${value}`).join(' · ')}</span>`);
      }
      const kinds = Object.entries(node.kinds || {}).sort((a, b) => b[1] - a[1]);
      if (kinds.length) {
        rows.push(`<span class="brain-tt-legend">类型构成：${kinds.map(([key, value]) => `${ENTITY_LABEL[key] || key} ${value}`).join(' · ')}</span>`);
      }
      if (node.tagsOfTopic?.length) {
        rows.push(`<span class="brain-tt-legend">标签：${node.tagsOfTopic.map(escapeHtml).join(' · ')}</span>`);
      }
      if (node.keywordsOfTopic?.length) {
        rows.push(`<span class="brain-tt-legend">关键词：${node.keywordsOfTopic.map(escapeHtml).join(' · ')}</span>`);
      }
      if (node.agentsOfTopic?.length) {
        rows.push(`<span class="brain-tt-legend">来源：${node.agentsOfTopic.map(escapeHtml).join(' · ')}</span>`);
      }
    } else {
      const kinds = Object.entries(node.kinds || {}).sort((a, b) => b[1] - a[1]).slice(0, 4);
      if (kinds.length) {
        rows.push(`<span class="brain-tt-legend">类型构成：${kinds.map(([key, value]) => `${ENTITY_LABEL[key] || key} ${value}`).join(' · ')}</span>`);
      }
    }
    const examples = node.group === 'topic' ? node.examples.slice(0, 3) : (node.example?.title ? [node.example] : []);
    const exampleHtml = examples.length
      ? `<ul class="brain-tt-examples">${examples.map(item => `<li><button type="button" data-entry="${escapeAttr(item.id)}">${escapeHtml(short(item.title, 42))}</button></li>`).join('')}</ul>`
      : '<p class="brain-tt-empty">这一类还没有记忆</p>';
    const footnote = node.group === 'agent'
      ? '来源为记忆记录中的元数据，不代表该 Agent 当前在线或活跃。'
      : node.group === 'topic'
        ? '点击可查看该主题下的全部记忆。'
        : node.group === 'keyword'
          ? '关键词来自记忆标题：点击后在记忆库中按该词搜索。'
          : '点击节点可在记忆库中按该标签筛选。';
    return `<div class="brain-tt-head"><span class="brain-tt-group">${group}</span><strong>${escapeHtml(node.fullLabel || node.label)}</strong></div>`
      + `<div class="brain-tt-metrics">${rows.join('')}</div>`
      + exampleHtml
      + `<p class="brain-tt-foot">${escapeHtml(footnote)}</p>`;
  }

  function escapeHtml(value) {
    return String(value ?? '').replace(/[&<>"']/g, character => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[character]));
  }
  const escapeAttr = escapeHtml;

  function showTooltip(node) {
    if (!elements.tooltip) return;
    elements.tooltip.innerHTML = tooltipHtml(node);
    elements.tooltip.dataset.group = node.group;
    elements.tooltip.classList.add('visible');
    if (node.hit) positionTooltip(node.hit.x, node.hit.y);
    else positionTooltip(width / 2, height / 2);
  }
  function hideTooltip() {
    if (elements.tooltip) elements.tooltip.classList.remove('visible');
  }
  function positionTooltip(x, y) {
    const box = elements.tooltip;
    if (!box) return;
    const boxWidth = box.offsetWidth || 232;
    const boxHeight = box.offsetHeight || 130;
    let left = x + 16;
    if (left + boxWidth > width - 12) left = Math.max(12, x - boxWidth - 16);
    let top = y - boxHeight - 14;
    if (top < 10) top = Math.min(height - boxHeight - 10, y + 18);
    box.style.transform = `translate(${Math.round(left)}px,${Math.round(Math.max(10, top))}px)`;
  }

  function overlaySize(node) { return Math.max(10, node.radius * scale * 0.62 + (node.group === 'topic' ? 5 : 4)); }

  function project(node, spin) {
    const cos = Math.cos(spin.pitch), sin = Math.sin(spin.pitch);
    const y = node.anchorY * cos - node.anchorZ * sin;
    const z = node.anchorY * sin + node.anchorZ * cos;
    const cx = Math.cos(spin.yaw), sx = Math.sin(spin.yaw);
    const x = node.anchorX * cx + z * sx;
    const depth = -node.anchorX * sx + z * cx;
    const focal = 380;
    const perspective = focal / (focal + depth);
    return {
      x: spin.cx + x * scale * perspective,
      y: spin.cy + y * scale * perspective,
      depth,
      perspective,
      r: node.radius * scale * 0.62 * perspective,
    };
  }

  function drawPoints(context, points, spin, inner) {
    for (const point of points) {
      const projected = project(point, spin);
      if (projected.x < -40 || projected.x > width + 40) continue;
      const near = Math.max(0, Math.min(1, 1.05 - (projected.depth + 90) / 190));
      const flicker = reduced ? 0.72 : 0.62 + 0.38 * Math.sin(spin.time * 0.0013 + point.seed * TAU);
      const alpha = (inner ? 0.055 : 0.085) + 0.3 * near * flicker;
      const size = (inner ? 0.4 : 0.55) + (inner ? 0.5 : 1.05) * near;
      const tint = point.anchorY > 12 ? 0.42 : point.anchorY < -34 ? 0.62 : 0.16;
      context.fillStyle = rgba(mix(palette.accent, palette.core, tint * (0.4 + 0.6 * point.seed)), alpha * (1 - 0.5 * point.fissure));
      context.beginPath();
      context.arc(projected.x, projected.y, size, 0, TAU);
      context.fill();
    }
  }

  function drawLinks(context, spin) {
    const positions = new Map();
    for (const node of nodeModel) positions.set(node.id, project(node, spin));
    const active = hovered || focused;
    context.lineWidth = 1;
    for (const link of graph?.links || []) {
      const from = positions.get(link.from), to = positions.get(link.to);
      if (!from || !to) continue;
      const depth = (from.depth + to.depth) / 2;
      const near = Math.max(0, Math.min(1, 1 - (depth + 90) / 190));
      const isActive = active === link.from || active === link.to;
      const alpha = (isActive ? 0.46 : 0.055) + (isActive ? 0.3 : 0.075) * near * link.strength;
      context.strokeStyle = rgba(isActive ? palette.accent : palette.wire, alpha);
      context.beginPath();
      context.moveTo(from.x, from.y);
      context.lineTo(to.x, to.y);
      context.stroke();
    }
    return positions;
  }

  function drawNodes(context, spin, positions) {
    const active = hovered || focused;
    const order = { tag: 0, keyword: 1, agent: 2, topic: 3 };
    const ordered = [...nodeModel].sort((a, b) => (order[a.group] - order[b.group]) || ((positions.get(b.id)?.depth ?? 0) - (positions.get(a.id)?.depth ?? 0)));
    for (const node of ordered) {
      const projected = positions.get(node.id);
      if (!projected) continue;
      const near = Math.max(0, Math.min(1, 1 - (projected.depth + 90) / 190));
      const isActive = active === node.id;
      const isParent = active && (node.id === active || node.parentId === active);
      const primary = node.group === 'topic';
      const base = projected.r * (1 + (isActive ? 0.34 : 0));
      const color = primary
        ? palette.accent
        : node.group === 'agent'
          ? mix(palette.accent, palette.core, 0.55)
          : node.group === 'keyword'
            ? mix(palette.accent, palette.text, 0.35)
            : palette.accent;
      const alpha = (primary ? 0.68 : 0.3) + (primary ? 0.32 : 0.5) * near;
      if (primary || isActive) {
        context.beginPath();
        context.arc(projected.x, projected.y, base * (isActive ? 3.6 : 2.6), 0, TAU);
        context.fillStyle = rgba(color, (isActive ? 0.22 : 0.12) * (0.4 + 0.6 * near));
        context.fill();
      }
      context.beginPath();
      context.arc(projected.x, projected.y, base, 0, TAU);
      context.fillStyle = rgba(color, alpha * (isActive || isParent ? 1 : 0.34 + 0.66 * near));
      context.fill();
      context.lineWidth = primary ? 1.4 : 1;
      context.strokeStyle = rgba(primary ? palette.text : palette.accent, primary ? 0.5 + 0.3 * near : 0.28 + 0.4 * near);
      context.stroke();
      if ((primary || isActive || (node.group === 'agent' && near > 0.72)) && node.group) {
        context.font = primary ? `600 12.5px ${FONT_UI}` : `11px ${FONT_UI}`;
        context.textAlign = 'center';
        context.textBaseline = 'top';
        context.fillStyle = rgba(palette.text, primary ? 0.55 + 0.4 * near : 0.5 + 0.5 * near);
        context.fillText(node.label, projected.x, projected.y + base + (isActive ? 7 : 5));
      }
    }
  }

  function frame(now) {
    if (!running || !graph || !cloudModel) { running = false; return; }
    const context = elements.canvas?.getContext('2d');
    if (!context) return;
    const delta = Math.min(48, Math.max(8, now - (lastFrame || now)));
    lastFrame = now;
    if (!reduced) {
      // Entrance turn: the volume rotates into place once, then either coasts or
      // settles until the pointer takes over.
      if (yaw < yawBase - 0.004) yaw = Math.min(yawBase, yaw + delta * 0.0011);
      else if (!pointer.dragging && now - idleSince > 2600) {
        yawVelocity += (yawBase - yaw) * 0.00004 * delta;
        yawVelocity *= 0.94;
        yaw += yawVelocity * delta * 0.06 + Math.sin(now * 0.00013) * 0.0006 * delta;
      }
      pitch += (REST_PITCH - pitch) * 0.002;
    }
    const spin = { yaw, pitch, cx: width / 2, cy: height * centerY, time: now };
    context.setTransform(dpr, 0, 0, dpr, 0, 0);
    context.clearRect(0, 0, width, height);
    // Painter order matters: the atmosphere goes down first and the membrane is
    // never covered by it, so every lit pixel belongs to an actual node.
    const wash = context.createRadialGradient(width / 2, height * 0.46, 8, width / 2, height * 0.5, Math.max(width, height) * 0.62);
    wash.addColorStop(0, rgba(palette.accent, 0.09));
    wash.addColorStop(0.45, rgba(palette.core, 0.03));
    wash.addColorStop(1, rgba(palette.accent, 0));
    context.fillStyle = wash;
    context.fillRect(0, 0, width, height);

    drawPoints(context, cloudModel.core, spin, true);
    const positions = drawLinks(context, spin);
    drawPoints(context, cloudModel.surface, spin, false);
    drawNodes(context, spin, positions);

    if (hovered || focused) {
      const active = nodeModel.find(node => node.id === (hovered || focused));
      const projected = active && positions.get(active.id);
      if (active && projected) {
        active.hit = { x: projected.x, y: projected.y };
        positionTooltip(projected.x, projected.y);
      }
    } else if (elements.tooltip?.classList.contains('visible')) {
      hideTooltip();
    }
    // Reposition the assistive buttons only when the volume actually moved.
    // Doing it every frame forced a layout flush per frame for no visual gain.
    if (!Number.isFinite(lastOverlay.yaw) || Math.abs(lastOverlay.yaw - yaw) > 0.008 || Math.abs(lastOverlay.pitch - pitch) > 0.008) {
      lastOverlay = { yaw, pitch };
      syncOverlay(positions);
    }
    if (reduced) { running = false; return; }
    requestAnimationFrame(frame);
  }

  /// Keep the keyboard/touch buttons on top of their node. The buttons exist for
  /// assistive use, so they must not be left in the corner where a focus ring
  /// would point at nothing.
  function syncOverlay(positions) {
    if (!elements.overlay) return;
    for (const button of elements.overlay.querySelectorAll('button[data-brain]')) {
      const node = nodeModel.find(item => item.id === button.dataset.brain);
      const projected = node && positions.get(node.id);
      if (!projected) { button.hidden = true; continue; }
      const size = overlaySize(node);
      button.hidden = false;
      button.style.width = `${Math.round(size)}px`;
      button.style.height = `${Math.round(size)}px`;
      button.style.left = `${Math.round(projected.x)}px`;
      button.style.top = `${Math.round(projected.y)}px`;
    }
  }

  function resize() {
    if (!elements.canvas || !elements.stage) return;
    lastOverlay = { yaw: NaN, pitch: NaN };
    // The canvas fills the stage's content box, so the backing store must match
    // that box exactly (or the bitmap and the model drift apart by the border).
    const box = elements.canvas.getBoundingClientRect();
    const cssWidth = Math.max(240, Math.round(box.width || elements.stage.clientWidth || 0));
    const cssHeight = Math.max(120, Math.round(box.height || elements.stage.clientHeight || 0));
    dpr = Math.min(2, Math.max(1, window.devicePixelRatio || 1));
    width = cssWidth;
    height = cssHeight;
    if (elements.canvas.width !== Math.round(cssWidth * dpr)) elements.canvas.width = Math.round(cssWidth * dpr);
    if (elements.canvas.height !== Math.round(cssHeight * dpr)) elements.canvas.height = Math.round(cssHeight * dpr);
    // Fit the whole volume inside the banner on both axes; the canvas is much
    // wider than it is tall, so height is normally the binding constraint.
    scale = FIT_FILL * Math.min((cssWidth * 0.92) / FIT_WIDTH, (cssHeight * 0.94) / FIT_HEIGHT);
  }

  function hitTest(x, y, positions) {
    let best = null;
    for (const node of nodeModel) {
      const projected = positions.get(node.id);
      if (!projected) continue;
      const reach = Math.max(10, projected.r + (node.group === 'topic' ? 10 : 8));
      const distance = Math.hypot(projected.x - x, projected.y - y) + (node.group === 'tag' ? 1.5 : 0);
      if (distance <= reach && (!best || distance < best.distance)) best = { node, distance, x: projected.x, y: projected.y };
    }
    return best;
  }

  function positionsNow() {
    const spin = { yaw, pitch, cx: width / 2, cy: height * centerY, time: 0 };
    const positions = new Map();
    for (const node of nodeModel) positions.set(node.id, project(node, spin));
    return positions;
  }

  let pointerSeen = 0;

  function pointerMove(event) {
    // Prefer Pointer Events, but keep a mouse fallback so the banner still
    // tracks a cursor in engines or drivers that only synthesize MouseEvents.
    if (typeof PointerEvent === 'undefined' || !(event instanceof PointerEvent)) {
      if (performance.now() - pointerSeen < 500) return;
    } else {
      pointerSeen = performance.now();
    }
    const rect = elements.canvas.getBoundingClientRect();
    const x = event.clientX - rect.left, y = event.clientY - rect.top;
    if (pointer.dragging) {
      yawVelocity = (event.clientX - pointer.lastX) * 0.0016;
      yaw += (event.clientX - pointer.lastX) * 0.0042;
      pitch = Math.max(-0.5, Math.min(0.5, pitch + (event.movementY || 0) * 0.0022));
      pointer.lastX = event.clientX;
      idleSince = performance.now();
      return;
    }
    pointer.x = x; pointer.y = y;
    const found = hitTest(x, y, positionsNow());
    const next = found?.node?.id || null;
    if (next !== hovered) {
      hovered = next;
      if (found?.node) { found.node.hit = { x: found.x, y: found.y }; showTooltip(found.node); }
      else hideTooltip();
    } else if (found) {
      positionTooltip(found.x, found.y);
    }
  }

  function refresh() {
    [...nodeModel].forEach(node => { delete node.hit; });
    hovered = null;
    hideTooltip();
  }

  function setCollapsed(collapsed) {
    if (!elements.banner) return;
    elements.banner.classList.toggle('collapsed', collapsed);
    elements.toggle?.setAttribute('aria-expanded', String(!collapsed));
    const label = document.querySelector('#brain-toggle-label');
    if (label) label.textContent = collapsed ? '展开图谱' : '收起图谱';
    try { localStorage.setItem('relic-brain-collapsed', collapsed ? '1' : '0'); } catch { /* Preference is optional. */ }
    if (collapsed) { running = false; } else if (graph && visible) { start(); resize(); }
  }

  function start() {
    if (running || !graph || !cloudModel) return;
    running = true;
    lastFrame = 0;
    requestAnimationFrame(frame);
  }
  function stop() { running = false; }

  function attach() {
    elements = {
      banner: document.querySelector('#memory-brain'),
      stage: document.querySelector('#brain-stage'),
      canvas: document.querySelector('#brain-canvas'),
      overlay: document.querySelector('#brain-overlay'),
      tooltip: document.querySelector('#brain-tooltip'),
      hint: document.querySelector('#brain-hint'),
      toggle: document.querySelector('#brain-toggle'),
    };
    if (!elements.banner || !elements.canvas || !elements.stage) return false;
    palette = readPalette();
    reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

    elements.canvas.addEventListener('pointermove', pointerMove);
    elements.canvas.addEventListener('mousemove', pointerMove);
    elements.canvas.addEventListener('pointerleave', () => { hovered = null; pointer.active = false; hideTooltip(); });
    elements.canvas.addEventListener('pointerdown', event => {
      pointer.dragging = true; pointer.lastX = event.clientX;
      elements.canvas.setPointerCapture?.(event.pointerId);
      elements.canvas.classList.add('dragging');
    });
    elements.canvas.addEventListener('pointerup', event => {
      const travelled = Math.abs((event.clientX || 0) - pointer.lastX);
      pointer.dragging = false;
      elements.canvas.releasePointerCapture?.(event.pointerId);
      elements.canvas.classList.remove('dragging');
      // A drag is a rotation gesture; only a near-stationary release is a click.
      const node = hovered && nodeModel.find(item => item.id === hovered);
      if (node && travelled < 5) {
        const rect = elements.canvas.getBoundingClientRect();
        elements.canvas.dispatchEvent(new CustomEvent('relic:brain-node', {
          bubbles: true,
          detail: { id: node.id, group: node.group, label: node.fullLabel || node.label, x: event.clientX - rect.left, y: event.clientY - rect.top },
        }));
      }
    });
    window.addEventListener('resize', () => { resize(); refresh(); });
    // Moving between displays changes devicePixelRatio; the backing store has to
    // follow or the bitmap is drawn at the wrong scale.
    window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`).addEventListener?.('change', () => { resize(); refresh(); });
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) stop();
      else if (graph && visible && !elements.banner.classList.contains('collapsed')) start();
    });
    window.addEventListener('relic:theme', () => { palette = readPalette(); refresh(); });
    if ('ResizeObserver' in window) {
      new ResizeObserver(entries => {
        // A hidden banner reports a zero box; only a real box can be drawn into.
        const box = entries[0]?.contentRect;
        if (!box || box.width < 80 || box.height < 40) return;
        resize();
        refresh();
        if (!graph) ensureLoaded();
        else if (visible && !elements.banner.classList.contains('collapsed')) start();
      }).observe(elements.stage);
    }
    if ('IntersectionObserver' in window) {
      new IntersectionObserver(entries => {
        visible = entries.some(entry => entry.isIntersecting);
        if (!visible) stop();
        else if (graph && !elements.banner.classList.contains('collapsed')) start();
      }, { threshold: 0.05 }).observe(elements.banner);
    }
    if (window.matchMedia('(prefers-reduced-motion: reduce)').addEventListener) {
      window.matchMedia('(prefers-reduced-motion: reduce)').addEventListener('change', event => {
        reduced = event.matches;
        refresh();
      });
    }
    let collapsed = false;
    try { collapsed = localStorage.getItem('relic-brain-collapsed') === '1'; } catch { /* Preference is optional. */ }
    setCollapsed(collapsed);
    return true;
  }

  async function load() {
    if (!elements.banner) return null;
    try {
      const response = await fetch('/api/brain', { headers: { Accept: 'application/json' }, cache: 'no-store' });
      const payload = await response.json().catch(() => null);
      if (!response.ok) throw new Error(payload?.error || `请求失败 (${response.status})`);
      resize();
      buildGraph(payload);
      lastError = '';
      elements.banner.classList.remove('error');
      refresh();
      if (visible && !elements.banner.classList.contains('collapsed')) start();
      return payload;
    } catch (error) {
      lastError = error.message || '无法读取记忆图谱';
      elements.banner.classList.add('error');
      // Say so in the banner itself: the tooltip only exists while a node is
      // hovered, so a failure cannot live there.
      if (elements.hint) elements.hint.textContent = `记忆图谱暂时无法读取：${lastError}`;
      if (elements.overlay) {
        elements.overlay.textContent = '';
        const note = document.createElement('span');
        note.className = 'brain-node-button empty';
        note.textContent = '记忆本身仍然可用，请使用「记忆库」浏览或筛选。';
        elements.overlay.appendChild(note);
      }
      return null;
    }
  }

  /// Map a node to the memory-library filters it stands for.
  ///
  /// A topic is a cluster, not a stored field: its node id is only a sequence
  /// number, so the topic's own label is what the memory library can search for.
  /// The mapping is returned as a ready-made filter patch so the caller never has
  /// to re-parse an id.
  function filterFor(fullId, group, label) {
    const id = String(fullId || '');
    const prefix = id.includes(':') ? id.slice(0, id.indexOf(':')) : '';
    const value = label || (id.includes(':') ? id.slice(id.indexOf(':') + 1) : id);
    const reset = { q: '', kind: '', tag: '', source_agent: '' };
    if (prefix === 'topic' || group === 'topic') return { ...reset, q: value };
    if (prefix === 'keyword' || group === 'keyword') return { ...reset, q: value };
    if (prefix === 'tag' || group === 'tag') return { ...reset, tag: value };
    if (prefix === 'agent' || group === 'agent') return { ...reset, source_agent: value };
    return null;
  }

  let loading = null;

  /// Build the projection once the banner has a measurable box.
  ///
  /// The banner starts hidden (it belongs to the overview), and a hidden element
  /// has no size: loading before it is shown would size the canvas to the default
  /// 300x150 and project every node into the wrong frame.
  function ensureLoaded() {
    if (loading || graph) return loading;
    const box = elements.canvas?.getBoundingClientRect();
    if (!box || box.width < 80 || box.height < 40) return null;
    loading = load().finally(() => { loading = null; });
    return loading;
  }

  async function init() {
    if (!attach()) return null;
    document.querySelector('#brain-toggle')?.addEventListener('click', () => {
      setCollapsed(!elements.banner.classList.contains('collapsed'));
    });
    // Repaint on theme change without coupling to app.js internals.
    new MutationObserver(() => { palette = readPalette(); refresh(); })
      .observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    return ensureLoaded();
  }

  /// Called when the overview becomes visible (or the banner is expanded).
  async function show() {
    resize();
    await ensureLoaded();
    resize();
    refresh();
    if (visible && graph && !elements.banner.classList.contains('collapsed')) start();
  }

  /// Read-only view of the projection state, used by the verification harness to
  /// reconcile bitmap coordinates with CSS coordinates.
  function debugCloud() {
    const extent = points => {
      const xs = points.map(p => p.anchorX), ys = points.map(p => p.anchorY), zs = points.map(p => p.anchorZ);
      return { x: [Math.round(Math.min(...xs)), Math.round(Math.max(...xs))], y: [Math.round(Math.min(...ys)), Math.round(Math.max(...ys))], z: [Math.round(Math.min(...zs)), Math.round(Math.max(...zs))] };
    };
    if (!cloudModel) return null;
    // How spherical is the membrane really? Compare each point's radius against
    // the average: a sphere keeps this spread tiny.
    const radii = cloudModel.surface.map(point => Math.hypot(point.anchorX, point.anchorY, point.anchorZ));
    const mean = radii.reduce((sum, value) => sum + value, 0) / Math.max(1, radii.length);
    const deviation = Math.sqrt(radii.reduce((sum, value) => sum + (value - mean) ** 2, 0) / Math.max(1, radii.length));
    return {
      surface: extent(cloudModel.surface),
      core: extent(cloudModel.core),
      radius: Number(mean.toFixed(1)),
      radiusSpread: Number((deviation / mean).toFixed(4)),
    };
  }

  function debug() {
    const box = elements.canvas?.getBoundingClientRect();
    const spin = { yaw, pitch, cx: width / 2, cy: height * centerY, time: 0 };
    const nodes = nodeModel.map(node => {
      const projected = project(node, spin);
      return { id: node.id, group: node.group, screen: [Math.round(projected.x), Math.round(projected.y)], radius: Number(node.radius.toFixed(2)) };
    });
    return {
      width, height, dpr, scale: Number(scale.toFixed(3)), centerY: Number(centerY.toFixed(3)),
      canvasCss: box ? { w: Math.round(box.width), h: Math.round(box.height), left: Math.round(box.left), top: Math.round(box.top) } : null,
      bitmap: elements.canvas ? { w: elements.canvas.width, h: elements.canvas.height } : null,
      yaw, pitch, running, reduced, error: lastError, truncated: graph?.hidden || null,
      groups: nodeModel.reduce((counts, node) => { counts[node.group] = (counts[node.group] || 0) + 1; return counts; }, {}),
      cloud: debugCloud(), nodes,
      outside: nodes.filter(node => node.screen[0] < 4 || node.screen[0] > width - 4 || node.screen[1] < 4 || node.screen[1] > height - 4).map(node => node.id),
    };
  }

  return { init, load, show, resize, setCollapsed, describe, filterFor, debug, error: () => lastError };
})();
