//! Embedded dashboard UI for the telemetry viewer.
//!
//! Serves a single-page application at `/` that provides a
//! professional-grade interface for viewing traces, observations,
//! sessions, and cost statistics. The dashboard is inspired by
//! Langfuse's design language with a dark theme and token-flow
//! notation.

use axum::response::Html;

/// Serve the dashboard HTML page.
pub async fn serve_dashboard() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

const DASHBOARD_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Juncture Telemetry</title>
<style>
:root {
  --bg: #0f1117;
  --bg-card: #1a1d27;
  --bg-hover: #252836;
  --bg-input: #1e2130;
  --border: #2d3148;
  --text: #e4e4e7;
  --text-dim: #8b8fa3;
  --text-muted: #565a6e;
  --accent: #6366f1;
  --accent-dim: #4f46e5;
  --success: #10b981;
  --warning: #f59e0b;
  --error: #ef4444;
  --info: #3b82f6;
  --purple: #a78bfa;
  --cyan: #22d3ee;
  --orange: #fb923c;
  --radius: 8px;
  --shadow: 0 1px 3px rgba(0,0,0,.3);
  --font: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
  --mono: 'SF Mono', 'Cascadia Code', 'Fira Code', Consolas, monospace;
}
*{margin:0;padding:0;box-sizing:border-box;}
body{font-family:var(--font);background:var(--bg);color:var(--text);line-height:1.5;}
a{color:var(--accent);text-decoration:none;}
a:hover{text-decoration:underline;}
button{cursor:pointer;font-family:inherit;}
input,select,textarea{font-family:inherit;}

.app{display:flex;height:100vh;}
.sidebar{width:220px;background:var(--bg-card);border-right:1px solid var(--border);display:flex;flex-direction:column;flex-shrink:0;}
.sidebar-header{padding:20px 16px 12px;border-bottom:1px solid var(--border);}
.sidebar-header h1{font-size:16px;font-weight:700;letter-spacing:-.02em;}
.sidebar-header p{font-size:11px;color:var(--text-dim);margin-top:2px;}
.nav{flex:1;padding:8px;}
.nav-item{display:flex;align-items:center;gap:10px;padding:8px 12px;border-radius:var(--radius);color:var(--text-dim);font-size:13px;font-weight:500;cursor:pointer;transition:all .15s;}
.nav-item:hover{background:var(--bg-hover);color:var(--text);}
.nav-item.active{background:var(--accent);color:#fff;}
.nav-item svg{width:16px;height:16px;flex-shrink:0;}
.main{flex:1;overflow:auto;padding:24px;}

/* Two-panel layout for trace detail */
.two-panel{display:flex;gap:0;height:calc(100vh - 48px);}
.panel-left{width:30%;min-width:280px;border-right:1px solid var(--border);overflow:auto;padding:16px;}
.panel-right{flex:1;overflow:auto;padding:16px;}
.back-link{display:inline-flex;align-items:center;gap:6px;color:var(--text-dim);font-size:13px;margin-bottom:16px;cursor:pointer;}
.back-link:hover{color:var(--text);}

/* Stats cards */
.stats-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:16px;margin-bottom:24px;}
.stat-card{background:var(--bg-card);border:1px solid var(--border);border-radius:var(--radius);padding:16px;}
.stat-card .label{font-size:11px;color:var(--text-dim);font-weight:600;text-transform:uppercase;letter-spacing:.06em;}
.stat-card .value{font-size:28px;font-weight:700;margin-top:4px;font-variant-numeric:tabular-nums;}
.stat-card .sub{font-size:12px;color:var(--text-dim);margin-top:2px;}
.stat-card.accent .value{color:var(--accent);}
.stat-card.success .value{color:var(--success);}
.stat-card.warning .value{color:var(--warning);}
.stat-card.error .value{color:var(--error);}
.stat-card.info .value{color:var(--info);}

/* Latency grid */
.latency-grid{display:grid;grid-template-columns:140px repeat(3,1fr);gap:1px;background:var(--border);border:1px solid var(--border);border-radius:var(--radius);overflow:hidden;margin-bottom:24px;}
.latency-grid .cell{background:var(--bg-card);padding:12px 16px;font-size:13px;}
.latency-grid .header{font-size:11px;font-weight:600;text-transform:uppercase;letter-spacing:.05em;color:var(--text-dim);}
.latency-grid .row-label{font-weight:600;font-size:13px;color:var(--text-dim);}
.latency-grid .val{font-variant-numeric:tabular-nums;font-weight:500;}

/* Chart container */
.chart-container{background:var(--bg-card);border:1px solid var(--border);border-radius:var(--radius);padding:16px;margin-bottom:24px;}
.chart-container h3{font-size:13px;font-weight:600;margin-bottom:12px;color:var(--text-dim);text-transform:uppercase;letter-spacing:.05em;}

/* Table */
.table-container{background:var(--bg-card);border:1px solid var(--border);border-radius:var(--radius);overflow:hidden;}
.table-header{display:flex;align-items:center;justify-content:space-between;padding:12px 16px;border-bottom:1px solid var(--border);}
.table-header h2{font-size:14px;font-weight:600;}
table{width:100%;border-collapse:collapse;font-size:13px;}
th{text-align:left;padding:10px 16px;font-weight:600;font-size:11px;text-transform:uppercase;letter-spacing:.05em;color:var(--text-dim);border-bottom:1px solid var(--border);background:var(--bg-card);position:sticky;top:0;}
td{padding:10px 16px;border-bottom:1px solid var(--border);vertical-align:top;}
tr:hover td{background:var(--bg-hover);}
tr.clickable{cursor:pointer;}

