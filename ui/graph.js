// Relation-network view for the Relic dashboard.
//
// This renders the same derived graph the CLI and MCP tools serve — typed edges,
// each with the derivation and evidence behind it — rather than a decorative
// aggregation. Nothing here is inferred in the browser: the layout is the only
// thing the client computes, and every line on screen is backed by a relation
// the reader can inspect by clicking it.
(function () {
  'use strict';

  const KINDS = {
    link: { label: '显式链接', color: '#7eb8ff', dash: '' },
    supersedes: { label: '版本替代', color: '#f2c14e', dash: '6 4' },
    tag: { label: '共同主题', color: '#8f7bff', dash: '2 4' },
    semantic: { label: '语义相近', color: '#4fd1a5', dash: '' },
    contradicts: { label: '结论冲突', color: '#ff6b81', dash: '6 3' },
    corroborates: { label: '结论互证', color: '#59c9e8', dash: '3 3' },
  };

  const DERIVATIONS = { explicit: '显式声明', statistical: '统计推导', logical: '逻辑推导' };

  const graphEscape = (value) => String(value ?? '').replace(/[&<>"']/g, (c) => (
    { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]
  ));

  // The relation kinds the reader has switched off, and the weakest relation
  // still worth drawing. Both are device preferences, never vault state.
  const hidden = new Set();
  let minimumWeight = 0;
  let current = null;

  const nodesById = (graph) => new Map(graph.nodes.map((node) => [node.id, node]));

  // A compact, deterministic force layout. Deterministic matters: the same vault
  // must look the same on every load, so positions come from a hash of the node
  // ID instead of a random seed.
  function seedPosition(id, index, total, width, height) {
    let hash = 2166136261;
    for (let i = 0; i < id.length; i += 1) {
      hash ^= id.charCodeAt(i);
      hash = Math.imul(hash, 16777619) >>> 0;
    }
    const angle = ((hash % 4096) / 4096) * Math.PI * 2 + (index / Math.max(total, 1)) * 0.4;
    const radius = 0.28 + ((hash >>> 12) % 512) / 512 * 0.32;
    return {
      x: width / 2 + Math.cos(angle) * width * radius,
      y: height / 2 + Math.sin(angle) * height * radius,
    };
  }

  function layout(graph, width, height) {
    const nodes = graph.nodes.map((node, index) => Object.assign({}, node, seedPosition(node.id, index, graph.nodes.length, width, height)));
    const byId = new Map(nodes.map((node) => [node.id, node]));
    const links = graph.edges
      .map((edge) => ({ edge, source: byId.get(edge.from), target: byId.get(edge.to) }))
      .filter((link) => link.source && link.target);

    // Fruchterman-Reingold style relaxation: repulsion between every pair,
    // attraction along edges, and a weak pull toward the centre so isolated
    // memories do not drift away from the network they belong to.
    const area = width * height;
    const ideal = Math.sqrt(area / Math.max(nodes.length, 1)) * 0.7;
    let temperature = Math.min(width, height) * 0.12;
    for (let step = 0; step < 220; step += 1) {
      for (const node of nodes) { node.dx = 0; node.dy = 0; }
      for (let i = 0; i < nodes.length; i += 1) {
        for (let j = i + 1; j < nodes.length; j += 1) {
          const a = nodes[i];
          const b = nodes[j];
          let dx = a.x - b.x;
          let dy = a.y - b.y;
          let distance = Math.hypot(dx, dy);
          if (distance < 0.01) { dx = 0.01 * (i + 1); dy = 0.01 * (j + 1); distance = 0.02; }
          const force = (ideal * ideal) / distance;
          const ux = (dx / distance) * force;
          const uy = (dy / distance) * force;
          a.dx += ux; a.dy += uy; b.dx -= ux; b.dy -= uy;
        }
      }
      for (const link of links) {
        let dx = link.source.x - link.target.x;
        let dy = link.source.y - link.target.y;
        const distance = Math.max(Math.hypot(dx, dy), 0.01);
        const force = (distance * distance) / ideal;
        const ux = (dx / distance) * force;
        const uy = (dy / distance) * force;
        link.source.dx -= ux; link.source.dy -= uy;
        link.target.dx += ux; link.target.dy += uy;
      }
      for (const node of nodes) {
        node.dx += (width / 2 - node.x) * 0.012;
        node.dy += (height / 2 - node.y) * 0.012;
      }
      for (const node of nodes) {
        const magnitude = Math.hypot(node.dx, node.dy) || 1;
        const applied = Math.min(magnitude, temperature);
        node.x += (node.dx / magnitude) * applied;
        node.y += (node.dy / magnitude) * applied;
        node.x = Math.max(24, Math.min(width - 24, node.x));
        node.y = Math.max(24, Math.min(height - 24, node.y));
      }
      temperature *= 0.985;
    }
    const settled = new Map(nodes.map((node) => [node.id, node]));
    return { nodes, links: links.map((link) => ({ edge: link.edge, source: settled.get(link.edge.from), target: settled.get(link.edge.to) })) };
  }

  function nodeRadius(node, maximum) {
    if (maximum <= 0) return 7;
    return 6 + Math.sqrt(node.degree / maximum) * 12;
  }

  function renderSvg(graph, width, height) {
    const { nodes, links } = layout(graph, width, height);
    const maximum = nodes.reduce((best, node) => Math.max(best, node.degree), 0);
    const positions = new Map(nodes.map((node) => [node.id, node]));
    const parts = [];

    // Edges first so nodes sit above them.
    for (const link of links) {
      const style = KINDS[link.edge.kind] || { color: '#8fa3b8', dash: '' };
      const width_px = 0.6 + link.edge.weight * 2.2;
      const mid = { x: (link.source.x + link.target.x) / 2, y: (link.source.y + link.target.y) / 2 };
      parts.push(`<line class="graph-edge" data-edge="${graphEscape(link.edge.from)}|${graphEscape(link.edge.to)}|${graphEscape(link.edge.kind)}"
        x1="${link.source.x.toFixed(1)}" y1="${link.source.y.toFixed(1)}"
        x2="${link.target.x.toFixed(1)}" y2="${link.target.y.toFixed(1)}"
        stroke="${style.color}" stroke-width="${width_px.toFixed(2)}"
        ${style.dash ? `stroke-dasharray="${style.dash}"` : ''}
        stroke-linecap="round" opacity="${(0.35 + link.edge.weight * 0.5).toFixed(2)}"></line>`);
      // A faint label on the strongest edges, so the picture states what the
      // lines mean without becoming unreadable.
      if (link.edge.weight >= 0.6) {
        parts.push(`<text class="graph-edge-label" x="${mid.x.toFixed(1)}" y="${mid.y.toFixed(1)}" fill="${style.color}" text-anchor="middle">${graphEscape((KINDS[link.edge.kind] || {}).label || link.edge.kind)}</text>`);
      }
    }

    for (const node of nodes) {
      const radius = nodeRadius(node, maximum);
      parts.push(`<g class="graph-node" data-node="${graphEscape(node.id)}" tabindex="0" role="button"
        aria-label="${graphEscape(`${node.title}，${node.degree} 条关系`)}">
        <circle cx="${node.x.toFixed(1)}" cy="${node.y.toFixed(1)}" r="${radius.toFixed(1)}"
          fill="${node.degree === 0 ? 'var(--graph-isolated)' : 'var(--graph-node)'}" stroke="var(--graph-node-stroke)" stroke-width="1.5"></circle>
        <text x="${node.x.toFixed(1)}" y="${(node.y + radius + 13).toFixed(1)}" text-anchor="middle">${graphEscape(node.title.length > 18 ? `${node.title.slice(0, 17)}…` : node.title)}</text>
      </g>`);
    }

    return { svg: parts.join(''), positions };
  }

  function relationList(graph) {
    const byId = nodesById(graph);
    const title = (id) => (byId.get(id) || {}).title || id;
    return graph.edges.slice().sort((a, b) => b.weight - a.weight).slice(0, 40).map((edge) => {
      const style = KINDS[edge.kind] || { label: edge.kind, color: '#8fa3b8' };
      return `<li class="graph-relation">
        <div class="graph-relation-head">
          <span class="graph-relation-kind" style="--kind-color:${style.color}">${graphEscape(style.label)}</span>
          <span class="graph-relation-weight">${edge.weight.toFixed(3)}</span>
          <span class="graph-relation-derivation">${graphEscape(DERIVATIONS[edge.derivation] || edge.derivation)}</span>
        </div>
        <p class="graph-relation-pair">${graphEscape(title(edge.from))} <span aria-hidden="true">↔</span> ${graphEscape(title(edge.to))}</p>
        <p class="graph-relation-evidence">${graphEscape(edge.evidence)}</p>
      </li>`;
    }).join('');
  }

  function legend() {
    return Object.entries(KINDS).map(([kind, style]) => `<button type="button" class="graph-legend-item${hidden.has(kind) ? ' off' : ''}" data-kind="${kind}" aria-pressed="${hidden.has(kind) ? 'false' : 'true'}">
      <span class="graph-legend-swatch" style="--kind-color:${style.color};--kind-dash:${style.dash || 'none'}"></span>${graphEscape(style.label)}
    </button>`).join('');
  }

  function visibleGraph(graph) {
    const edges = graph.edges.filter((edge) => !hidden.has(edge.kind) && edge.weight >= minimumWeight);
    const linked = new Set();
    for (const edge of edges) { linked.add(edge.from); linked.add(edge.to); }
    const nodes = graph.nodes.filter((node) => linked.has(node.id) || node.degree === 0);
    const degree = new Map();
    for (const edge of edges) {
      degree.set(edge.from, (degree.get(edge.from) || 0) + 1);
      degree.set(edge.to, (degree.get(edge.to) || 0) + 1);
    }
    return {
      nodes: nodes.map((node) => Object.assign({}, node, { degree: degree.get(node.id) || 0 })),
      edges,
    };
  }

  function summary(graph) {
    const stats = graph.stats || {};
    const kinds = Object.entries(stats.edges_by_kind || {})
      .map(([kind, count]) => `<span class="graph-chip"><span class="graph-legend-swatch" style="--kind-color:${(KINDS[kind] || {}).color || '#8fa3b8'}"></span>${graphEscape((KINDS[kind] || {}).label || kind)} ${count}</span>`)
      .join('');
    return `<div class="graph-summary">
      <span class="graph-chip"><b>${stats.nodes ?? graph.nodes.length}</b> 记忆</span>
      <span class="graph-chip"><b>${stats.edges ?? graph.edges.length}</b> 关系</span>
      <span class="graph-chip"><b>${stats.components ?? '—'}</b> 连通分量</span>
      <span class="graph-chip"><b>${stats.isolated ?? '—'}</b> 孤立记忆</span>
      ${kinds}
    </div>`;
  }

  function notices(graph) {
    const lines = [];
    if (graph.omitted_nodes > 0 || graph.omitted_edges > 0) {
      lines.push(`为保证可读性，视图中省略了 ${graph.omitted_nodes} 条记忆与 ${graph.omitted_edges} 条关系；命令行与 MCP 工具可读取完整图谱。`);
    }
    if (graph.min_weight > 0) {
      lines.push(`已隐藏权重低于 ${graph.min_weight} 的关系。`);
    }
    for (const note of graph.notes || []) { lines.push(note); }
    if (!lines.length) return '';
    return `<div class="graph-notes">${lines.map((line) => `<p>${graphEscape(line)}</p>`).join('')}</div>`;
  }

  function draw(root, graph) {
    current = graph;
    const view = visibleGraph(graph);
    const stage = root.querySelector('#graph-stage');
    const width = Math.max(stage.clientWidth || 900, 480);
    const height = Math.max(Math.round(width * 0.56), 360);
    const { svg } = renderSvg(view, width, height);
    root.querySelector('#graph-canvas').innerHTML = svg;
    root.querySelector('#graph-canvas').setAttribute('viewBox', `0 0 ${width} ${height}`);
    root.querySelector('#graph-summary').innerHTML = summary(graph);
    root.querySelector('#graph-relations').innerHTML = view.edges.length
      ? relationList({ nodes: graph.nodes, edges: view.edges })
      : '<li class="graph-relation-empty">当前筛选下没有关系。</li>';
    root.querySelector('#graph-notices').innerHTML = notices(graph);
    root.querySelector('#graph-count').textContent = `${view.nodes.length} 条记忆 · ${view.edges.length} 条关系`;
  }

  function brokenDown(title, body) {
    return `<section class="panel"><div class="panel-header"><h2>${graphEscape(title)}</h2></div><p class="graph-caption">${graphEscape(body)}</p></section>`;
  }

  function mount(node, data) {
    const graph = data;
    node.innerHTML = `
      <div class="graph-layout">
        <section class="panel graph-panel">
          <div class="panel-header">
            <div><h2>关系网络</h2><p class="graph-caption">每条连线都是一种带证据的关系：显式链接、版本替代、共同主题、语义相近、结论冲突或结论互证。点击连线或节点查看依据。</p></div>
            <span class="subtle-label" id="graph-count"></span>
          </div>
          <div class="graph-summary" id="graph-summary"></div>
          <div class="graph-toolbar">
            <div class="graph-legend" id="graph-legend">${legend()}</div>
            <label class="graph-slider">最低权重 <input type="range" id="graph-weight" min="0" max="1" step="0.05" value="${minimumWeight}"><output id="graph-weight-value">${minimumWeight.toFixed(2)}</output></label>
          </div>
          <div class="graph-stage" id="graph-stage"><svg id="graph-canvas" role="img" aria-label="记忆关系网络图"></svg></div>
          <div id="graph-notices"></div>
        </section>
        <aside class="panel graph-side">
          <div class="panel-header"><h2>关系明细</h2><span class="subtle-label">按权重排序</span></div>
          <ul class="graph-relations" id="graph-relations"></ul>
        </aside>
      </div>
      <section class="panel"><div class="panel-header"><h2>关于这张图</h2></div>
        <div class="note">
          <p>图与命令行 <code>relic graph</code>、MCP 工具读取的是同一份推导结果：Markdown 是唯一事实来源，图谱与向量层都可随时删除重建。</p>
          <p>“语义相近”由本地哈希向量空间（TF-IDF + 余弦）推导，不调用任何模型或网络；“结论冲突”是关键词极性对比得到的复核信号，不代表哪条记忆正确。</p>
          <p>节点大小表示关系数量。孤立记忆表示它尚未与任何记忆建立关系，通常意味着需要补充标签或链接。</p>
        </div>
      </section>`;

    const redraw = () => draw(node, graph);
    redraw();

    node.querySelector('#graph-legend').addEventListener('click', (event) => {
      const button = event.target.closest('[data-kind]');
      if (!button) return;
      const kind = button.dataset.kind;
      if (hidden.has(kind)) hidden.delete(kind); else hidden.add(kind);
      node.querySelector('#graph-legend').innerHTML = legend();
      redraw();
    });

    const slider = node.querySelector('#graph-weight');
    slider.addEventListener('input', () => {
      minimumWeight = Number(slider.value);
      node.querySelector('#graph-weight-value').textContent = minimumWeight.toFixed(2);
      redraw();
    });

    // Node click opens the memory itself, through the same dialog the rest of the
    // dashboard uses. The relation view is a way in, never a separate data path.
    const canvas = node.querySelector('#graph-canvas');
    canvas.addEventListener('click', (event) => {
      const group = event.target.closest('[data-node]');
      if (group) { window.RelicGraphOpen?.(group.dataset.node); return; }
      const line = event.target.closest('[data-edge]');
      if (line) {
        const [from, to, kind] = line.dataset.edge.split('|');
        const edge = graph.edges.find((item) => item.from === from && item.to === to && item.kind === kind);
        if (edge) window.RelicGraphToast?.(edge.evidence);
      }
    });
    canvas.addEventListener('keydown', (event) => {
      const group = event.target.closest('[data-node]');
      if (group && (event.key === 'Enter' || event.key === ' ')) {
        event.preventDefault();
        window.RelicGraphOpen?.(group.dataset.node);
      }
    });

    let timer = null;
    window.addEventListener('resize', () => {
      clearTimeout(timer);
      timer = setTimeout(() => { if (document.body.contains(node) && current) redraw(); }, 180);
    });
  }

  window.RelicGraph = {
    render(node, data) { mount(node, data); },
  };
}());
