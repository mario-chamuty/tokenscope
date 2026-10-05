const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

let layout = 'compact';
let selectedProject = '';
let errorLabel = null;

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

function showError(label, err) {
  console.error(label, err);
  errorLabel = label;
  const message = label + ': ' + String(err?.message ?? err);
  const box = document.getElementById('mini-error');
  document.getElementById('mini-error-text').textContent = message;
  box.title = message;
  box.classList.remove('hidden');
}

function clearError(label) {
  if (errorLabel !== label) return;
  errorLabel = null;
  document.getElementById('mini-error').classList.add('hidden');
}

document.getElementById('mini-error-close').addEventListener('click', () => {
  errorLabel = null;
  document.getElementById('mini-error').classList.add('hidden');
});

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

function on(event, handler) {
  listen(event, handler).catch(e => showError('Event ' + event, e));
}

function updateValues(s) {
  const hasProject = !!selectedProject && !!s.active_session;

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

  document.getElementById('e-cost').textContent = fmtCost(s.today_cost);
  document.getElementById('e-tokens').textContent = fmt(s.today_tokens);
  document.getElementById('e-sessions').textContent = s.today_sessions;
  document.getElementById('e-total').textContent = fmtCost(s.total_cost);

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

const loadStats = coalesced(async () => {
  const project = selectedProject;
  try {
    const s = await invoke('get_mini_stats', { project: project || null });
    if (project !== selectedProject) return;
    updateValues(s);
    clearError('Mini stats');
  } catch (e) {
    showError('Mini stats', e);
  }
});

function openProjectPicker(e) {
  e.preventDefault();
  e.stopPropagation();
  invoke('show_project_picker').catch(err => showError('Project picker', err));
}

document.getElementById('c-brand').addEventListener('mousedown', openProjectPicker);
document.getElementById('e-brand').addEventListener('mousedown', openProjectPicker);

on('project-selected', (event) => {
  selectedProject = event.payload || '';
  loadStats();
});

async function setLayout(newLayout) {
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
}

async function applyLayout(newLayout) {
  try {
    await setLayout(newLayout);
    await invoke('set_mini_layout', { layout });
    clearError('Mini layout');
  } catch (e) {
    showError('Mini layout', e);
  }
}

document.addEventListener('dblclick', (e) => {
  if (e.target.tagName === 'BUTTON' || e.target.closest('#c-brand') || e.target.closest('#e-brand')) return;
  applyLayout(layout === 'compact' ? 'expanded' : 'compact');
});

// Only the static "mini" window is reusable from the tray; dynamic "mini-<timestamp>" windows
// have no tray entry, so hiding them would leak an invisible window.
function closeOrHide(e) {
  e.preventDefault();
  e.stopPropagation();
  const w = getCurrentWindow();
  (w.label === 'mini' ? w.hide() : w.close()).catch(err => showError('Close window', err));
}
document.getElementById('c-close').addEventListener('mousedown', closeOrHide);
document.getElementById('e-close').addEventListener('mousedown', closeOrHide);

on('refresh-tick', () => loadStats());
on('scan-complete', () => loadStats());

(async () => {
  try {
    const settings = await invoke('get_settings');
    await setLayout(settings.mini_layout);
  } catch (e) {
    showError('Mini layout', e);
  }
  loadStats();
})();
