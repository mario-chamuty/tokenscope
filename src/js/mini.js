const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

let layout = 'compact';
let selectedProject = ''; // empty = all/global

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
  if (n >= 1000) return '$' + (n / 1000).toFixed(1) + 'K';
  return '$' + n.toFixed(2);
}

function updateValues(s) {
  const hasProject = !!selectedProject && !!s.active_session;

  // Compact - show project session stats when filtered, else global
  if (hasProject) {
    const ap = s.active_session;
    document.getElementById('c-cost').textContent = fmtCost(ap.session_cost);
    document.getElementById('c-tokens').textContent = fmt(ap.session_tokens);
    document.getElementById('c-sessions').textContent = ap.session_turns + ' turns';
    document.getElementById('c-total').textContent = fmtCost(s.today_cost);
  } else {
    document.getElementById('c-cost').textContent = fmtCost(s.today_cost);
    document.getElementById('c-tokens').textContent = fmt(s.today_tokens);
    document.getElementById('c-sessions').textContent = s.today_sessions;
    document.getElementById('c-total').textContent = fmtCost(s.total_cost);
  }

  // Expanded - always show today globals in grid
  document.getElementById('e-cost').textContent = fmtCost(s.today_cost);
  document.getElementById('e-tokens').textContent = fmt(s.today_tokens);
  document.getElementById('e-sessions').textContent = s.today_sessions;
  document.getElementById('e-total').textContent = fmtCost(s.total_cost);

  // Brand name & dot
  const cName = document.getElementById('c-name');
  const eName = document.getElementById('e-name');
  const cDot = document.getElementById('c-dot');
  const eDot = document.getElementById('e-dot');
  const eActive = document.getElementById('e-active');

  if (hasProject) {
    const ap = s.active_session;
    cName.textContent = ap.project;
    cName.classList.add('has-project');
    cName.title = ap.project;
    eName.textContent = ap.project;
    eName.classList.add('has-project');
    eName.title = ap.project;
    cDot.style.background = '#4facfe';
    cDot.style.boxShadow = '0 0 5px #4facfe';
    eDot.style.background = '#4facfe';
    eDot.style.boxShadow = '0 0 5px #4facfe';
    eActive.classList.remove('hidden');
    document.getElementById('e-sess-cost').textContent = fmtCost(ap.session_cost);
    document.getElementById('e-sess-tokens').textContent = fmt(ap.session_tokens);
    document.getElementById('e-sess-turns').textContent = ap.session_turns;
  } else {
    cName.textContent = selectedProject || 'TokenScope';
    cName.classList.toggle('has-project', !!selectedProject);
    cName.title = selectedProject || '';
    eName.textContent = selectedProject || 'TokenScope';
    eName.classList.toggle('has-project', !!selectedProject);
    eName.title = selectedProject || '';
    cDot.style.background = selectedProject ? '#4facfe' : '#34d399';
    cDot.style.boxShadow = selectedProject ? '0 0 5px #4facfe' : '0 0 5px #34d399';
    eDot.style.background = selectedProject ? '#4facfe' : '#34d399';
    eDot.style.boxShadow = selectedProject ? '0 0 5px #4facfe' : '0 0 5px #34d399';
    eActive.classList.add('hidden');
  }
}

async function loadStats() {
  try {
    const s = await invoke('get_mini_stats', { project: selectedProject || null });
    updateValues(s);
  } catch (e) {
    console.error('Mini stats error:', e);
  }
}

// ─── Project picker (native popup menu) ───
function openProjectPicker(e) {
  e.preventDefault();
  e.stopPropagation();
  invoke('show_project_picker').catch(err => console.error('Picker error:', err));
}

document.getElementById('c-brand').addEventListener('mousedown', openProjectPicker);
document.getElementById('e-brand').addEventListener('mousedown', openProjectPicker);

// Listen for selection from native menu
listen('project-selected', (event) => {
  selectedProject = event.payload || '';
  loadStats();
});

// ─── Layout ───
async function applyLayout(newLayout) {
  layout = newLayout;
  const win = getCurrentWindow();
  const compact = document.getElementById('compact');
  const expanded = document.getElementById('expanded');

  if (layout === 'compact') {
    compact.classList.remove('hidden');
    expanded.classList.add('hidden');
    await win.setSize(new window.__TAURI__.dpi.LogicalSize(520, 52));
  } else {
    compact.classList.add('hidden');
    expanded.classList.remove('hidden');
    await win.setSize(new window.__TAURI__.dpi.LogicalSize(400, 130));
  }

  try {
    const settings = await invoke('get_settings');
    settings.mini_layout = layout;
    await invoke('save_settings', { newSettings: settings });
  } catch (e) {
    console.error('Save settings error:', e);
  }
}

// Toggle layout on double-click (but not on brand or close)
document.addEventListener('dblclick', (e) => {
  if (e.target.tagName === 'BUTTON' || e.target.closest('#c-brand') || e.target.closest('#e-brand')) return;
  applyLayout(layout === 'compact' ? 'expanded' : 'compact');
});

// Close buttons
// The static mini window (label === "mini") is reusable from the tray, so we
// hide it. Dynamic spawns (label "mini-<timestamp>") have no tray entry, so
// hiding would just leak an invisible, unreachable window — close/destroy
// instead.
function closeOrHide(e) {
  e.preventDefault();
  e.stopPropagation();
  const w = getCurrentWindow();
  if (w.label === 'mini') {
    w.hide();
  } else {
    w.close();
  }
}
document.getElementById('c-close').addEventListener('mousedown', closeOrHide);
document.getElementById('e-close').addEventListener('mousedown', closeOrHide);

// Events
listen('refresh-tick', () => loadStats());
listen('scan-complete', () => loadStats());

// Self-poll every 10s
setInterval(loadStats, 10000);

// Init
(async () => {
  try {
    const settings = await invoke('get_settings');
    await applyLayout(settings.mini_layout || 'compact');
  } catch {
    await applyLayout('compact');
  }
  loadStats();
  setTimeout(loadStats, 3000);
  setTimeout(loadStats, 8000);
})();