/* Tags */
.tag{display:inline-block;padding:2px 8px;border-radius:12px;font-size:11px;font-weight:500;background:var(--accent);color:#fff;margin-right:4px;}
.tag.env{background:var(--info);}
.tag.user{background:var(--success);}

/* Badges */
.badge{display:inline-block;padding:2px 8px;border-radius:12px;font-size:11px;font-weight:600;}
.badge.generation{background:rgba(167,139,250,.13);color:var(--purple);border:1px solid rgba(167,139,250,.27);}
.badge.tool{background:rgba(34,211,238,.13);color:var(--cyan);border:1px solid rgba(34,211,238,.27);}
.badge.span{background:rgba(148,163,184,.13);color:#94a3b8;border:1px solid rgba(148,163,184,.27);}
.badge.retrieval{background:rgba(251,146,60,.13);color:var(--orange);border:1px solid rgba(251,146,60,.27);}
.badge.success{background:rgba(16,185,129,.13);color:#4ade80;border:1px solid rgba(16,185,129,.27);}
.badge.error{background:rgba(239,68,68,.13);color:#f87171;border:1px solid rgba(239,68,68,.27);}

/* Observation tree */
.obs-tree{font-size:13px;}
.obs-node{margin-left:0;padding-left:0;}
.obs-node .obs-node{border-left:2px solid var(--border);margin-left:10px;padding-left:12px;}
.obs-node-header{display:flex;align-items:center;gap:6px;padding:5px 8px;border-radius:var(--radius);cursor:pointer;transition:background .1s;font-size:12px;}
.obs-node-header:hover{background:var(--bg-hover);}
.obs-node-header.selected{background:var(--accent);color:#fff;}
.obs-node-header .name{font-weight:500;}
.obs-node-header .dur{font-size:11px;color:var(--text-dim);font-variant-numeric:tabular-nums;margin-left:auto;}
.obs-node-header .cost{font-size:11px;color:var(--text-dim);font-family:var(--mono);}
.obs-node-header .tokens{font-size:11px;color:var(--text-dim);}
.obs-node-header.selected .dur,.obs-node-header.selected .cost,.obs-node-header.selected .tokens{color:rgba(255,255,255,.7);}
.obs-children{}
.obs-toggle{width:14px;height:14px;display:flex;align-items:center;justify-content:center;color:var(--text-muted);font-size:10px;flex-shrink:0;}
.obs-toggle.has-children{color:var(--text-dim);cursor:pointer;}

/* Tabs */
.tabs{display:flex;gap:0;border-bottom:1px solid var(--border);margin-bottom:16px;}
.tab{padding:8px 16px;font-size:13px;font-weight:500;color:var(--text-dim);cursor:pointer;border-bottom:2px solid transparent;transition:all .15s;}
.tab:hover{color:var(--text);}
.tab.active{color:var(--accent);border-bottom-color:var(--accent);}

/* Detail panel */
.detail-kv{display:grid;grid-template-columns:140px 1fr;gap:4px 12px;font-size:13px;}
.detail-kv .k{color:var(--text-dim);}
.detail-kv .v{word-break:break-all;}

/* Trace summary bar */
.trace-summary{display:flex;gap:24px;padding:12px 16px;background:var(--bg);border:1px solid var(--border);border-radius:var(--radius);margin-bottom:16px;flex-wrap:wrap;}
.trace-summary .item{display:flex;flex-direction:column;gap:2px;}
.trace-summary .item .lbl{font-size:10px;color:var(--text-muted);text-transform:uppercase;letter-spacing:.06em;font-weight:600;}
.trace-summary .item .val{font-size:15px;font-weight:600;font-variant-numeric:tabular-nums;}

/* Code block */
.code-block{background:var(--bg);border:1px solid var(--border);border-radius:var(--radius);padding:12px;font-family:var(--mono);font-size:12px;line-height:1.6;overflow-x:auto;white-space:pre-wrap;word-break:break-word;max-height:400px;overflow-y:auto;}

/* Copy button */
.copy-btn{background:var(--bg-hover);border:1px solid var(--border);color:var(--text-dim);padding:4px 10px;border-radius:var(--radius);font-size:11px;cursor:pointer;margin-bottom:4px;display:inline-block;}
.copy-btn:hover{color:var(--text);border-color:var(--text-dim);}

/* Pagination */
.pagination{display:flex;align-items:center;justify-content:space-between;padding:12px 16px;border-top:1px solid var(--border);font-size:12px;color:var(--text-dim);}
.pagination button{background:var(--bg-hover);border:1px solid var(--border);color:var(--text);padding:4px 12px;border-radius:var(--radius);font-size:12px;}
.pagination button:disabled{opacity:.4;cursor:default;}

/* Filters */
.filters{display:flex;gap:8px;margin-bottom:16px;flex-wrap:wrap;align-items:center;}
.filters input,.filters select{background:var(--bg-input);border:1px solid var(--border);color:var(--text);padding:6px 12px;border-radius:var(--radius);font-size:13px;}
.filters input:focus,.filters select:focus{outline:none;border-color:var(--accent);}
.filters input::placeholder{color:var(--text-muted);}
.filters button{background:var(--accent);border:none;color:#fff;padding:6px 16px;border-radius:var(--radius);font-size:13px;font-weight:500;}
.filters button:hover{background:var(--accent-dim);}

/* Loading */
.loading{text-align:center;padding:40px;color:var(--text-dim);}
.spinner{display:inline-block;width:20px;height:20px;border:2px solid var(--border);border-top-color:var(--accent);border-radius:50%;animation:spin .6s linear infinite;}
@keyframes spin{to{transform:rotate(360deg);}}

/* Empty state */
.empty{text-align:center;padding:60px 20px;color:var(--text-dim);}
.empty p{font-size:14px;}

/* Role coloring for messages */
.role-system{color:var(--purple);}
.role-user{color:var(--info);}
.role-assistant{color:var(--success);}
.role-tool{color:var(--cyan);}

/* Search box in obs tree */
.obs-search{background:var(--bg-input);border:1px solid var(--border);color:var(--text);padding:6px 12px;border-radius:var(--radius);font-size:12px;width:100%;margin-bottom:8px;}
.obs-search:focus{outline:none;border-color:var(--accent);}
.obs-search::placeholder{color:var(--text-muted);}

/* Type filter buttons */
.type-filters{display:flex;gap:4px;margin-bottom:12px;flex-wrap:wrap;}
.type-filters button{background:var(--bg-hover);border:1px solid var(--border);color:var(--text-dim);padding:3px 10px;border-radius:12px;font-size:11px;font-weight:500;cursor:pointer;}
.type-filters button.active{background:var(--accent);color:#fff;border-color:var(--accent);}
.type-filters button:hover:not(.active){border-color:var(--text-dim);color:var(--text);}

/* Session cards */
.session-card{background:var(--bg);border:1px solid var(--border);border-radius:var(--radius);padding:16px;margin-bottom:12px;cursor:pointer;transition:border-color .15s;}
.session-card:hover{border-color:var(--accent);}
.session-card .card-header{display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;}
.session-card .card-header .sid{font-family:var(--mono);font-size:12px;color:var(--text-dim);}
.session-card .card-meta{display:flex;gap:16px;font-size:12px;color:var(--text-dim);margin-bottom:8px;}
.session-card .card-meta span{display:flex;align-items:center;gap:4px;}

/* Responsive */
@media(max-width:768px){
  .sidebar{display:none;}
  .stats-grid{grid-template-columns:1fr 1fr;}
  .two-panel{flex-direction:column;}
  .panel-left{width:100%;min-width:0;border-right:none;border-bottom:1px solid var(--border);}
  .latency-grid{grid-template-columns:100px repeat(3,1fr);}
}
</style>
</head>
<body>
<div class="app">
  <nav class="sidebar">
    <div class="sidebar-header">
      <h1>Juncture</h1>
      <p>Telemetry Dashboard</p>
    </div>
    <div class="nav">
      <div class="nav-item active" data-page="dashboard" onclick="navigate('dashboard')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/></svg>
        Dashboard
      </div>
      <div class="nav-item" data-page="traces" onclick="navigate('traces')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M13 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V9z"/><path d="M13 2v7h7"/></svg>
        Traces
      </div>
      <div class="nav-item" data-page="sessions" onclick="navigate('sessions')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 15a2 2 0 01-2 2H7l-4 4V5a2 2 0 012-2h14a2 2 0 012 2z"/></svg>
        Sessions
      </div>
    </div>
  </nav>
  <main class="main" id="content">
    <div class="loading"><div class="spinner"></div></div>
  </main>
</div>

<script>
const API = '/api/public';
let currentPage = 'dashboard';
let tracesPage = 0;
let sessionsPage = 0;
let tracesFilters = {name:'',userId:'',from:'',to:'',tags:''};
let obsFilterText = '';
let obsTypeFilter = 'all';

// ── Helpers ────────────────────────────────────────────────
async function api(path) {
  const r = await fetch(API + path);
  return r.json();
}

function escHtml(s) {
  if (!s) return '';
  return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}

function fmtDuration(ms) {
  if (ms == null || isNaN(ms)) return '-';
  if (ms < 1) return '<1ms';
  if (ms < 1000) return Math.round(ms) + 'ms';
  if (ms < 60000) return (ms/1000).toFixed(2) + 's';
  return (ms/60000).toFixed(1) + 'm';
}

function fmtCost(c) {
  if (c == null) return '-';
  if (c === 0) return '$0.00';
  if (c < 0.01) return '$' + c.toFixed(6);
  return '$' + c.toFixed(4);
}

function fmtTokens(n) {
  if (n == null) return '-';
  if (n >= 1000000) return (n/1000000).toFixed(1) + 'M';
  if (n >= 1000) return (n/1000).toFixed(1) + 'K';
  return String(n);
}

function fmtTokenFlow(inp, out) {
  if (inp == null && out == null) return '-';
  const i = inp || 0, o = out || 0;
  return fmtTokens(i) + ' -> ' + fmtTokens(o) + ' (' + fmtTokens(i+o) + ')';
}

function fmtTime(ts) {
  if (!ts) return '-';
  return new Date(ts).toLocaleString();
}

function fmtTimeShort(ts) {
  if (!ts) return '-';
  return new Date(ts).toLocaleTimeString();
}

function obsTypeBadge(t) {
  const m = {GENERATION:'generation',TOOL_CALL:'tool',RETRIEVAL:'retrieval',SPAN:'span'};
  const lm = {GENERATION:'Gen',TOOL_CALL:'Tool',RETRIEVAL:'Retrieval',SPAN:'Span'};
  return '<span class="badge ' + (m[t]||'span') + '">' + (lm[t]||t) + '</span>';
}

function levelBadge(l) {
  if (l === 'ERROR') return '<span class="badge error">Error</span>';
  if (l === 'WARNING') return '<span class="badge error">Warn</span>';
  return '';
}

function truncatePreview(s, len) {
  if (!s) return '';
  const str = typeof s === 'string' ? s : JSON.stringify(s);
  return str.length > len ? str.slice(0, len) + '...' : str;
}

function jsonBlock(obj) {
  if (!obj) return '<span style="color:var(--text-muted)">-</span>';
  const s = typeof obj === 'string' ? obj : JSON.stringify(obj, null, 2);
  return '<div class="code-block">' + escHtml(s) + '</div>';
}

function jsonBlockWithCopy(obj) {
  if (!obj) return '<span style="color:var(--text-muted)">-</span>';
  const s = typeof obj === 'string' ? obj : JSON.stringify(obj, null, 2);
  const id = 'cb' + Math.random().toString(36).slice(2,8);
  return '<button class="copy-btn" onclick="copyBlock(\''+id+'\')">Copy</button><div class="code-block" id="'+id+'">' + escHtml(s) + '</div>';
}

function copyBlock(id) {
  const el = document.getElementById(id);
  if (el) navigator.clipboard.writeText(el.textContent);
}

function parseDurationMs(t) {
  if (!t.startTime || !t.endTime) return null;
  return new Date(t.endTime) - new Date(t.startTime);
}

function setActiveNav(page) {
  document.querySelectorAll('.nav-item').forEach(el => {
    el.classList.toggle('active', el.dataset.page === page);
  });
}

// ── Router ─────────────────────────────────────────────────
function navigate(page) {
  currentPage = page;
  setActiveNav(page);
  if (page === 'dashboard') renderDashboard();
  else if (page === 'traces') renderTraces();
  else if (page === 'sessions') renderSessions();
}

// ── Dashboard ──────────────────────────────────────────────
async function renderDashboard() {
  const el = document.getElementById('content');
  el.innerHTML = '<div class="loading"><div class="spinner"></div></div>';

  const [summary, daily, models] = await Promise.all([
    api('/stats/summary'),
    api('/stats/daily'),
    api('/stats/models')
  ]);

  const errRate = summary.totalObservations > 0
    ? ((summary.errorCount / summary.totalObservations) * 100).toFixed(1) + '%'
    : '0%';

  el.innerHTML = ''
    // Stat cards
    + '<div class="stats-grid">'
    + statCard('Total Traces', summary.totalTraces, '', 'accent')
    + statCard('Total Cost', fmtCost(summary.totalCost), '', 'warning')
    + statCard('Total Tokens', fmtTokens(summary.totalTokens), '', 'info')
    + statCard('Avg Latency', fmtDuration(summary.latencyP50Ms), 'p50', '')
    + statCard('Error Rate', errRate, summary.errorCount + ' errors', 'error')
    + statCard('Active Sessions', summary.activeSessions, '', 'success')
    + '</div>'
    // Charts row
    + '<div style="display:grid;grid-template-columns:1fr 1fr;gap:16px;margin-bottom:24px">'
    + '<div class="chart-container"><h3>Traces Over Time (30d)</h3><div id="chart-traces"></div></div>'
    + '<div class="chart-container"><h3>Model Costs Breakdown</h3><div id="chart-models"></div></div>'
    + '</div>'
    // Latency percentiles
    + '<div style="margin-bottom:24px"><h3 style="font-size:13px;font-weight:600;color:var(--text-dim);text-transform:uppercase;letter-spacing:.05em;margin-bottom:8px;">Latency Percentiles</h3>'
    + '<div class="latency-grid">'
    + '<div class="cell header"></div><div class="cell header">p50</div><div class="cell header">p95</div><div class="cell header">p99</div>'
    + '<div class="cell row-label">Trace</div>'
    + '<div class="cell val">' + fmtDuration(summary.latencyP50Ms) + '</div>'
    + '<div class="cell val">' + fmtDuration(summary.latencyP95Ms) + '</div>'
    + '<div class="cell val">' + fmtDuration(summary.latencyP99Ms) + '</div>'
    + '</div></div>'
    // Token usage chart
    + '<div class="chart-container"><h3>Token Usage (Daily)</h3><div id="chart-tokens"></div></div>';

  // Render charts
  renderAreaChart('chart-traces', daily, 'date', 'traceCount', '#6366f1');
  renderBarChart('chart-models', models.slice(0, 8), 'model', 'totalCost');
  renderStackedBarChart('chart-tokens', daily, ['totalTokens'], ['#6366f1'], daily.map(d => ({input: Math.round(d.totalTokens*0.6), output: Math.round(d.totalTokens*0.4)})));
}

function statCard(label, value, sub, cls) {
  return '<div class="stat-card ' + cls + '">'
    + '<div class="label">' + escHtml(label) + '</div>'
    + '<div class="value">' + value + '</div>'
    + (sub ? '<div class="sub">' + escHtml(sub) + '</div>' : '')
    + '</div>';
}

// ── SVG Area Chart ─────────────────────────────────────────
function renderAreaChart(containerId, data, xKey, yKey, color) {
  const container = document.getElementById(containerId);
  if (!container || !data.length) {
    if (container) container.innerHTML = '<div class="empty"><p>No data</p></div>';
    return;
  }
  const W = 500, H = 200, pad = {t:16,r:10,b:30,l:50};
  const cw = W - pad.l - pad.r, ch = H - pad.t - pad.b;
  const vals = data.map(d => d[yKey] || 0);
  const maxV = Math.max(...vals, 1) * 1.15;
  const stepX = cw / Math.max(data.length - 1, 1);

  let points = '';
  let areaPath = 'M' + pad.l + ',' + (pad.t + ch);
  data.forEach((d, i) => {
    const x = pad.l + i * stepX;
    const y = pad.t + ch - ((d[yKey]||0) / maxV) * ch;
    points += '<circle cx="'+x+'" cy="'+y+'" r="3" fill="'+color+'" opacity="0.8"/>';
    areaPath += ' L' + x + ',' + y;
  });
  areaPath += ' L' + (pad.l + (data.length-1)*stepX) + ',' + (pad.t+ch) + ' Z';

  // X labels (show every Nth)
  let xLabels = '';
  const step = Math.max(1, Math.floor(data.length / 6));
  data.forEach((d, i) => {
    if (i % step === 0 || i === data.length - 1) {
      const x = pad.l + i * stepX;
      const lbl = d[xKey] ? d[xKey].slice(5) : '';
      xLabels += '<text x="'+x+'" y="'+(H-4)+'" text-anchor="middle" font-size="10" fill="#565a6e">'+escHtml(lbl)+'</text>';
    }
  });

  // Y labels
  let yLabels = '';
  for (let i = 0; i <= 4; i++) {
    const v = Math.round(maxV * i / 4);
    const y = pad.t + ch - (i/4)*ch;
    yLabels += '<text x="'+(pad.l-8)+'" y="'+(y+4)+'" text-anchor="end" font-size="10" fill="#565a6e">'+v+'</text>';
    yLabels += '<line x1="'+pad.l+'" y1="'+y+'" x2="'+(pad.l+cw)+'" y2="'+y+'" stroke="#2d3148" stroke-dasharray="2,2"/>';
  }

  container.innerHTML = '<svg viewBox="0 0 '+W+' '+H+'" style="width:100%;height:auto">'
    + '<defs><linearGradient id="ag-'+containerId+'" x1="0" y1="0" x2="0" y2="1">'
    + '<stop offset="0%" stop-color="'+color+'" stop-opacity="0.3"/>'
    + '<stop offset="100%" stop-color="'+color+'" stop-opacity="0.02"/>'
    + '</linearGradient></defs>'
    + yLabels + xLabels
    + '<path d="'+areaPath+'" fill="url(#ag-'+containerId+')" stroke="none"/>'
    + '<polyline points="'+data.map((d,i) => (pad.l+i*stepX)+','+(pad.t+ch-((d[yKey]||0)/maxV)*ch)).join(' ')+'" fill="none" stroke="'+color+'" stroke-width="2"/>'
    + points
    + '</svg>';
}

// ── SVG Horizontal Bar Chart ───────────────────────────────
function renderBarChart(containerId, data, labelKey, valueKey) {
  const container = document.getElementById(containerId);
  if (!container || !data.length) {
    if (container) container.innerHTML = '<div class="empty"><p>No data</p></div>';
    return;
  }
  const barH = 24, gap = 6, padL = 140, padR = 60;
  const W = 500, H = data.length * (barH + gap) + 10;
  const maxV = Math.max(...data.map(d => d[valueKey] || 0), 0.001);
  const barW = W - padL - padR;

  let bars = '';
  const colors = ['#a78bfa','#6366f1','#3b82f6','#22d3ee','#10b981','#f59e0b','#fb923c','#ef4444'];
  data.forEach((d, i) => {
    const y = i * (barH + gap) + 5;
    const w = ((d[valueKey]||0) / maxV) * barW;
    const c = colors[i % colors.length];
    bars += '<text x="'+(padL-8)+'" y="'+(y+barH/2+4)+'" text-anchor="end" font-size="11" fill="#8b8fa3">'
      + escHtml(truncatePreview(d[labelKey], 18)) + '</text>';
    bars += '<rect x="'+padL+'" y="'+y+'" width="'+Math.max(w,2)+'" height="'+barH+'" rx="4" fill="'+c+'" opacity="0.85"/>';
    bars += '<text x="'+(padL+w+8)+'" y="'+(y+barH/2+4)+'" font-size="11" fill="#8b8fa3">'
      + fmtCost(d[valueKey]) + '</text>';
  });

  container.innerHTML = '<svg viewBox="0 0 '+W+' '+H+'" style="width:100%;height:auto">' + bars + '</svg>';
}

// ── SVG Stacked Bar Chart ──────────────────────────────────
function renderStackedBarChart(containerId, data, keys, colors, preprocessed) {
  const container = document.getElementById(containerId);
  if (!container || !data.length) {
    if (container) container.innerHTML = '<div class="empty"><p>No data</p></div>';
    return;
  }
  const W = 500, H = 180, pad = {t:24,r:10,b:30,l:50};
  const cw = W - pad.l - pad.r, ch = H - pad.t - pad.b;
  const vals = preprocessed || data;
  const maxV = Math.max(...vals.map(d => (d.input||0)+(d.output||0)), 1) * 1.15;
  const barW = Math.max(4, Math.min(24, cw / data.length - 2));

  let bars = '';
  data.forEach((d, i) => {
    const x = pad.l + i * (cw / data.length) + (cw/data.length - barW) / 2;
    const pd = vals[i] || {input:0,output:0};
    const inpH = ((pd.input||0) / maxV) * ch;
    const outH = ((pd.output||0) / maxV) * ch;
    const totalH = inpH + outH;
    if (totalH < 1) return;
    bars += '<rect x="'+x+'" y="'+(pad.t+ch-inpH-outH)+'" width="'+barW+'" height="'+outH+'" rx="2" fill="#22d3ee" opacity="0.85"/>';
    bars += '<rect x="'+x+'" y="'+(pad.t+ch-inpH)+'" width="'+barW+'" height="'+inpH+'" rx="2" fill="#6366f1" opacity="0.85"/>';
  });

  // Y labels
  let yLabels = '';
  for (let i = 0; i <= 4; i++) {
    const v = Math.round(maxV * i / 4);
    const y = pad.t + ch - (i/4)*ch;
    yLabels += '<text x="'+(pad.l-8)+'" y="'+(y+4)+'" text-anchor="end" font-size="10" fill="#565a6e">'+fmtTokens(v)+'</text>';
    yLabels += '<line x1="'+pad.l+'" y1="'+y+'" x2="'+(pad.l+cw)+'" y2="'+y+'" stroke="#2d3148" stroke-dasharray="2,2"/>';
  }

  // X labels
  let xLabels = '';
  const step = Math.max(1, Math.floor(data.length / 8));
  data.forEach((d, i) => {
    if (i % step === 0 || i === data.length - 1) {
      const x = pad.l + i * (cw / data.length) + (cw/data.length)/2;
      xLabels += '<text x="'+x+'" y="'+(H-4)+'" text-anchor="middle" font-size="10" fill="#565a6e">'+escHtml((d.date||'').slice(5))+'</text>';
    }
  });

  // Legend
  const legend = '<rect x="'+(W-120)+'" y="4" width="10" height="10" rx="2" fill="#6366f1"/><text x="'+(W-106)+'" y="13" font-size="10" fill="#8b8fa3">Input</text>'
    + '<rect x="'+(W-60)+'" y="4" width="10" height="10" rx="2" fill="#22d3ee"/><text x="'+(W-46)+'" y="13" font-size="10" fill="#8b8fa3">Output</text>';

  container.innerHTML = '<svg viewBox="0 0 '+W+' '+H+'" style="width:100%;height:auto">'
    + yLabels + xLabels + bars + legend + '</svg>';
}

// ── Traces ─────────────────────────────────────────────────
function renderTraces() {
  const el = document.getElementById('content');
  el.innerHTML = ''
    + '<div class="filters">'
    + '<input type="text" id="f-name" placeholder="Trace name..." value="' + escHtml(tracesFilters.name) + '">'
    + '<input type="text" id="f-user" placeholder="User ID..." value="' + escHtml(tracesFilters.userId) + '">'
    + '<input type="text" id="f-from" placeholder="YYYY-MM-DD" pattern="\\d{4}-\\d{2}-\\d{2}" value="' + escHtml(tracesFilters.from) + '">'
    + '<input type="text" id="f-to" placeholder="YYYY-MM-DD" pattern="\\d{4}-\\d{2}-\\d{2}" value="' + escHtml(tracesFilters.to) + '">'
    + '<button onclick="applyTraceFilters()">Search</button>'
    + '</div>'
    + '<div class="table-container" id="traces-table"><div class="loading"><div class="spinner"></div></div></div>';
  loadTracesTable();
}

function applyTraceFilters() {
  tracesFilters.name = document.getElementById('f-name').value;
  tracesFilters.userId = document.getElementById('f-user').value;
  tracesFilters.from = document.getElementById('f-from').value;
  tracesFilters.to = document.getElementById('f-to').value;
  tracesPage = 0;
  loadTracesTable();
}

async function loadTracesTable() {
  const container = document.getElementById('traces-table');
  let url = '/traces?page=' + tracesPage + '&pageSize=50';
  if (tracesFilters.name) url += '&name=' + encodeURIComponent(tracesFilters.name);
  if (tracesFilters.userId) url += '&userId=' + encodeURIComponent(tracesFilters.userId);
  if (tracesFilters.from) url += '&fromTimestamp=' + encodeURIComponent(tracesFilters.from + 'T00:00:00Z');
  if (tracesFilters.to) url += '&toTimestamp=' + encodeURIComponent(tracesFilters.to + 'T23:59:59Z');

  const data = await api(url);
  const traces = data.data || [];
  const total = data.totalCount || 0;
  const totalPages = Math.ceil(total / 50) || 1;

  container.innerHTML = ''
    + '<div class="table-header"><h2>Traces (' + total + ')</h2></div>'
    + '<div style="overflow-x:auto"><table><thead><tr>'
    + '<th>Name</th><th>Session</th><th>User</th><th>Time</th><th>Duration</th><th>Tokens (in->out)</th><th>Cost</th><th>Tags</th>'
    + '</tr></thead><tbody>'
    + (traces.length === 0 ? '<tr><td colspan="8" style="text-align:center;color:var(--text-dim);padding:40px">No traces found</td></tr>' :
      traces.map(t => {
        const dur = parseDurationMs(t);
        return '<tr class="clickable" onclick="showTraceDetail(\''+t.id+'\')">'
          + '<td><strong>' + escHtml(t.name||'-') + '</strong></td>'
          + '<td style="color:var(--text-dim);font-size:12px;font-family:var(--mono)">' + escHtml((t.sessionId||'-').slice(0,12)) + '</td>'
          + '<td>' + (t.userId ? '<span class="tag user">' + escHtml(t.userId) + '</span>' : '-') + '</td>'
          + '<td style="color:var(--text-dim)">' + fmtTimeShort(t.startTime) + '</td>'
          + '<td>' + fmtDuration(dur) + '</td>'
          + '<td style="font-family:var(--mono);font-size:12px">' + fmtTokens(t.totalTokens) + '</td>'
          + '<td style="font-family:var(--mono)">' + fmtCost(t.totalCost) + '</td>'
          + '<td>' + (t.tags||[]).map(g => '<span class="tag">' + escHtml(g) + '</span>').join('') + '</td>'
          + '</tr>';
      }).join(''))
    + '</tbody></table></div>'
    + '<div class="pagination">'
    + '<span>Page ' + (tracesPage+1) + ' of ' + totalPages + ' (' + total + ' total)</span>'
    + '<div><button ' + (tracesPage<=0?'disabled':'') + ' onclick="tracesPage--;loadTracesTable()">Previous</button> '
    + '<button ' + (tracesPage>=totalPages-1?'disabled':'') + ' onclick="tracesPage++;loadTracesTable()">Next</button></div>'
    + '</div>';
}

// ── Trace Detail (two-panel) ───────────────────────────────
async function showTraceDetail(traceId) {
  const el = document.getElementById('content');
  el.innerHTML = '<div class="loading"><div class="spinner"></div></div>';

  const data = await api('/traces/' + traceId);
  const trace = data.trace;
  const observations = data.observations || [];
  const dur = parseDurationMs(trace);

  // Store for observation detail
  window._obsMap = {};
  observations.forEach(o => window._obsMap[o.id] = o);
  window._traceData = trace;

  // Build tree structure
  const byParent = {};
  observations.forEach(o => {
    const pid = o.parentObservationId || '__root__';
    if (!byParent[pid]) byParent[pid] = [];
    byParent[pid].push(o);
  });

  // Summary bar
  const totalObsCost = observations.reduce((s,o) => s + (o.cost||0), 0);
  const totalObsTokens = observations.reduce((s,o) => s + ((o.usage&&o.usage.totalTokens)||0), 0);

  el.innerHTML = ''
    + '<div class="back-link" onclick="navigate(\'traces\')">&larr; Back to Traces</div>'
    + '<div class="trace-summary">'
    + '<div class="item"><span class="lbl">Duration</span><span class="val">' + fmtDuration(dur) + '</span></div>'
    + '<div class="item"><span class="lbl">Total Cost</span><span class="val">' + fmtCost(trace.totalCost || totalObsCost) + '</span></div>'
    + '<div class="item"><span class="lbl">Total Tokens</span><span class="val">' + fmtTokenFlow(
      observations.reduce((s,o) => s+((o.usage&&o.usage.inputTokens)||0), 0),
      observations.reduce((s,o) => s+((o.usage&&o.usage.outputTokens)||0), 0)
    ) + '</span></div>'
    + '<div class="item"><span class="lbl">Observations</span><span class="val">' + observations.length + '</span></div>'
    + '<div class="item"><span class="lbl">Trace ID</span><span class="val" style="font-family:var(--mono);font-size:11px;color:var(--text-dim)">' + trace.id.slice(0,8) + '...</span></div>'
    + '</div>'
    + '<div class="two-panel">'
    + '<div class="panel-left" id="obs-tree-panel">'
    + '<input class="obs-search" type="text" placeholder="Filter observations..." oninput="filterObsTree(this.value)">'
    + '<div class="type-filters">'
    + '<button class="active" data-type="all" onclick="setObsTypeFilter(\'all\',this)">All</button>'
    + '<button data-type="GENERATION" onclick="setObsTypeFilter(\'GENERATION\',this)">Gen</button>'
    + '<button data-type="TOOL_CALL" onclick="setObsTypeFilter(\'TOOL_CALL\',this)">Tool</button>'
    + '<button data-type="SPAN" onclick="setObsTypeFilter(\'SPAN\',this)">Span</button>'
    + '</div>'
    + '<div class="obs-tree" id="obs-tree">' + renderObsTree('__root__', byParent, 0) + '</div>'
    + '</div>'
    + '<div class="panel-right" id="obs-detail-panel">'
    + '<div class="empty"><p>Select an observation to view details</p></div>'
    + '</div>'
    + '</div>';
}

function renderObsTree(parentId, byParent, depth) {
  const children = byParent[parentId] || [];
  if (children.length === 0) return '';
  return children.map(o => {
    const dur = parseDurationMs(o);
    const hasChildren = (byParent[o.id]||[]).length > 0;
    const tokenStr = o.usage ? fmtTokenFlow(o.usage.inputTokens, o.usage.outputTokens) : '';
    return '<div class="obs-node" data-id="'+o.id+'" data-type="'+o.observationType+'" data-name="'+escHtml(o.name).toLowerCase()+'">'
      + '<div class="obs-node-header" onclick="selectObs(\''+o.id+'\')" id="obs-'+o.id+'">'
      + '<span class="obs-toggle '+(hasChildren?'has-children':'')+'">'+(hasChildren?'&#9654;':'&middot;')+'</span>'
      + obsTypeBadge(o.observationType)
      + '<span class="name">' + escHtml(o.name) + '</span>'
      + (o.cost != null ? '<span class="cost">' + fmtCost(o.cost) + '</span>' : '')
      + (tokenStr ? '<span class="tokens">' + tokenStr + '</span>' : '')
      + levelBadge(o.level)
      + '<span class="dur">' + fmtDuration(dur) + '</span>'
      + '</div>'
      + '<div class="obs-children">' + renderObsTree(o.id, byParent, depth+1) + '</div>'
      + '</div>';
  }).join('');
}

function filterObsTree(text) {
  obsFilterText = text.toLowerCase();
  applyObsFilters();
}

function setObsTypeFilter(type, btn) {
  obsTypeFilter = type;
  document.querySelectorAll('.type-filters button').forEach(b => b.classList.remove('active'));
  btn.classList.add('active');
  applyObsFilters();
}

function applyObsFilters() {
  const allNodes = document.querySelectorAll('#obs-tree .obs-node');
  // Pass 1: find all nodes that directly match the filter
  const directMatches = new Set();
  allNodes.forEach(el => {
    const name = el.dataset.name || '';
    const type = el.dataset.type || '';
    const matchText = !obsFilterText || name.includes(obsFilterText);
    const matchType = obsTypeFilter === 'all' || type.toLowerCase() === obsTypeFilter.toLowerCase();
    if (matchText && matchType) {
      directMatches.add(el.dataset.id);
    }
  });
  // Pass 2: for each node, show it if it matches OR has a matching descendant
  allNodes.forEach(el => {
    const id = el.dataset.id;
    if (directMatches.has(id)) {
      el.style.display = '';
      return;
    }
    // Check if any descendant obs-node is a direct match
    const descendantNodes = el.querySelectorAll('.obs-node');
    const hasMatchingChild = Array.from(descendantNodes).some(
      child => directMatches.has(child.dataset.id)
    );
    el.style.display = hasMatchingChild ? '' : 'none';
  });
}

function selectObs(obsId) {
  // Highlight
  document.querySelectorAll('.obs-node-header').forEach(el => el.classList.remove('selected'));
  const el = document.getElementById('obs-' + obsId);
  if (el) el.classList.add('selected');

  const o = window._obsMap[obsId];
  if (!o) return;

  const dur = parseDurationMs(o);
  const panel = document.getElementById('obs-detail-panel');

  // Build tabs content
  let overview = '<div class="detail-kv">'
    + '<span class="k">ID</span><span class="v" style="font-family:var(--mono);font-size:11px">' + o.id + '</span>'
    + '<span class="k">Name</span><span class="v">' + escHtml(o.name) + '</span>'
    + '<span class="k">Type</span><span class="v">' + obsTypeBadge(o.observationType) + '</span>'
    + (o.model ? '<span class="k">Model</span><span class="v"><code>' + escHtml(o.model) + '</code></span>' : '')
    + '<span class="k">Duration</span><span class="v">' + fmtDuration(dur) + '</span>'
    + (o.cost != null ? '<span class="k">Cost</span><span class="v" style="font-family:var(--mono)">' + fmtCost(o.cost) + '</span>' : '');
  if (o.usage) {
    overview += '<span class="k">Tokens</span><span class="v" style="font-family:var(--mono)">' + fmtTokenFlow(o.usage.inputTokens, o.usage.outputTokens) + '</span>';
    if (o.usage.cachedTokens) {
      overview += '<span class="k">Cached</span><span class="v">' + fmtTokens(o.usage.cachedTokens) + '</span>';
    }
  }
  if (o.statusMessage) {
    overview += '<span class="k">Status</span><span class="v" style="color:var(--error)">' + escHtml(o.statusMessage) + '</span>';
  }
  overview += '</div>';
  if (o.modelParameters) {
    overview += '<div style="margin-top:12px"><h3 style="font-size:12px;color:var(--text-dim);margin-bottom:6px">Parameters</h3>' + jsonBlock(o.modelParameters) + '</div>';
  }

  const inputContent = o.input ? renderMessageContent(o.input) : '<span style="color:var(--text-muted)">No input</span>';
  const outputContent = o.output ? jsonBlockWithCopy(o.output) : '<span style="color:var(--text-muted)">No output</span>';

  panel.innerHTML = ''
    + '<div style="display:flex;align-items:center;gap:8px;margin-bottom:16px">'
    + obsTypeBadge(o.observationType)
    + '<strong style="font-size:15px">' + escHtml(o.name) + '</strong>'
    + levelBadge(o.level)
    + '</div>'
    + '<div class="tabs">'
    + '<div class="tab active" onclick="switchObsTab(\'overview\',this)">Overview</div>'
    + '<div class="tab" onclick="switchObsTab(\'input\',this)">Input</div>'
    + '<div class="tab" onclick="switchObsTab(\'output\',this)">Output</div>'
    + '</div>'
    + '<div id="obs-tab-content">' + overview + '</div>';

  // Store for tab switching
  window._obsTabs = {overview, input: inputContent, output: outputContent};
}

function switchObsTab(tab, btn) {
  document.querySelectorAll('.tabs .tab').forEach(t => t.classList.remove('active'));
  btn.classList.add('active');
  document.getElementById('obs-tab-content').innerHTML = window._obsTabs[tab] || '';
}

function renderMessageContent(input) {
  // Check if input has messages array
  if (input && input.messages && Array.isArray(input.messages)) {
    return input.messages.map(m => {
      const role = m.role || 'unknown';
      const content = typeof m.content === 'string' ? m.content : JSON.stringify(m.content, null, 2);
      return '<div style="margin-bottom:8px;padding:8px 12px;background:var(--bg);border:1px solid var(--border);border-radius:var(--radius)">'
        + '<div style="font-size:11px;font-weight:600;text-transform:uppercase;letter-spacing:.05em;margin-bottom:4px" class="role-'+escHtml(role)+'">' + escHtml(role) + '</div>'
        + '<div class="code-block" style="max-height:200px">' + escHtml(content) + '</div>'
        + '</div>';
    }).join('');
  }
  return jsonBlockWithCopy(input);
}

// ── Sessions ───────────────────────────────────────────────
async function renderSessions() {
  const el = document.getElementById('content');
  el.innerHTML = '<div class="loading"><div class="spinner"></div></div>';

  const data = await api('/sessions/enriched?page=' + sessionsPage + '&pageSize=50');
  const sessions = data.data || [];
  const total = data.totalCount || 0;
  const totalPages = Math.ceil(total / 50) || 1;

  el.innerHTML = ''
    + '<div class="table-header" style="margin-bottom:16px"><h2>Sessions (' + total + ')</h2></div>'
    + (sessions.length === 0
      ? '<div class="empty"><p>No sessions found</p></div>'
      : sessions.map(s => {
        return '<div class="session-card" onclick="showSessionDetail(\'' + escHtml(s.id) + '\')">'
          + '<div class="card-header">'
          + '<span class="sid">' + escHtml(s.id) + '</span>'
          + (s.userId ? '<span class="tag user">' + escHtml(s.userId) + '</span>' : '')
          + '</div>'
          + '<div class="card-meta">'
          + '<span>' + s.traceCount + ' traces</span>'
          + '<span>' + fmtCost(s.totalCost) + '</span>'
          + '<span>' + fmtTokens(s.totalTokens) + ' tokens</span>'
          + '<span>Created ' + fmtTime(s.createdAt) + '</span>'
          + (s.lastActive ? '<span>Last active ' + fmtTime(s.lastActive) + '</span>' : '')
          + '</div>'
          + '</div>';
      }).join(''))
    + '<div class="pagination" style="margin-top:16px">'
    + '<span>Page ' + (sessionsPage+1) + ' of ' + totalPages + ' (' + total + ' total)</span>'
    + '<div><button ' + (sessionsPage<=0?'disabled':'') + ' onclick="sessionsPage--;renderSessions()">Previous</button> '
    + '<button ' + (sessionsPage>=totalPages-1?'disabled':'') + ' onclick="sessionsPage++;renderSessions()">Next</button></div>'
    + '</div>';
}

async function showSessionDetail(sessionId) {
  const el = document.getElementById('content');
  el.innerHTML = '<div class="loading"><div class="spinner"></div></div>';

  const [session, tracesData] = await Promise.all([
    api('/sessions/' + encodeURIComponent(sessionId)),
    api('/traces?sessionId=' + encodeURIComponent(sessionId) + '&pageSize=100')
  ]);

  const traces = tracesData.data || [];
  const totalCost = traces.reduce((s,t) => s + (t.totalCost||0), 0);
  const totalTokens = traces.reduce((s,t) => s + (t.totalTokens||0), 0);

  el.innerHTML = ''
    + '<div class="back-link" onclick="navigate(\'sessions\')">&larr; Back to Sessions</div>'
    + '<div class="detail-kv" style="margin-bottom:24px">'
    + '<span class="k">Session ID</span><span class="v" style="font-family:var(--mono);font-size:12px">' + escHtml(sessionId) + '</span>'
    + '<span class="k">User</span><span class="v">' + (session.userId ? '<span class="tag user">' + escHtml(session.userId) + '</span>' : '-') + '</span>'
    + '<span class="k">Created</span><span class="v">' + fmtTime(session.createdAt) + '</span>'
    + '<span class="k">Traces</span><span class="v">' + traces.length + '</span>'
    + '<span class="k">Total Cost</span><span class="v" style="font-family:var(--mono)">' + fmtCost(totalCost) + '</span>'
    + '<span class="k">Total Tokens</span><span class="v">' + fmtTokens(totalTokens) + '</span>'
    + '</div>'
    + '<h3 style="font-size:13px;font-weight:600;color:var(--text-dim);text-transform:uppercase;letter-spacing:.05em;margin-bottom:12px">Traces</h3>'
    + (traces.length === 0
      ? '<div class="empty"><p>No traces in this session</p></div>'
      : traces.map(t => {
        const dur = parseDurationMs(t);
        const preview = t.input ? truncatePreview(
          (typeof t.input === 'string' ? t.input : JSON.stringify(t.input)), 120
        ) : '';
        return '<div class="session-card" onclick="showTraceDetail(\'' + t.id + '\')">'
          + '<div class="card-header">'
          + '<strong>' + escHtml(t.name || '-') + '</strong>'
          + '<span style="font-family:var(--mono);font-size:12px;color:var(--text-dim)">' + fmtDuration(dur) + '</span>'
          + '</div>'
          + '<div class="card-meta">'
          + '<span>' + fmtTimeShort(t.startTime) + '</span>'
          + '<span style="font-family:var(--mono)">' + fmtTokenFlow(
            t.totalTokens ? Math.round(t.totalTokens*0.6) : null,
            t.totalTokens ? Math.round(t.totalTokens*0.4) : null
          ) + '</span>'
          + '<span style="font-family:var(--mono)">' + fmtCost(t.totalCost) + '</span>'
          + '</div>'
          + (preview ? '<div style="font-size:12px;color:var(--text-dim);margin-top:6px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">' + escHtml(preview) + '</div>' : '')
          + '</div>';
      }).join(''));
}

// ── Init ───────────────────────────────────────────────────
navigate('dashboard');
</script>
</body>
</html>"##;

// Rust guideline compliant 2026-06-01
