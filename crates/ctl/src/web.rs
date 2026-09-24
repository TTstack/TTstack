//! Embedded web frontend for TTstack management.
//!
//! Serves a single-page application directly from the controller binary.
//! No external files or build tools required.

use axum::response::Html;

/// GET / — serve the management dashboard.
///
/// The page itself requires no authentication. When API key auth is
/// enabled, the JS client detects 401 responses and prompts the user
/// to enter the key (stored in sessionStorage).
pub async fn index() -> Html<&'static str> {
    Html(FRONTEND_HTML)
}

const FRONTEND_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>TTstack — Dashboard</title>
<style>
  :root {
    --bg: #0f1117; --surface: #1a1d27; --border: #2a2d3a;
    --text: #e4e6ed; --muted: #8b8fa3; --accent: #6c8cff;
    --green: #4ade80; --red: #f87171; --yellow: #fbbf24;
  }
  * { margin: 0; padding: 0; box-sizing: border-box; }
  body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
    background: var(--bg); color: var(--text); line-height: 1.5; }
  .container { max-width: 1200px; margin: 0 auto; padding: 1rem; }

  header { display: flex; align-items: center; justify-content: space-between;
    padding: 1rem 0; border-bottom: 1px solid var(--border); margin-bottom: 1.5rem; }
  header h1 { font-size: 1.4rem; font-weight: 600; }
  header h1 span { color: var(--accent); }

  nav { display: flex; gap: 0.5rem; }
  nav button { background: var(--surface); color: var(--text); border: 1px solid var(--border);
    padding: 0.4rem 1rem; border-radius: 6px; cursor: pointer; font-size: 0.85rem; }
  nav button.active { background: var(--accent); color: #fff; border-color: var(--accent); }
  nav button:hover { border-color: var(--accent); }

  .stats { display: grid; grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
    gap: 1rem; margin-bottom: 1.5rem; }
  .stat { background: var(--surface); border: 1px solid var(--border);
    border-radius: 8px; padding: 1rem; }
  .stat .label { font-size: 0.75rem; color: var(--muted); text-transform: uppercase;
    letter-spacing: 0.05em; }
  .stat .value { font-size: 1.5rem; font-weight: 700; margin-top: 0.25rem; }

  .panel { background: var(--surface); border: 1px solid var(--border);
    border-radius: 8px; margin-bottom: 1.5rem; }
  .panel-header { display: flex; justify-content: space-between; align-items: center;
    padding: 0.75rem 1rem; border-bottom: 1px solid var(--border); }
  .panel-header h2 { font-size: 1rem; font-weight: 600; }

  table { width: 100%; border-collapse: collapse; font-size: 0.85rem; }
  th { text-align: left; padding: 0.6rem 1rem; color: var(--muted);
    font-weight: 500; font-size: 0.75rem; text-transform: uppercase;
    letter-spacing: 0.05em; border-bottom: 1px solid var(--border); }
  td { padding: 0.6rem 1rem; border-bottom: 1px solid var(--border); }
  tr:last-child td { border-bottom: none; }
  tr:hover td { background: rgba(108, 140, 255, 0.05); }

  .badge { display: inline-block; padding: 0.15rem 0.5rem; border-radius: 4px;
    font-size: 0.75rem; font-weight: 500; }
  .badge-online, .badge-active, .badge-running { background: rgba(74,222,128,0.15); color: var(--green); }
  .badge-offline, .badge-stopped { background: rgba(248,113,113,0.15); color: var(--red); }
  .badge-failed { background: rgba(248,113,113,0.25); color: var(--red); }
  .badge-creating, .badge-paused { background: rgba(251,191,36,0.15); color: var(--yellow); }

  .btn { background: var(--accent); color: #fff; border: none; padding: 0.35rem 0.8rem;
    border-radius: 5px; cursor: pointer; font-size: 0.8rem; }
  .btn:hover { opacity: 0.85; }
  .btn:disabled { opacity: 0.4; cursor: not-allowed; }
  .btn-danger { background: var(--red); }
  .btn-sm { padding: 0.2rem 0.5rem; font-size: 0.75rem; }

  .modal-overlay { display: none; position: fixed; top: 0; left: 0; width: 100%; height: 100%;
    background: rgba(0,0,0,0.6); z-index: 100; justify-content: center; align-items: center; }
  .modal-overlay.show { display: flex; }
  .modal { background: var(--surface); border: 1px solid var(--border);
    border-radius: 10px; padding: 1.5rem; width: 90%; max-width: 520px; }
  .modal h3 { margin-bottom: 1rem; }
  .modal label { display: block; font-size: 0.85rem; color: var(--muted); margin-bottom: 0.25rem; }
  .modal input, .modal select { width: 100%; background: var(--bg); color: var(--text);
    border: 1px solid var(--border); padding: 0.5rem; border-radius: 5px;
    margin-bottom: 0.75rem; font-size: 0.85rem; }
  .modal .actions { display: flex; gap: 0.5rem; justify-content: flex-end; margin-top: 0.5rem; }
  .modal .row { display: flex; gap: 0.75rem; }
  .modal .row > div { flex: 1; }
  .modal .checkbox-row { display: flex; align-items: center; gap: 0.5rem; margin-bottom: 0.75rem; }
  .modal .checkbox-row input[type="checkbox"] { width: auto; margin: 0; }

  .empty { text-align: center; padding: 2rem; color: var(--muted); }
  .loading { text-align: center; padding: 1rem; color: var(--muted); font-size: 0.85rem; }
  .toast { position: fixed; bottom: 1rem; right: 1rem; background: var(--surface);
    border: 1px solid var(--border); padding: 0.75rem 1rem; border-radius: 8px;
    z-index: 200; display: none; font-size: 0.85rem; max-width: 400px; }
  .toast.show { display: block; }
  .toast.error { border-color: var(--red); }
  .expiry { font-size: 0.75rem; color: var(--muted); }
</style>
</head>
<body>
<div class="container">
  <header>
    <h1><span>TT</span>stack Dashboard</h1>
    <nav>
      <button class="active" data-tab="status" onclick="switchTab('status')">Status</button>
      <button data-tab="hosts" onclick="switchTab('hosts')">Hosts</button>
      <button data-tab="envs" onclick="switchTab('envs')">Environments</button>
      <button data-tab="images" onclick="switchTab('images')">Images</button>
    </nav>
  </header>

  <!-- Status Tab -->
  <div id="tab-status" class="tab-content">
    <div class="stats" id="fleet-stats"><div class="loading">Loading...</div></div>
  </div>

  <!-- Hosts Tab -->
  <div id="tab-hosts" class="tab-content" style="display:none">
    <div class="panel">
      <div class="panel-header">
        <h2>Hosts</h2>
        <button class="btn" onclick="showModal('add-host')">+ Add Host</button>
      </div>
      <table><thead><tr>
        <th>ID</th><th>Address</th><th>State</th><th>Engines</th><th>CPU</th><th>Memory</th><th>VMs</th><th></th>
      </tr></thead><tbody id="hosts-body"><tr><td colspan="8" class="loading">Loading...</td></tr></tbody></table>
    </div>
  </div>

  <!-- Environments Tab -->
  <div id="tab-envs" class="tab-content" style="display:none">
    <div class="panel">
      <div class="panel-header">
        <h2>Environments</h2>
        <button class="btn" onclick="showModal('add-env')">+ Create Env</button>
      </div>
      <table><thead><tr>
        <th>Name</th><th>Owner</th><th>State</th><th>VMs</th><th>Expires</th><th></th>
      </tr></thead><tbody id="envs-body"><tr><td colspan="6" class="loading">Loading...</td></tr></tbody></table>
    </div>
    <div class="panel" id="env-detail-panel" style="display:none">
      <div class="panel-header"><h2 id="env-detail-title">VM Details</h2></div>
      <table><thead><tr>
        <th>ID</th><th>Image</th><th>Engine</th><th>State</th><th>IP</th><th>Ports</th>
      </tr></thead><tbody id="env-vms-body"></tbody></table>
    </div>
  </div>

  <!-- Images Tab -->
  <div id="tab-images" class="tab-content" style="display:none">
    <div class="panel">
      <div class="panel-header"><h2>Available Images</h2></div>
      <table><thead><tr>
        <th>Name</th><th>Host</th>
      </tr></thead><tbody id="images-body"><tr><td colspan="2" class="loading">Loading...</td></tr></tbody></table>
    </div>
  </div>
</div>

<!-- Add Host Modal -->
<div class="modal-overlay" id="modal-add-host">
  <div class="modal">
    <h3>Add Host</h3>
    <label>Agent Address</label>
    <input id="host-addr" placeholder="10.0.0.2:9100">
    <div class="actions">
      <button class="btn" style="background:var(--border)" onclick="hideModals()">Cancel</button>
      <button class="btn" id="btn-add-host" onclick="addHost()">Add</button>
    </div>
  </div>
</div>

<!-- Create Environment Modal -->
<div class="modal-overlay" id="modal-add-env">
  <div class="modal">
    <h3>Create Environment</h3>
    <label>Name</label>
    <input id="env-name" placeholder="my-test-env">
    <div class="row">
      <div><label>Owner</label><input id="env-owner" placeholder="web" value="web"></div>
      <div><label>Image</label><input id="env-image" placeholder="ubuntu-22.04"></div>
    </div>
    <div class="row">
      <div><label>Engine</label>
        <select id="env-engine" onchange="updateEngineOptions()">
          <option value="qemu">QEMU</option>
          <option value="firecracker">Firecracker</option>
          <option value="docker">Docker</option>
          <option value="bhyve">Bhyve (experimental)</option>
          <option value="jail">Jail (experimental)</option>
        </select>
      </div>
      <div><label>Replicas</label><input id="env-dup" type="number" value="1" min="1"></div>
    </div>
    <div class="row">
      <div><label>CPU Cores</label><input id="env-cpu" type="number" value="2" min="1"></div>
      <div><label>Memory (MB)</label><input id="env-mem" type="number" value="1024" min="64"></div>
      <div><label>QEMU disk (MiB)</label><input id="env-disk" type="number" value="40960" min="128"></div>
    </div>
    <label>Ports (comma-separated)</label>
    <input id="env-ports" placeholder="22, 80, 443" value="22">
    <div class="row">
      <div><label>Lifetime (seconds, 0 = no expiry)</label>
        <input id="env-lifetime" type="number" value="21600" min="0">
      </div>
    </div>
    <div class="checkbox-row">
      <input type="checkbox" id="env-deny-outgoing">
      <label for="env-deny-outgoing" style="margin-bottom:0">Deny outgoing network</label>
    </div>
    <div><label>SSH Public Keys (one per line)</label>
      <textarea id="env-ssh-keys" rows="3" placeholder="ssh-ed25519 AAAA... user@host"></textarea>
    </div>
    <div class="actions">
      <button class="btn" style="background:var(--border)" onclick="hideModals()">Cancel</button>
      <button class="btn" id="btn-create-env" onclick="createEnv()">Create</button>
    </div>
  </div>
</div>

<div class="toast" id="toast"></div>

<script>
const API = '';
let refreshTimer = null;
let currentTab = 'status';
let selectedEnv = null;

function getApiKey() { return sessionStorage.getItem('tt_api_key'); }
function setApiKey(k) { sessionStorage.setItem('tt_api_key', k); }

// HTML-escape to prevent XSS
function esc(s) {
  if (s == null) return '';
  const d = document.createElement('div');
  d.appendChild(document.createTextNode(String(s)));
  return d.innerHTML;
}

async function api(method, path, body) {
  const opts = { method, headers: { 'Content-Type': 'application/json' } };
  const key = getApiKey();
  if (key) opts.headers['Authorization'] = 'Bearer ' + key;
  if (body) opts.body = JSON.stringify(body);
  const res = await fetch(API + path, opts);
  if (res.status === 401) {
    const k = prompt('API key required:');
    if (k) { setApiKey(k); return api(method, path, body); }
    throw new Error('Authentication required');
  }
  const data = await res.json();
  if (!data.ok) throw new Error(data.error || 'Request failed');
  return data.data;
}

function switchTab(name) {
  currentTab = name;
  document.querySelectorAll('.tab-content').forEach(el => el.style.display = 'none');
  document.querySelectorAll('nav button').forEach(el => el.classList.remove('active'));
  document.getElementById('tab-' + name).style.display = 'block';
  document.querySelector('nav button[data-tab="' + name + '"]').classList.add('active');
  refresh(name);
}

function badge(state) {
  return '<span class="badge badge-' + esc(state) + '">' + esc(state) + '</span>';
}

function toast(msg, isError) {
  const el = document.getElementById('toast');
  el.textContent = msg;
  el.className = 'toast show' + (isError ? ' error' : '');
  setTimeout(function() { el.className = 'toast'; }, 5000);
}

function showModal(id) { document.getElementById('modal-' + id).classList.add('show'); }
function hideModals() { document.querySelectorAll('.modal-overlay').forEach(function(el) { el.classList.remove('show'); }); }

function setBtn(id, disabled) {
  const btn = document.getElementById(id);
  if (btn) btn.disabled = disabled;
}

function formatExpiry(expiresAt) {
  if (!expiresAt || expiresAt === 0) return '<span class="expiry">never</span>';
  var now = Math.floor(Date.now() / 1000);
  var diff = expiresAt - now;
  if (diff <= 0) return '<span class="expiry" style="color:var(--red)">expired</span>';
  var h = Math.floor(diff / 3600);
  var m = Math.floor((diff % 3600) / 60);
  if (h > 0) return '<span class="expiry">' + h + 'h ' + m + 'm</span>';
  return '<span class="expiry">' + m + 'm</span>';
}

async function refresh(tab) {
  try {
    if (tab === 'status') await loadStatus();
    else if (tab === 'hosts') await loadHosts();
    else if (tab === 'envs') await loadEnvs();
    else if (tab === 'images') await loadImages();
  } catch (e) { toast(e.message, true); }
}

async function loadStatus() {
  var s = await api('GET', '/api/status');
  document.getElementById('fleet-stats').innerHTML =
    '<div class="stat"><div class="label">Hosts</div><div class="value">' + esc(s.hosts_online) + '/' + esc(s.hosts) + '</div></div>' +
    '<div class="stat"><div class="label">VMs</div><div class="value">' + esc(s.total_vms) + '</div></div>' +
    '<div class="stat"><div class="label">Envs</div><div class="value">' + esc(s.total_envs) + '</div></div>' +
    '<div class="stat"><div class="label">CPU</div><div class="value">' + esc(s.cpu_used) + '/' + esc(s.cpu_total) + '</div></div>' +
    '<div class="stat"><div class="label">Memory</div><div class="value">' + esc(s.mem_used) + '/' + esc(s.mem_total) + ' MB</div></div>' +
    '<div class="stat"><div class="label">Disk</div><div class="value">' + esc(s.disk_used) + '/' + esc(s.disk_total) + ' MB</div></div>';
}

async function loadHosts() {
  var hosts = await api('GET', '/api/hosts');
  var tbody = document.getElementById('hosts-body');
  if (!hosts.length) { tbody.innerHTML = '<tr><td colspan="8" class="empty">No hosts registered</td></tr>'; return; }
  tbody.innerHTML = hosts.map(function(h) {
    return '<tr>' +
      '<td>' + esc(h.id) + '</td>' +
      '<td>' + esc(h.addr) + '</td>' +
      '<td>' + badge(h.state) + '</td>' +
      '<td>' + esc((h.engines || []).join(', ')) + '</td>' +
      '<td>' + esc(h.resource.cpu_used) + '/' + esc(h.resource.cpu_total) + '</td>' +
      '<td>' + esc(h.resource.mem_used) + '/' + esc(h.resource.mem_total) + ' MB</td>' +
      '<td>' + esc(h.resource.vm_count) + '</td>' +
      '<td><button class="btn btn-sm btn-danger" onclick="removeHost(\'' + esc(h.id) + '\')">Remove</button></td>' +
      '</tr>';
  }).join('');
}

async function loadEnvs() {
  var envs = await api('GET', '/api/envs');
  var tbody = document.getElementById('envs-body');
  if (!envs.length) { selectedEnv = null; document.getElementById('env-detail-panel').style.display = 'none'; tbody.innerHTML = '<tr><td colspan="6" class="empty">No environments</td></tr>'; return; }
  tbody.innerHTML = envs.map(function(e) {
    return '<tr>' +
      '<td><a href="#" onclick="showEnv(\'' + esc(e.id) + '\'); return false;" style="color:var(--accent)">' + esc(e.id) + '</a></td>' +
      '<td>' + esc(e.owner) + '</td>' +
      '<td>' + badge(e.state) + (e.error ? '<div>' + esc(e.error) + '</div>' : '') + '</td>' +
      '<td>' + esc(e.vm_ids.length) + '</td>' +
      '<td>' + formatExpiry(e.expires_at) + '</td>' +
      '<td>' +
        (['active', 'stopped', 'failed'].includes(e.state) ?
          '<button class="btn btn-sm" onclick="toggleEnv(\'' + esc(e.id) + '\',\'stopped\')">Start</button> ' +
          '<button class="btn btn-sm" onclick="toggleEnv(\'' + esc(e.id) + '\',\'active\')">Stop</button> ' : '') +
        '<button class="btn btn-sm btn-danger" onclick="deleteEnv(\'' + esc(e.id) + '\')">Delete</button>' +
      '</td>' +
      '</tr>';
  }).join('');
  if (selectedEnv && envs.some(e => e.id === selectedEnv)) await showEnv(selectedEnv);
  else { selectedEnv = null; document.getElementById('env-detail-panel').style.display = 'none'; }
}

async function showEnv(id) {
  selectedEnv = id;
  try {
    var detail = await api('GET', '/api/envs/' + encodeURIComponent(id));
    var hosts = await api('GET', '/api/hosts');
    var addresses = Object.fromEntries(hosts.map(h => [h.id, new URL('http://' + h.addr).hostname]));
    document.getElementById('env-detail-title').textContent = 'VMs in ' + id;
    document.getElementById('env-detail-panel').style.display = 'block';
    var tbody = document.getElementById('env-vms-body');
    tbody.innerHTML = detail.vms.map(function(vm) {
      var ports = Object.entries(vm.port_map).map(function(e) { return (addresses[vm.host_id] || vm.host_id) + ':' + e[1] + '\u2192' + e[0]; }).join(', ');
      return '<tr>' +
        '<td>' + esc(vm.id) + '</td>' +
        '<td>' + esc(vm.image) + '</td>' +
        '<td>' + esc(vm.engine) + '</td>' +
        '<td>' + badge(vm.state) + (vm.error ? '<div>' + esc(vm.error) + '</div>' : '') + '</td>' +
        '<td>' + esc(vm.ip) + '</td>' +
        '<td>' + esc(ports || '-') + '</td>' +
        '</tr>';
    }).join('');
  } catch (e) { toast(e.message, true); }
}

async function loadImages() {
  var images = await api('GET', '/api/images');
  var tbody = document.getElementById('images-body');
  if (!images.length) { tbody.innerHTML = '<tr><td colspan="2" class="empty">No images available</td></tr>'; return; }
  tbody.innerHTML = images.map(function(i) {
    return '<tr><td>' + esc(i.name) + '</td><td>' + esc(i.host_id) + '</td></tr>';
  }).join('');
}

async function addHost() {
  var addr = document.getElementById('host-addr').value.trim();
  if (!addr) return;
  setBtn('btn-add-host', true);
  try {
    await api('POST', '/api/hosts', { addr: addr });
    hideModals(); toast('Host added'); document.getElementById('host-addr').value = ''; loadHosts();
  } catch (e) { toast(e.message, true); }
  finally { setBtn('btn-add-host', false); }
}

async function removeHost(id) {
  if (!confirm('Remove host ' + id + '?')) return;
  try { await api('DELETE', '/api/hosts/' + encodeURIComponent(id)); toast('Host removed'); loadHosts(); }
  catch (e) { toast(e.message, true); }
}

function updateEngineOptions() {
  const engine = document.getElementById('env-engine').value;
  document.getElementById('env-disk').disabled = engine !== 'qemu';
  document.getElementById('env-deny-outgoing').disabled = engine === 'docker';
  document.getElementById('env-ssh-keys').disabled = !['qemu', 'jail'].includes(engine);
}

async function createEnv() {
  var name = document.getElementById('env-name').value.trim();
  var owner = document.getElementById('env-owner').value.trim() || 'web';
  var image = document.getElementById('env-image').value.trim();
  var engine = document.getElementById('env-engine').value;
  var cpu = parseInt(document.getElementById('env-cpu').value) || 2;
  var mem = parseInt(document.getElementById('env-mem').value) || 1024;
  var disk = parseInt(document.getElementById('env-disk').value) || 40960;
  var dup = parseInt(document.getElementById('env-dup').value) || 1;
  var portsStr = document.getElementById('env-ports').value.trim();
  var lifetime = parseInt(document.getElementById('env-lifetime').value) || 0;
  var denyOutgoing = document.getElementById('env-deny-outgoing').checked;
  var sshKeysRaw = document.getElementById('env-ssh-keys').value.trim();
  var sshKeys = sshKeysRaw ? sshKeysRaw.split('\n').map(function(l) { return l.trim(); }).filter(function(l) { return l.length > 0; }) : [];

  if (!name || !image) { toast('Name and image are required', true); return; }

  var ports = portsStr ? portsStr.split(',').map(function(p) { return parseInt(p.trim()); }).filter(function(p) { return p > 0 && p <= 65535; }) : [];

  var vms = [];
  for (var i = 0; i < dup; i++) {
    vms.push({ image: image, engine: engine, cpu: cpu, mem: mem, disk: engine === 'qemu' ? disk : null, ports: ports, deny_outgoing: engine === 'docker' ? false : denyOutgoing, ssh_keys: [] });
  }

  var body = { id: name, owner: owner, vms: vms, lifetime: lifetime, ssh_keys: ['qemu', 'jail'].includes(engine) ? sshKeys : [] };

  setBtn('btn-create-env', true);
  try {
    var result = await api('POST', '/api/envs', body);
    hideModals(); toast('Creation started; status will update automatically');
    if (result && result.warnings && result.warnings.length) {
      toast('Warnings: ' + result.warnings.join('; '), true);
    }
    loadEnvs();
  } catch (e) { toast(e.message, true); }
  finally { setBtn('btn-create-env', false); }
}

async function deleteEnv(id) {
  if (!confirm('Delete environment ' + id + '? This will destroy all VMs.')) return;
  try { await api('DELETE', '/api/envs/' + encodeURIComponent(id)); toast('Environment deleted'); loadEnvs(); }
  catch (e) { toast(e.message, true); }
}

async function toggleEnv(id, state) {
  var action = state === 'active' ? 'stop' : 'start';
  try { await api('POST', '/api/envs/' + encodeURIComponent(id) + '/' + action, {}); toast(action === 'start' ? 'Environment started' : 'Environment stopped'); loadEnvs(); }
  catch (e) { toast(e.message, true); }
}

// Auto-refresh every 30 seconds
function startAutoRefresh() {
  if (refreshTimer) clearInterval(refreshTimer);
  refreshTimer = setInterval(function() { refresh(currentTab); }, 5000);
}

// Initial load
loadStatus();
startAutoRefresh();
</script>
</body>
</html>
"##;
