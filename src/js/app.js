const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

let currentRange = '30d';
let advancedRange = 'all';
let currentProject = '';
let activeModels = ['fable', 'opus', 'sonnet', 'haiku', 'mythos', 'other'];
let dailyChart = null;
let modelChart = null;
let projectChart = null;
let refreshCountdown = 30;
let isScanning = false;
let currentSessions = [];
let sortCol = 'last_active';
let sortDir = 'desc';
let expandedSession = null;
let turnsCache = { sid: null, html: '' };
let renderedSessionsKey = '';

const C = {
  input:   '#4facfe',
  output:  '#00f2fe',
  cacheRd: '#fbbf24',
  cacheCr: '#f87171',
  grid:    'rgba(28,32,53,0.6)',
  fable:   '#e879f9',
  opus:    '#ff6b35',
  sonnet:  '#4facfe',
  haiku:   '#34d399',
  mythos:  '#a78bfa',
  other:   '#6b7280',
  text:    '#8890a8',
  cost:    '#34d399',
};

const MODEL_COLORS = {
  fable: C.fable,
  mythos: C.mythos,
  opus: C.opus,
  sonnet: C.sonnet,
  haiku: C.haiku,
  other: C.other,
};

function escHtml(s) {
  return String(s ?? '')
    .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

function errorMessage(err) {
  return String(err?.message ?? err);
}

const errorToasts = new Map();

function showError(label, err) {
  console.error(label, err);
  let toast = errorToasts.get(label);
  if (!toast) {
    toast = document.createElement('div');
    toast.className = 'error-toast';
    const text = document.createElement('span');
    text.className = 'error-toast-text';
    const close = document.createElement('button');
    close.type = 'button';
    close.className = 'alert-close';
    close.textContent = '×';
    close.addEventListener('click', () => clearError(label));
    toast.append(text, close);
    document.getElementById('error-toasts').appendChild(toast);
    errorToasts.set(label, toast);
  }
  toast.firstChild.textContent = label + ': ' + errorMessage(err);
}

function clearError(label) {
  const toast = errorToasts.get(label);
  if (toast) {
    toast.remove();
    errorToasts.delete(label);
  }
}

function coalesced(task) {
  let running = false;
  let queued = false;
  return async () => {
    if (running) {
      queued = true;
      return;
    }
    running = true;
    try {
      do {
        queued = false;
        await task();
      } while (queued);
    } finally {
      running = false;
    }
  };
}

const htmlCache = new WeakMap();

function setHtml(el, html) {
  if (htmlCache.get(el) === html) return;
  htmlCache.set(el, html);
  el.innerHTML = html;
}

function fmt(n) {
  if (n == null) return '--';
  const abs = Math.abs(n);
  if (abs >= 1e9) return (n / 1e9).toFixed(2) + 'B';
  if (abs >= 1e6) return (n / 1e6).toFixed(2) + 'M';
  if (abs >= 1e3) return (n / 1e3).toFixed(1) + 'K';
  return n.toLocaleString();
}

function fmtCost(n) {
  if (n == null) return '--';
  if (n >= 1000) return '$' + (n/1000).toFixed(1) + 'K';
  return '$' + n.toFixed(2);
}

function fmtDur(m) {
  if (!m || m < 1) return '<1m';
  if (m < 60) return m + 'm';
  return Math.floor(m/60) + 'h ' + (m%60) + 'm';
}

function fmtTime(iso) {
  if (!iso) return '';
  const d = new Date(iso);
  if (isNaN(d)) return String(iso);
  const month = d.toLocaleDateString('en', { month: 'short', day: 'numeric' });
  // hour12:false renders midnight as 24:xx in Chromium
  const time = d.toLocaleTimeString('en', { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' });
  return month + ' ' + time;
}

function fmtClock(utc) {
  // last_updated is "YYYY-MM-DD HH:MM:SS" in UTC without a zone marker
  const d = new Date(utc.replace(' ', 'T') + 'Z');
  if (isNaN(d)) return utc;
  return d.toLocaleTimeString('en', { hourCycle: 'h23' });
}

function modelKey(m) {
  const model = String(m || '').toLowerCase();
  if (model.includes('fable'))  return 'fable';
  if (model.includes('mythos')) return 'mythos';
  if (model.includes('opus'))   return 'opus';
  if (model.includes('sonnet')) return 'sonnet';
  if (model.includes('haiku'))  return 'haiku';
  return 'other';
}

Chart.defaults.font.family = "'JetBrains Mono', monospace";
Chart.defaults.color = C.text;
Chart.defaults.borderColor = C.grid;

function hideScanOverlay() {
  document.getElementById('scan-overlay').classList.add('hidden');
  document.getElementById('live-dot').classList.remove('scanning');
}

function showScanProgress(p) {
  const overlay = document.getElementById('scan-overlay');
  overlay.classList.remove('hidden');
  document.getElementById('live-dot').classList.add('scanning');

  if (!p || p.phase === 'discovering') {
    document.getElementById('scan-detail').textContent = 'Discovering session files...';
    document.getElementById('scan-files').textContent = '';
    document.getElementById('scan-turns').textContent = '';
    document.getElementById('scan-fill').style.width = '0%';
  } else if (p.phase === 'migrating') {
    document.getElementById('scan-detail').textContent = 'Upgrading the usage database (a backup is saved first)...';
    document.getElementById('scan-files').textContent = '';
    document.getElementById('scan-turns').textContent = '';
    document.getElementById('scan-fill').style.width = '0%';
  } else if (p.phase === 'scanning') {
    const pct = p.total > 0 ? Math.round((p.current / p.total) * 100) : 0;
    document.getElementById('scan-detail').textContent = p.current_project || 'Scanning...';
    document.getElementById('scan-files').textContent = p.current + ' / ' + p.total + ' files';
    document.getElementById('scan-turns').textContent = fmt(p.turns_found) + ' turns';
    document.getElementById('scan-fill').style.width = pct + '%';
  } else if (p.phase === 'done') {
    hideScanOverlay();
    isScanning = false;
  }
}

function forceShowLoader() {
  const overlay = document.getElementById('scan-overlay');
  overlay.classList.remove('hidden');
  document.getElementById('live-dot').classList.add('scanning');
  document.getElementById('scan-detail').textContent = 'Discovering session files...';
  document.getElementById('scan-fill').style.width = '0%';
}

const queryKey = () => JSON.stringify([currentRange, activeModels, currentProject]);
const advancedKey = () => JSON.stringify([advancedRange, activeModels, currentProject]);

const loadDashboard = coalesced(async () => {
  const key = queryKey();
  try {
    const data = await invoke('get_dashboard', {
      range: currentRange,
      models: activeModels,
      project: currentProject || null,
    });
    if (key !== queryKey()) return;
    renderStats(data.summary);
    renderDailyChart(data.daily);
    renderModelChart(data.by_model);
    renderProjectChart(data.top_projects);
    currentSessions = data.recent_sessions;
    sortSessions();
    renderTable(currentSessions);
    updateProjectFilter(data.projects);
    document.getElementById('last-updated').textContent = fmtClock(data.last_updated);
    clearError('Dashboard');
  } catch (e) {
    showError('Dashboard', e);
  }
  loadAdvanced();
});

const loadAdvanced = coalesced(async () => {
  const key = advancedKey();
  try {
    const adv = await invoke('get_advanced_stats', {
      range: advancedRange,
      models: activeModels,
      project: currentProject || null,
    });
    if (key !== advancedKey()) return;
    renderAdvanced(adv);
    clearError('Advanced stats');
  } catch (e) {
    showError('Advanced stats', e);
  }
});

function renderAdvanced(a) {
  const set = (id, v) => { const el = document.getElementById(id); if (el) el.textContent = v; };
  const setW = (id, pct) => { const el = document.getElementById(id); if (el) el.style.width = pct.toFixed(1) + '%'; };
  const estTokens = chars => Math.round((chars || 0) / 4);

  // Thinking tokens are billed as output but never logged, so they are the residual.
  const outputTotal = a.total_output_tokens || 0;
  const textEst = estTokens(a.text_chars);
  const toolEst = estTokens(a.tool_input_chars);
  const thinkEst = Math.max(0, outputTotal - textEst - toolEst);
  const denom = Math.max(1, outputTotal);

  set('compo-output-total', fmt(outputTotal) + ' tok');
  setW('compo-text-fill',     (textEst  / denom) * 100);
  setW('compo-tool-fill',     (toolEst  / denom) * 100);
  setW('compo-thinking-fill', (thinkEst / denom) * 100);
  set('compo-text-val',     `${fmt(textEst)} tok (${((textEst  / denom)*100).toFixed(1)}%)`);
  set('compo-tool-val',     `${fmt(toolEst)} tok (${((toolEst  / denom)*100).toFixed(1)}%)`);
  set('compo-thinking-val', `${fmt(thinkEst)} tok (${((thinkEst / denom)*100).toFixed(1)}%)`);
  set('compo-thinking-turns', fmt(a.turns_with_thinking || 0));
  set('compo-total-turns', fmt(a.total_turns || 0));

  const fresh = a.total_input_tokens || 0;
  const cWrite = a.total_cache_creation || 0;
  const cRead = a.total_cache_read || 0;
  const inputTotal = fresh + cWrite + cRead;
  const inputDenom = Math.max(1, inputTotal);

  set('cache-total-val', fmt(inputTotal) + ' tok');
  setW('cache-fresh-fill', (fresh / inputDenom) * 100);
  setW('cache-write-fill', (cWrite / inputDenom) * 100);
  setW('cache-read-fill',  (cRead / inputDenom) * 100);
  set('cache-fresh-val', `${fmt(fresh)} tok (${((fresh / inputDenom)*100).toFixed(1)}%)`);
  set('cache-write-val', `${fmt(cWrite)} tok (${((cWrite / inputDenom)*100).toFixed(1)}%)`);
  set('cache-read-val',  `${fmt(cRead)} tok (${((cRead / inputDenom)*100).toFixed(1)}%)`);

  set('stat-ask-user',  fmt(a.ask_user_count || 0));
  set('stat-plan-mode', fmt(a.plan_mode_count || 0));
  set('stat-denied',    fmt(a.denied_count || 0));

  const summary = document.getElementById('advanced-summary');
  if (summary) {
    summary.textContent = `${fmt(a.turns_with_thinking)} turns w/ thinking · ${fmt(a.turns_with_tools)} turns w/ tools · ${fmt(a.total_turns)} total`;
  }

  const sub = a.subagent_stats || {};
  set('sub-spawn', fmt(sub.spawn_count || 0));
  set('sub-turns', fmt(sub.subagent_turns || 0));
  set('sub-input', fmt(sub.subagent_input_tokens || 0));
  set('sub-output', fmt(sub.subagent_output_tokens || 0));
  set('sub-cost', fmtCost(sub.subagent_cost || 0));

  const deniedBody = document.getElementById('denied-body');
  if (deniedBody) {
    if (!a.denied_breakdown || !a.denied_breakdown.length) {
      setHtml(deniedBody, '<tr><td colspan="3" class="empty">No denied calls in range</td></tr>');
    } else {
      setHtml(deniedBody, a.denied_breakdown.map(d =>
        `<tr>
          <td>${escHtml(d.tool_name)}</td>
          <td><span class="tool-cat tool-cat-${escHtml(d.category)}">${escHtml(d.category)}</span></td>
          <td class="num">${fmt(d.call_count)}</td>
        </tr>`
      ).join(''));
    }
  }

  const skillBody = document.getElementById('skill-body');
  if (!a.skill_breakdown || !a.skill_breakdown.length) {
    setHtml(skillBody, '<tr><td colspan="2" class="empty">No skill invocations in range</td></tr>');
  } else {
    setHtml(skillBody, a.skill_breakdown.map(s =>
      `<tr><td>${escHtml(s.skill_name)}</td><td class="num">${fmt(s.call_count)}</td></tr>`
    ).join(''));
  }

  const toolBody = document.getElementById('tool-body');
  if (!a.tool_breakdown || !a.tool_breakdown.length) {
    setHtml(toolBody, '<tr><td colspan="4" class="empty">No tool calls in range</td></tr>');
  } else {
    setHtml(toolBody, a.tool_breakdown.slice(0, 20).map(t =>
      `<tr>
        <td>${escHtml(t.tool_name)}</td>
        <td><span class="tool-cat tool-cat-${escHtml(t.category)}">${escHtml(t.category)}</span></td>
        <td class="num">${fmt(t.call_count)}</td>
        <td class="num">${fmt(t.input_chars)}</td>
      </tr>`
    ).join(''));
  }

  const mcpBody = document.getElementById('mcp-body');
  if (!a.mcp_breakdown || !a.mcp_breakdown.length) {
    setHtml(mcpBody, '<tr><td colspan="4" class="empty">No MCP calls in range</td></tr>');
  } else {
    setHtml(mcpBody, a.mcp_breakdown.map(m =>
      `<tr>
        <td>${escHtml(m.server)}</td>
        <td class="num">${fmt(m.tool_count)}</td>
        <td class="num">${fmt(m.call_count)}</td>
        <td class="num">${fmt(m.input_chars)}</td>
      </tr>`
    ).join(''));
  }

  const renderPairs = (elId, rows, emptyLabel) => {
    const el = document.getElementById(elId);
    if (!el) return;
    if (!rows || !rows.length) {
      setHtml(el, `<tr><td colspan="2" class="empty">${emptyLabel}</td></tr>`);
      return;
    }
    setHtml(el, rows.map(r =>
      `<tr><td>${escHtml(r.subject)}</td><td class="num">${fmt(r.call_count)}</td></tr>`
    ).join(''));
  };

  const subtypeBody = document.getElementById('subtype-body');
  if (subtypeBody) {
    if (!a.subagent_types || !a.subagent_types.length) {
      setHtml(subtypeBody, '<tr><td colspan="2" class="empty">No subagent spawns in range</td></tr>');
    } else {
      setHtml(subtypeBody, a.subagent_types.map(r =>
        `<tr><td>${escHtml(r.subagent_type)}</td><td class="num">${fmt(r.call_count)}</td></tr>`
      ).join(''));
    }
  }

  renderPairs('bash-body', a.top_bash, 'No bash invocations in range');
  renderPairs('files-body', a.top_files, 'No file operations in range');
  renderPairs('domains-body', a.top_domains, 'No WebFetch calls in range');

  const catBody = document.getElementById('cat-body');
  if (catBody) {
    if (!a.category_totals || !a.category_totals.length) {
      setHtml(catBody, '<tr><td colspan="4" class="empty">No tool calls in range</td></tr>');
    } else {
      setHtml(catBody, a.category_totals.map(c =>
        `<tr>
          <td><span class="tool-cat tool-cat-${escHtml(c.category)}">${escHtml(c.category)}</span></td>
          <td class="num">${fmt(c.call_count)}</td>
          <td class="num">${fmt(c.turn_count)}</td>
          <td class="num">${fmt(c.input_chars)}</td>
        </tr>`
      ).join(''));
    }
  }
}

function renderStats(s) {
  document.getElementById('stat-sessions').textContent = fmt(s.sessions);
  document.getElementById('stat-turns').textContent = fmt(s.turns);
  document.getElementById('stat-input').textContent = fmt(s.input_tokens);
  document.getElementById('stat-output').textContent = fmt(s.output_tokens);
  document.getElementById('stat-cache-read').textContent = fmt(s.cache_read);
  document.getElementById('stat-cache-creation').textContent = fmt(s.cache_creation);
  document.getElementById('stat-cache-hit').textContent = s.cache_hit_pct + '%';
  document.getElementById('stat-cost').textContent = fmtCost(s.est_cost);

  const sc = document.getElementById('subagent-count');
  if (s.subagent_turns > 0) {
    sc.textContent = fmt(s.subagent_turns) + ' subagent turns';
  } else {
    sc.textContent = '';
  }
}

function updateProjectFilter(projects) {
  const sel = document.getElementById('project-filter');
  const existing = [...sel.options].slice(1).map(o => o.value);
  if (JSON.stringify(existing) !== JSON.stringify(projects)) {
    sel.innerHTML = '<option value="">All Projects</option>';
    for (const p of projects) {
      const opt = document.createElement('option');
      opt.value = p;
      opt.textContent = p;
      sel.appendChild(opt);
    }
  }
  if (currentProject && !projects.includes(currentProject)) {
    currentProject = '';
    resetCharts();
    loadDashboard();
  }
  sel.value = currentProject;
}

document.getElementById('project-filter').addEventListener('change', (e) => {
  currentProject = e.target.value;
  resetCharts();
  loadDashboard();
});

function renderDailyChart(daily) {
  const labels = daily.map(d => d.date.slice(5));
  const inputData = daily.map(d => d.input_tokens);
  const outputData = daily.map(d => d.output_tokens);
  const cacheRdData = daily.map(d => d.cache_read);
  const cacheCrData = daily.map(d => d.cache_creation);
  const costData = daily.map(d => d.est_cost);

  if (dailyChart) {
    dailyChart.data.labels = labels;
    dailyChart.data.datasets[0].data = inputData;
    dailyChart.data.datasets[1].data = outputData;
    dailyChart.data.datasets[2].data = cacheRdData;
    dailyChart.data.datasets[3].data = cacheCrData;
    dailyChart.data.datasets[4].data = costData;
    dailyChart.update('none');
    return;
  }

  dailyChart = new Chart(document.getElementById('dailyChart'), {
    data: {
      labels,
      datasets: [
        { type: 'bar', label: 'Input',       data: inputData,  backgroundColor: C.input, stack: 'tokens', yAxisID: 'y' },
        { type: 'bar', label: 'Output',      data: outputData, backgroundColor: C.output, stack: 'tokens', yAxisID: 'y' },
        { type: 'bar', label: 'Cache Read',  data: cacheRdData, backgroundColor: C.cacheRd, stack: 'tokens', yAxisID: 'y' },
        { type: 'bar', label: 'Cache Write', data: cacheCrData, backgroundColor: C.cacheCr, stack: 'tokens', yAxisID: 'y' },
        { type: 'line', label: 'Cost ($)',   data: costData, borderColor: C.cost, backgroundColor: 'rgba(52,211,153,0.1)',
          borderWidth: 2, pointRadius: 2, pointBackgroundColor: C.cost, fill: true, tension: 0.3, yAxisID: 'yCost' },
      ]
    },
    options: {
      responsive: true,
      maintainAspectRatio: false,
      animation: false,
      interaction: { mode: 'index', intersect: false },
      plugins: {
        legend: { display: false },
        tooltip: {
          backgroundColor: '#161a26',
          borderColor: '#2a3050',
          borderWidth: 1,
          titleFont: { size: 11 },
          bodyFont: { size: 10 },
          callbacks: {
            label: c => {
              if (c.dataset.label === 'Cost ($)') return 'Cost: ' + fmtCost(c.raw);
              return c.dataset.label + ': ' + fmt(c.raw);
            }
          }
        }
      },
      scales: {
        x: {
          stacked: true,
          grid: { display: false },
          ticks: { font: { size: 9 }, maxRotation: 0, autoSkip: true, maxTicksLimit: 20 },
        },
        y: {
          stacked: true,
          position: 'left',
          grid: { color: C.grid, lineWidth: 0.5 },
          ticks: { font: { size: 9 }, callback: v => fmt(v) },
        },
        yCost: {
          position: 'right',
          grid: { display: false },
          ticks: { font: { size: 9 }, color: C.cost, callback: v => '$' + v.toFixed(0) },
        }
      }
    }
  });
}

function renderModelChart(models) {
  const labels = models.map(m => m.model);
  const data = models.map(m => m.total_tokens);
  const colors = models.map(m => MODEL_COLORS[m.family] ?? C.other);

  if (modelChart) {
    modelChart.data.labels = labels;
    modelChart.data.datasets[0].data = data;
    modelChart.data.datasets[0].backgroundColor = colors;
    modelChart.update('none');
    return;
  }

  modelChart = new Chart(document.getElementById('modelChart'), {
    type: 'doughnut',
    data: { labels, datasets: [{ data, backgroundColor: colors, borderWidth: 0, hoverOffset: 6 }] },
    options: {
      responsive: true, maintainAspectRatio: false, animation: false, cutout: '60%',
      plugins: {
        legend: { position: 'bottom', labels: { font: { size: 10 }, boxWidth: 10, padding: 10 } },
        tooltip: {
          backgroundColor: '#161a26', borderColor: '#2a3050', borderWidth: 1,
          callbacks: {
            label: c => {
              const total = c.dataset.data.reduce((a,b) => a+b, 0);
              const pct = total > 0 ? ((c.raw / total) * 100).toFixed(1) : 0;
              return c.label + ': ' + fmt(c.raw) + ' (' + pct + '%)';
            }
          }
        }
      }
    }
  });
}

function renderProjectChart(projects) {
  const labels = projects.map(p => p.project);
  const inputData = projects.map(p => p.input_tokens);
  const outputData = projects.map(p => p.output_tokens);

  if (projectChart) {
    projectChart.data.labels = labels;
    projectChart.data.datasets[0].data = inputData;
    projectChart.data.datasets[1].data = outputData;
    projectChart.update('none');
    return;
  }

  projectChart = new Chart(document.getElementById('projectChart'), {
    type: 'bar',
    data: {
      labels,
      datasets: [
        { label: 'Input',  data: inputData,  backgroundColor: C.input },
        { label: 'Output', data: outputData, backgroundColor: C.output },
      ]
    },
    options: {
      responsive: true, maintainAspectRatio: false, animation: false, indexAxis: 'y',
      plugins: {
        legend: { position: 'top', align: 'end', labels: { font: { size: 10 }, boxWidth: 10 } },
        tooltip: { backgroundColor: '#161a26', borderColor: '#2a3050', borderWidth: 1, callbacks: { label: c => c.dataset.label + ': ' + fmt(c.raw) } }
      },
      scales: {
        x: { stacked: true, grid: { color: C.grid, lineWidth: 0.5 }, ticks: { font: { size: 9 }, callback: v => fmt(v) } },
        y: { stacked: true, grid: { display: false }, ticks: { font: { size: 10 } } }
      }
    }
  });
}

function sortSessions() {
  const dir = sortDir === 'asc' ? 1 : -1;
  currentSessions.sort((a, b) => {
    const va = a[sortCol], vb = b[sortCol];
    return dir * (typeof va === 'string' ? va.localeCompare(vb) : va - vb);
  });
}

function renderTable(sessions) {
  const key = JSON.stringify([sessions, expandedSession]);
  if (key === renderedSessionsKey) return;

  const tbody = document.getElementById('sessions-body');
  if (!sessions.length) {
    tbody.innerHTML = '<tr><td colspan="10" style="text-align:center;color:var(--text-muted);padding:24px">No sessions in range</td></tr>';
    renderedSessionsKey = key;
    return;
  }

  let html = '';
  for (const s of sessions) {
    const mk = modelKey(s.model);
    const subBadge = s.subagent_turns > 0
      ? `<span class="subagent-badge">${s.subagent_turns} sub</span>`
      : '';
    const isExpanded = expandedSession === s.session_id;
    const sid = escHtml(s.session_id);

    html += `<tr class="session-row" data-sid="${sid}">
      <td class="sid">${escHtml(s.session_id.slice(0, 8))}</td>
      <td>${escHtml(s.project)}</td>
      <td>${fmtTime(s.last_active)}</td>
      <td>${fmtDur(s.duration_minutes)}</td>
      <td class="model-${mk}">${mk}${subBadge}</td>
      <td class="num">${s.turns}</td>
      <td class="num">${fmt(s.input_tokens)}</td>
      <td class="num">${fmt(s.output_tokens)}</td>
      <td class="num cost-val">${fmtCost(s.est_cost)}</td>
      <td><button class="expand-btn" data-sid="${sid}">${isExpanded ? '▲' : '▼'}</button></td>
    </tr>`;

    if (isExpanded) {
      const detail = turnsCache.sid === s.session_id
        ? turnsCache.html
        : '<em style="color:var(--text-muted)">Loading turns...</em>';
      html += `<tr class="detail-row" id="detail-${sid}">
        <td colspan="10"><div class="detail-content">${detail}</div></td>
      </tr>`;
    }
  }
  tbody.innerHTML = html;
  renderedSessionsKey = key;

  if (expandedSession) {
    loadSessionTurns(expandedSession);
  }
}

document.getElementById('sessions-body').addEventListener('click', (e) => {
  const btn = e.target.closest('.expand-btn');
  if (!btn) return;

  const sid = btn.dataset.sid;
  expandedSession = expandedSession === sid ? null : sid;
  turnsCache = { sid: null, html: '' };
  renderTable(currentSessions);
});

let turnsSeq = 0;

async function loadSessionTurns(sid) {
  const seq = ++turnsSeq;
  let html = null;
  let failure = null;
  try {
    const turns = await invoke('get_session_turns', { sessionId: sid });
    html = turnsTableHtml(turns);
    clearError('Session turns');
  } catch (e) {
    failure = e;
  }
  if (seq !== turnsSeq || expandedSession !== sid) return;

  const detailRow = document.getElementById('detail-' + sid);
  const content = detailRow && detailRow.querySelector('.detail-content');
  if (failure !== null) {
    showError('Session turns', failure);
    if (content) content.innerHTML = `<em style="color:var(--accent-red)">Failed to load turns: ${escHtml(errorMessage(failure))}</em>`;
    return;
  }
  turnsCache = { sid, html };
  if (content) content.innerHTML = html;
}

function turnsTableHtml(turns) {
  if (!turns.length) return '<em style="color:var(--text-muted)">No turns found</em>';

  let html = '<table><thead><tr><th>Time</th><th>Model</th><th class="num">Input</th><th class="num">Output</th><th class="num">Cache Rd</th><th class="num">Cache Wr</th><th class="num">Cost</th><th></th></tr></thead><tbody>';
  for (const t of turns) {
    const mk = modelKey(t.model);
    const sub = t.is_subagent ? '<span class="subagent-badge">sub</span>' : '';
    html += `<tr>
      <td>${fmtTime(t.timestamp)}</td>
      <td class="model-${mk}">${mk}${sub}</td>
      <td class="num">${fmt(t.input_tokens)}</td>
      <td class="num">${fmt(t.output_tokens)}</td>
      <td class="num">${fmt(t.cache_read)}</td>
      <td class="num">${fmt(t.cache_creation)}</td>
      <td class="num cost-val">${fmtCost(t.est_cost)}</td>
      <td></td>
    </tr>`;
  }
  return html + '</tbody></table>';
}

document.querySelectorAll('thead th.sortable').forEach(th => {
  th.addEventListener('click', () => {
    const col = th.dataset.sort;
    if (sortCol === col) {
      sortDir = sortDir === 'asc' ? 'desc' : 'asc';
    } else {
      sortCol = col;
      sortDir = col === 'last_active' || col === 'est_cost' || col === 'turns' || col === 'input_tokens' || col === 'output_tokens' || col === 'duration_minutes' ? 'desc' : 'asc';
    }

    document.querySelectorAll('thead th.sortable').forEach(h => h.classList.remove('sort-asc', 'sort-desc'));
    th.classList.add(sortDir === 'asc' ? 'sort-asc' : 'sort-desc');

    sortSessions();
    renderTable(currentSessions);
  });
});

function resetCharts() {
  if (dailyChart)   { dailyChart.destroy();   dailyChart = null; }
  if (modelChart)   { modelChart.destroy();   modelChart = null; }
  if (projectChart) { projectChart.destroy(); projectChart = null; }
}

document.querySelectorAll('#model-toggles .toggle').forEach(btn => {
  btn.addEventListener('click', () => {
    const active = document.querySelectorAll('#model-toggles .toggle.active');
    // An empty model list makes the backend apply no filter, which would show every model.
    if (btn.classList.contains('active') && active.length === 1) return;
    btn.classList.toggle('active');
    btn.setAttribute('aria-pressed', String(btn.classList.contains('active')));
    activeModels = [...document.querySelectorAll('#model-toggles .toggle.active')].map(b => b.dataset.model);
    resetCharts();
    loadDashboard();
  });
});

document.querySelectorAll('#range-toggles .toggle').forEach(btn => {
  btn.addEventListener('click', () => {
    document.querySelectorAll('#range-toggles .toggle').forEach(b => b.classList.remove('active'));
    btn.classList.add('active');
    currentRange = btn.dataset.range;
    resetCharts();
    loadDashboard();
  });
});

{
  const advRange = document.getElementById('advanced-range');
  if (advRange) {
    advRange.addEventListener('change', () => {
      advancedRange = advRange.value;
      loadAdvanced();
    });
  }
}

function setupDropdown(btnId, menuId) {
  const btn = document.getElementById(btnId);
  const menu = document.getElementById(menuId);
  btn.addEventListener('click', (e) => {
    e.stopPropagation();
    document.querySelectorAll('.dropdown-menu.open').forEach(m => { if (m !== menu) m.classList.remove('open'); });
    menu.classList.toggle('open');
  });
}

setupDropdown('btn-export', 'export-menu');
setupDropdown('btn-tools', 'tools-menu');

document.addEventListener('click', () => {
  document.querySelectorAll('.dropdown-menu.open').forEach(m => m.classList.remove('open'));
});
document.querySelectorAll('.dropdown-menu').forEach(m => {
  m.addEventListener('click', e => e.stopPropagation());
});

async function runExport(id, busyLabel, errorLabel, action) {
  const item = document.getElementById(id);
  if (item.disabled) return;
  const origHtml = item.innerHTML;
  item.disabled = true;
  item.textContent = busyLabel;
  try {
    await action(label => { item.textContent = label; });
    document.getElementById('export-menu').classList.remove('open');
  } catch (e) {
    showError(errorLabel, e);
    item.textContent = 'Error!';
  }
  setTimeout(() => {
    item.innerHTML = origHtml;
    item.disabled = false;
  }, 1500);
}

document.getElementById('act-report').addEventListener('click', () =>
  runExport('act-report', 'Generating...', 'Markdown report', async setLabel => {
    const path = await invoke('generate_report', { range: currentRange });
    setLabel('Opening...');
    await invoke('open_path', { path });
  })
);

document.getElementById('act-pdf').addEventListener('click', () =>
  runExport('act-pdf', 'Generating...', 'PDF report', async setLabel => {
    const path = await invoke('generate_pdf', { range: currentRange });
    setLabel('Opening...');
    await invoke('open_path', { path });
  })
);

document.getElementById('act-csv').addEventListener('click', () =>
  runExport('act-csv', 'Exporting...', 'CSV export', async setLabel => {
    const path = await invoke('export_csv', { range: currentRange, models: activeModels });
    setLabel('Opening folder...');
    await invoke('open_folder', { path });
  })
);

document.getElementById('act-mini').addEventListener('click', async () => {
  document.getElementById('tools-menu').classList.remove('open');
  try {
    await invoke('show_mini_window');
  } catch (e) {
    showError('Mini monitor', e);
  }
});

document.getElementById('act-new-mini').addEventListener('click', async () => {
  document.getElementById('tools-menu').classList.remove('open');
  try {
    await invoke('create_mini_window');
  } catch (e) {
    showError('New mini monitor', e);
  }
});

document.getElementById('act-rescan').addEventListener('click', async () => {
  const item = document.getElementById('act-rescan');
  if (item.disabled) return;
  const origHtml = item.innerHTML;
  item.disabled = true;
  document.getElementById('tools-menu').classList.remove('open');
  forceShowLoader();
  try {
    await invoke('full_rescan');
    item.textContent = 'Scanning...';
  } catch (e) {
    // The periodic background scan emits no progress events, so nothing else would hide the loader.
    if (!isScanning) hideScanOverlay();
    if (String(e).startsWith('A scan')) {
      item.textContent = 'Already running';
    } else {
      item.textContent = 'Error!';
      showError('Full rescan', e);
    }
  }
  setTimeout(() => {
    item.innerHTML = origHtml;
    item.disabled = false;
  }, 2500);
});

document.getElementById('act-open-data').addEventListener('click', async () => {
  document.getElementById('tools-menu').classList.remove('open');
  try {
    const dir = await invoke('get_data_dir');
    await invoke('open_path', { path: dir });
  } catch (e) {
    showError('Open data folder', e);
  }
});

document.getElementById('act-active').addEventListener('click', async () => {
  document.getElementById('tools-menu').classList.remove('open');
  try {
    const sessions = await invoke('get_active_sessions');
    const panel = document.getElementById('active-panel');
    const tbody = document.getElementById('active-body');
    if (!sessions.length) {
      tbody.innerHTML = '<tr><td colspan="4" style="text-align:center;color:var(--text-muted);padding:16px">No active sessions detected</td></tr>';
    } else {
      tbody.innerHTML = sessions.map(s => {
        const mk = modelKey(s.model);
        return `<tr>
          <td class="sid">${escHtml(s.session_id.slice(0, 8))}</td>
          <td>${escHtml(s.project)}</td>
          <td class="model-${mk}">${mk}</td>
          <td>${fmtTime(s.last_activity)}</td>
        </tr>`;
      }).join('');
    }
    panel.classList.remove('hidden');
    clearError('Active sessions');
  } catch (e) {
    showError('Active sessions', e);
  }
});

document.getElementById('active-close').addEventListener('click', () => {
  document.getElementById('active-panel').classList.add('hidden');
});

let editingPricing = null;

function renderPricingEntries(pricing) {
  const grid = document.getElementById('pricing-grid-v2');
  // The first 5 children are the header row.
  while (grid.children.length > 5) grid.removeChild(grid.lastChild);

  (pricing.entries || []).forEach((entry, idx) => {
    const labelEl = document.createElement('div');
    labelEl.className = 'pricing-label-v2';
    const period = entry.effective_from || entry.effective_until
      ? ` · ${entry.effective_from ? `from ${entry.effective_from.slice(0, 10)}` : ''}${entry.effective_from && entry.effective_until ? ' ' : ''}${entry.effective_until ? `until ${entry.effective_until.slice(0, 10)}` : ''}`
      : '';
    labelEl.innerHTML = `<span>${escHtml(entry.label)}</span>
      <span class="pricing-pattern mono" title="Model pattern and effective period">${escHtml(entry.pattern + period)}</span>`;
    grid.appendChild(labelEl);

    for (const field of ['input', 'output', 'cache_read', 'cache_creation']) {
      const input = document.createElement('input');
      input.type = 'number';
      input.className = 'setting-input sm';
      input.step = '0.01';
      input.min = '0';
      input.dataset.entryIdx = idx;
      input.dataset.field = field;
      input.value = entry.pricing[field];
      grid.appendChild(input);
    }
  });

  document.getElementById('pr-fb-input').value = pricing.fallback.input;
  document.getElementById('pr-fb-output').value = pricing.fallback.output;
  document.getElementById('pr-fb-cache-read').value = pricing.fallback.cache_read;
  document.getElementById('pr-fb-cache-creation').value = pricing.fallback.cache_creation;
}

function renderPricingUpdateStatus(settings) {
  document.getElementById('set-auto-pricing').checked = settings.auto_update_pricing !== false;
  const status = document.getElementById('pricing-update-status');
  if (settings.pricing_update_error) {
    status.textContent = `Last check failed; using saved prices. ${settings.pricing_update_error}`;
    status.classList.add('error');
    return;
  }
  status.classList.remove('error');
  if (settings.pricing_last_updated) {
    status.textContent = `Official prices updated ${fmtTime(settings.pricing_last_updated)}.`;
  } else if (settings.pricing_last_checked) {
    status.textContent = `Official prices checked ${fmtTime(settings.pricing_last_checked)}; no changes.`;
  } else {
    status.textContent = 'Official prices are checked in the background every 24 hours.';
  }
}

document.getElementById('act-settings').addEventListener('click', async () => {
  document.getElementById('tools-menu').classList.remove('open');
  try {
    const s = await invoke('get_settings');
    editingPricing = s.pricing;
    document.getElementById('set-threshold').value = s.cost_threshold || '';
    renderPricingEntries(s.pricing);
    renderPricingUpdateStatus(s);
    document.getElementById('settings-modal').classList.remove('hidden');
  } catch (e) {
    showError('Settings', e);
  }
});

document.getElementById('settings-close').addEventListener('click', () => {
  document.getElementById('settings-modal').classList.add('hidden');
});

document.getElementById('settings-modal').addEventListener('click', (e) => {
  if (e.target.id === 'settings-modal') {
    document.getElementById('settings-modal').classList.add('hidden');
  }
});

function readPrice(el, label) {
  const v = parseFloat(el.value);
  if (!(v >= 0)) throw new Error(`Invalid price for ${label}`);
  return v;
}

document.getElementById('settings-save').addEventListener('click', async () => {
  try {
    const costThreshold = parseFloat(document.getElementById('set-threshold').value) || 0;
    const autoUpdatePricing = document.getElementById('set-auto-pricing').checked;
    let pricing = null;

    if (editingPricing && editingPricing.entries) {
      document.querySelectorAll('#pricing-grid-v2 input[data-entry-idx]').forEach(el => {
        const entry = editingPricing.entries[parseInt(el.dataset.entryIdx, 10)];
        const field = el.dataset.field;
        if (entry) entry.pricing[field] = readPrice(el, `${entry.label} (${field})`);
      });
      for (const [id, field] of [
        ['pr-fb-input', 'input'],
        ['pr-fb-output', 'output'],
        ['pr-fb-cache-read', 'cache_read'],
        ['pr-fb-cache-creation', 'cache_creation'],
      ]) {
        editingPricing.fallback[field] = readPrice(document.getElementById(id), `fallback (${field})`);
      }
      pricing = editingPricing;
    }

    await invoke('save_settings', { costThreshold, autoUpdatePricing, pricing });
    document.getElementById('settings-modal').classList.add('hidden');
    clearError('Save settings');
    resetCharts();
    loadDashboard();
  } catch (e) {
    showError('Save settings', e);
  }
});

document.getElementById('pricing-refresh').addEventListener('click', async () => {
  const button = document.getElementById('pricing-refresh');
  const status = document.getElementById('pricing-update-status');
  button.disabled = true;
  button.textContent = 'Checking…';
  status.classList.remove('error');
  status.textContent = 'Checking the official Claude API pricing table…';
  try {
    const result = await invoke('refresh_pricing');
    const fresh = await invoke('get_settings');
    editingPricing = fresh.pricing;
    renderPricingEntries(fresh.pricing);
    renderPricingUpdateStatus(fresh);
    if (result.updated) {
      resetCharts();
      loadDashboard();
    }
  } catch (e) {
    status.textContent = `Price check failed; saved prices are unchanged. ${errorMessage(e)}`;
    status.classList.add('error');
  } finally {
    button.disabled = false;
    button.textContent = 'Check now';
  }
});

document.getElementById('settings-reset').addEventListener('click', async () => {
  document.getElementById('set-threshold').value = '';
  try {
    await invoke('reset_pricing');
    const fresh = await invoke('get_settings');
    editingPricing = fresh.pricing;
    renderPricingEntries(fresh.pricing);
    resetCharts();
    loadDashboard();
  } catch (e) {
    showError('Reset pricing', e);
  }
});

async function openProjectsModal() {
  document.getElementById('tools-menu').classList.remove('open');
  document.getElementById('projects-modal').classList.remove('hidden');
  await renderProjectsList();
}

async function renderProjectsList() {
  const list = document.getElementById('projects-list');
  list.innerHTML = '<div class="projects-empty">Loading projects…</div>';
  try {
    const projects = await invoke('list_projects');
    if (!projects.length) {
      list.innerHTML = '<div class="projects-empty">No projects found yet. Start a Claude Code session in a project, then come back.</div>';
      return;
    }
    list.innerHTML = projects.map(p => {
      const badge = p.has_override ? '<span class="project-row-badge">Renamed</span>' : '';
      const resetBtn = p.has_override
        ? `<button class="action-btn" data-act="reset" data-path="${escHtml(p.project_path)}">Reset</button>`
        : '';
      const meta = (p.session_count === 1 ? '1 session' : `${p.session_count} sessions`)
        + (p.last_active ? ' · last active ' + fmtTime(p.last_active) : '');
      return `
        <div class="project-row${p.has_override ? ' renamed' : ''}" data-path="${escHtml(p.project_path)}">
          <div class="project-row-info">
            <div class="project-row-name">
              <span class="project-row-name-text">${escHtml(p.display_name)}</span>
              ${badge}
            </div>
            <div class="project-row-path" title="${escHtml(p.project_path)}">${escHtml(p.project_path)}</div>
            <div class="project-row-meta">${escHtml(meta)}</div>
          </div>
          <div class="project-row-actions">
            <button class="action-btn" data-act="rename" data-path="${escHtml(p.project_path)}">Rename</button>
            ${resetBtn}
          </div>
        </div>`;
    }).join('');
  } catch (e) {
    showError('List projects', e);
    list.innerHTML = `<div class="projects-empty">Failed to load projects: ${escHtml(errorMessage(e))}</div>`;
  }
}

document.getElementById('projects-list').addEventListener('click', async (e) => {
  const btn = e.target.closest('button[data-act]');
  if (!btn) return;
  const path = btn.dataset.path;
  const act = btn.dataset.act;

  if (act === 'reset') {
    try {
      await invoke('reset_project_name', { projectPath: path });
    } catch (err) {
      showError('Reset project name', err);
      return;
    }
    await renderProjectsList();
    loadDashboard();
    return;
  }

  if (act === 'rename') {
    const row = btn.closest('.project-row');
    const nameWrap = row.querySelector('.project-row-name');
    const currentName = row.querySelector('.project-row-name-text').textContent;

    nameWrap.innerHTML = `<input class="project-rename-input" type="text" value="${escHtml(currentName)}" maxlength="80" />`;
    const input = nameWrap.querySelector('input');
    input.focus();
    input.select();

    const commit = async () => {
      const newName = input.value.trim();
      if (!newName || newName === currentName) {
        await renderProjectsList();
        return;
      }
      try {
        await invoke('rename_project', { projectPath: path, newName });
        loadDashboard();
      } catch (err) {
        showError('Rename project', err);
      }
      await renderProjectsList();
    };

    input.addEventListener('blur', commit, { once: true });
    input.addEventListener('keydown', (ev) => {
      if (ev.key === 'Enter') { ev.preventDefault(); input.blur(); }
      else if (ev.key === 'Escape') { ev.preventDefault(); input.value = currentName; input.blur(); }
    });
  }
});

document.getElementById('act-manage-projects').addEventListener('click', openProjectsModal);
document.getElementById('btn-manage-projects').addEventListener('click', openProjectsModal);

document.getElementById('projects-close').addEventListener('click', () => {
  document.getElementById('projects-modal').classList.add('hidden');
});

document.getElementById('projects-modal').addEventListener('click', (e) => {
  if (e.target.id === 'projects-modal') {
    document.getElementById('projects-modal').classList.add('hidden');
  }
});

document.getElementById('act-about').addEventListener('click', async () => {
  document.getElementById('tools-menu').classList.remove('open');
  try {
    const ver = await window.__TAURI__.app.getVersion();
    document.getElementById('about-version').textContent = 'v' + ver;
    document.getElementById('about-modal').classList.remove('hidden');
  } catch (e) {
    showError('About', e);
  }
});

document.getElementById('about-close').addEventListener('click', () => {
  document.getElementById('about-modal').classList.add('hidden');
});

document.getElementById('about-modal').addEventListener('click', (e) => {
  if (e.target.id === 'about-modal') {
    document.getElementById('about-modal').classList.add('hidden');
  }
});

document.getElementById('cost-alert-close').addEventListener('click', () => {
  document.getElementById('cost-alert').classList.add('hidden');
});

const appWindow = getCurrentWindow();
let posTimer = null;

function saveWindowPos() {
  clearTimeout(posTimer);
  posTimer = setTimeout(async () => {
    try {
      // A minimized Windows window reports (-32000, -32000); persisting that restores it off-screen.
      if (await appWindow.isMinimized()) return;
      const pos = await appWindow.outerPosition();
      const size = await appWindow.outerSize();
      await invoke('save_window_position', {
        x: pos.x, y: pos.y,
        width: size.width, height: size.height,
      });
      clearError('Window position');
    } catch (e) {
      showError('Window position', e);
    }
  }, 500);
}

appWindow.onMoved(saveWindowPos).catch(e => showError('Window events', e));
appWindow.onResized(saveWindowPos).catch(e => showError('Window events', e));

let pollingScan = false;

setInterval(async () => {
  if (isScanning) {
    if (pollingScan) return;
    pollingScan = true;
    try {
      // Safety net for scan-progress events emitted before the listener attached.
      if (await invoke('is_scanning')) {
        const snapshot = await invoke('get_scan_progress');
        if (snapshot) showScanProgress(snapshot);
      } else {
        isScanning = false;
        hideScanOverlay();
        loadDashboard();
      }
      clearError('Scan progress');
    } catch (e) {
      showError('Scan progress', e);
    } finally {
      pollingScan = false;
    }
    return;
  }
  refreshCountdown = Math.max(0, refreshCountdown - 1);
  document.getElementById('refresh-countdown').textContent = refreshCountdown + 's';
}, 1000);

function on(event, handler) {
  listen(event, handler).catch(e => showError('Event ' + event, e));
}

on('cost-alert', e => {
  document.getElementById('cost-alert-text').textContent = `Daily cost threshold exceeded: ${fmtCost(e.payload)}`;
  document.getElementById('cost-alert').classList.remove('hidden');
});

on('scan-progress', e => {
  isScanning = true;
  showScanProgress(e.payload);
});

on('scan-problems', e => {
  const problems = e.payload;
  const shown = problems.slice(0, 3).join('; ');
  const more = problems.length > 3 ? ` (+${problems.length - 3} more)` : '';
  showError('Scan', shown + more);
});

on('scan-complete', () => {
  isScanning = false;
  hideScanOverlay();
  loadDashboard();
});

on('refresh-tick', () => {
  if (!isScanning) {
    loadDashboard();
    refreshCountdown = 30;
  }
});

on('pricing-updated', () => {
  resetCharts();
  loadDashboard();
});

async function loadTier() {
  try {
    const tier = await invoke('get_account_tier');
    if (tier && tier.rate_limit_tier) {
      const label = tier.rate_limit_tier
        .replace('default_', '')
        .replace(/_/g, ' ');
      document.getElementById('tier-badge').textContent = label;
      document.getElementById('tier-group').style.display = '';
    }
  } catch (e) {
    showError('Account tier', e);
  }
}

forceShowLoader();

(async () => {
  try {
    isScanning = await invoke('is_scanning');
  } catch (e) {
    isScanning = false;
    showError('Scan status', e);
  }

  if (isScanning) {
    try {
      const snapshot = await invoke('get_scan_progress');
      if (snapshot) showScanProgress(snapshot);
    } catch (e) {
      showError('Scan progress', e);
    }
  } else {
    hideScanOverlay();
    loadDashboard();
  }

  loadTier();
})();
