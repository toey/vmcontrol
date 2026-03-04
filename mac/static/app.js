// ──────────────────────────────────────────
// API Key Authentication Helper
// ──────────────────────────────────────────
function getApiKey() {
    return localStorage.getItem('vmcontrol_api_key') || '';
}
function setApiKey(key) {
    if (key) localStorage.setItem('vmcontrol_api_key', key);
    else localStorage.removeItem('vmcontrol_api_key');
}
function apiHeaders(extra) {
    var h = extra || {};
    var key = getApiKey();
    if (key) h['X-API-Key'] = key;
    return h;
}
// Wrapper for fetch that handles 401 (auth required)
async function apiFetch(url, opts) {
    opts = opts || {};
    opts.headers = apiHeaders(opts.headers || {});
    var res = await fetch(url, opts);
    if (res.status === 401) {
        var key = prompt('API Key required. Enter your VMCONTROL_API_KEY:');
        if (key) {
            setApiKey(key);
            opts.headers['X-API-Key'] = key;
            res = await fetch(url, opts);
        }
    }
    return res;
}

// ──────────────────────────────────────────
// API Key Management UI
// ──────────────────────────────────────────
async function loadApikey() {
    try {
        var response = await apiFetch('/api/apikey');
        var data = await safeJson(response);
        if (data && data.api_key) {
            document.getElementById('apikey-display').value = data.api_key;
            // Sync to localStorage
            setApiKey(data.api_key);
        } else {
            document.getElementById('apikey-display').value = '';
            document.getElementById('apikey-display').placeholder = '(not set)';
        }
    } catch (e) {
        console.error('Failed to load API key:', e);
    }
}

function toggleApikeyVisibility() {
    var inp = document.getElementById('apikey-display');
    var btn = document.getElementById('apikey-toggle-btn');
    if (inp.type === 'password') {
        inp.type = 'text';
        btn.textContent = 'Hide';
    } else {
        inp.type = 'password';
        btn.textContent = 'Show';
    }
}

function copyApikey() {
    var inp = document.getElementById('apikey-display');
    var key = inp.value;
    if (!key) return;
    navigator.clipboard.writeText(key).then(function() {
        var statusEl = document.getElementById('status-indicator');
        statusEl.className = 'success';
        statusEl.textContent = 'API key copied to clipboard';
    }).catch(function() {
        // Fallback
        inp.type = 'text';
        inp.select();
        document.execCommand('copy');
        inp.type = 'password';
    });
}

async function generateApikey() {
    if (!confirm('Generate a new API key? The current key will be replaced.\n\nYou will need to update all clients with the new key.')) return;
    var statusEl = document.getElementById('status-indicator');
    statusEl.className = 'loading';
    statusEl.textContent = 'Generating new API key...';
    try {
        var response = await apiFetch('/api/apikey/generate', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: '{}',
        });
        var data = await safeJson(response);
        if (data && data.success) {
            // Update localStorage with new key
            setApiKey(data.api_key);
            // Update display
            document.getElementById('apikey-display').value = data.api_key;
            statusEl.className = 'success';
            statusEl.textContent = 'API key generated successfully';
        } else {
            statusEl.className = 'error';
            statusEl.textContent = 'Error: ' + (data ? data.message : 'Unknown error');
        }
    } catch (err) {
        statusEl.className = 'error';
        statusEl.textContent = 'Error: ' + err.message;
    }
}

// HTML entity escaping to prevent XSS when inserting server data into innerHTML
function escapeHtml(str) {
    if (str == null) return '';
    return String(str).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;').replace(/'/g,'&#39;');
}

// OS Templates — loaded from DB, keyed by template key
// { 'ubuntu-server': { vcpus: '2', memory: '2048', is_windows: '0', arch: 'x86_64', image: 'ubuntu-server', name: 'Ubuntu Server', id: 1 }, ... }
var OS_TEMPLATES = { 'custom': null };
window._osTemplateList = []; // raw array from API

// Default templates — seeded to DB if empty
var DEFAULT_OS_TEMPLATES = [
    { key: 'ubuntu-server',   name: 'Ubuntu Server',   vcpus: '2', memory: '2048', is_windows: '0', arch: 'x86_64',  image: 'ubuntu-server' },
    { key: 'ubuntu-desktop',  name: 'Ubuntu Desktop',  vcpus: '4', memory: '4096', is_windows: '0', arch: 'x86_64',  image: 'ubuntu-desktop' },
    { key: 'debian',          name: 'Debian',           vcpus: '2', memory: '1024', is_windows: '0', arch: 'x86_64',  image: 'debian' },
    { key: 'centos-rocky',    name: 'CentOS / Rocky',   vcpus: '2', memory: '2048', is_windows: '0', arch: 'x86_64',  image: 'centos' },
    { key: 'windows-desktop', name: 'Windows 10/11',    vcpus: '4', memory: '4096', is_windows: '1', arch: 'x86_64',  image: 'windows-10' },
    { key: 'windows-server',  name: 'Windows Server',   vcpus: '8', memory: '8192', is_windows: '1', arch: 'x86_64',  image: 'windows-server' },
    { key: 'macos',           name: 'macOS',             vcpus: '8', memory: '8192', is_windows: '0', arch: 'x86_64',  image: 'macos' },
    { key: 'minimal-linux',   name: 'Minimal Linux',    vcpus: '1', memory: '512',  is_windows: '0', arch: 'x86_64',  image: 'minimal' },
];

async function loadOsTemplates() {
    try {
        var response = await apiFetch('/api/os-templates');
        var list = await safeJson(response);
        if (!list || list.length === 0) {
            // Seed defaults
            for (var i = 0; i < DEFAULT_OS_TEMPLATES.length; i++) {
                await apiFetch('/api/os-templates/create', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify(DEFAULT_OS_TEMPLATES[i]),
                });
            }
            // Reload after seeding
            response = await apiFetch('/api/os-templates');
            list = await safeJson(response) || [];
        }
        window._osTemplateList = list;
        // Build lookup map
        OS_TEMPLATES = { 'custom': null };
        list.forEach(function(t) {
            OS_TEMPLATES[t.key] = { vcpus: t.vcpus, memory: t.memory, is_windows: t.is_windows, arch: t.arch || 'x86_64', image: t.image, name: t.name, id: t.id };
        });
        // Populate the Create VM template dropdown
        populateOsTemplateDropdown();
        // Populate template image dropdown
        populateTplImageSelect();
        // Render management list
        renderOsTemplateList();
    } catch (e) {
        console.error('Failed to load OS templates:', e);
    }
}

function populateOsTemplateDropdown() {
    var sel = document.getElementById('create-os-template');
    if (!sel) return;
    var current = sel.value;
    sel.innerHTML = '<option value="custom">Custom</option>';
    window._osTemplateList.forEach(function(t) {
        var opt = document.createElement('option');
        opt.value = t.key;
        opt.textContent = t.name;
        sel.appendChild(opt);
    });
    if (current) sel.value = current;
}

// Template-to-image mappings (cached from server)
window._templateImageMap = {};

async function loadImageMappings() {
    try {
        var response = await apiFetch('/api/template-images');
        var map = await safeJson(response);
        window._templateImageMap = map || {};
    } catch (e) {
        window._templateImageMap = {};
    }
    return window._templateImageMap;
}

function getImageMappings() {
    return window._templateImageMap || {};
}

function saveImageMapping(templateKey, diskName) {
    // Update local cache immediately
    if (diskName) { window._templateImageMap[templateKey] = diskName; }
    else { delete window._templateImageMap[templateKey]; }
    // Persist to server (fire and forget)
    apiFetch('/api/template-images/set', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ template_key: templateKey, disk_name: diskName || '' })
    }).catch(function(e) { console.error('Failed to save template image mapping:', e); });
}

function applyOsTemplate() {
    var sel = document.getElementById('create-os-template');
    var tpl = OS_TEMPLATES[sel.value];
    var templateKey = sel.value;

    if (!tpl) return;

    // Fill CPU / Memory / Features
    document.getElementById('start-vcpus').value = tpl.vcpus;
    document.getElementById('start-memory-size').value = tpl.memory;
    document.getElementById('start-is-windows').value = tpl.is_windows;
    document.getElementById('start-arch').value = tpl.arch || 'x86_64';

    // Resolve base image: saved mapping → auto-match → none
    var savedMap = getImageMappings();
    var imageName = savedMap[templateKey] || null;

    // Verify saved image still exists
    if (imageName) {
        var disks = window._diskList || [];
        var exists = disks.some(function(d) { return d.name === imageName; });
        if (!exists) imageName = null;
    }

    // Auto-match if no saved mapping
    if (!imageName && tpl.image) {
        imageName = findMatchingDisk(tpl.image);
    }

    // Auto-clone base image and set as disk 0
    if (imageName) {
        autoCloneDiskForTemplate(imageName);
    } else {
        applyBaseImageToDisk(imageName);
    }
}

// Auto-clone a base image and set cloned disk as disk 0
async function autoCloneDiskForTemplate(sourceImage) {
    var vmName = val('create-vm-name').trim();
    // Generate random suffix (6 chars)
    var rand = Math.random().toString(36).substring(2, 8);
    var cloneName;
    if (vmName) {
        cloneName = vmName + '-' + rand;
    } else {
        cloneName = sourceImage.replace(/^template-/, '') + '-' + rand;
    }

    var statusEl = document.getElementById('status-indicator');
    statusEl.className = 'loading';
    statusEl.textContent = 'Cloning ' + sourceImage + ' → ' + cloneName + '...';

    try {
        var response = await apiFetch('/api/disk/clone', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ source: sourceImage, name: cloneName }),
        });
        var data = await safeJson(response);
        if (data.success) {
            statusEl.className = 'success';
            statusEl.textContent = 'Cloned: ' + cloneName + '.qcow2';
            // Reload disk list then set cloned disk as disk 0
            await loadDiskList();
            applyBaseImageToDisk(cloneName);
        } else {
            statusEl.className = 'error';
            statusEl.textContent = 'Clone error: ' + data.message;
            // Fallback: just select the source image
            applyBaseImageToDisk(sourceImage);
        }
    } catch (err) {
        statusEl.className = 'error';
        statusEl.textContent = 'Clone error: ' + err.message;
        applyBaseImageToDisk(sourceImage);
    }
}

// Set first disk row's select to the chosen base image
function applyBaseImageToDisk(diskName) {
    if (!diskName) return;
    var diskSelect = document.querySelector('#start-disks .disk-diskname');
    if (diskSelect) {
        // Make sure the option exists in the select
        var found = false;
        for (var i = 0; i < diskSelect.options.length; i++) {
            if (diskSelect.options[i].value === diskName) { found = true; break; }
        }
        if (found) {
            diskSelect.value = diskName;
        } else {
            // Might need to add it (disk could be owned by another VM)
            populateDiskSelect(diskSelect, diskName);
        }
    }
}

// Find best matching disk from cached list
function findMatchingDisk(pattern) {
    var disks = window._diskList || [];
    var editingVm = window._editingVm || '';
    var pat = pattern.toLowerCase();
    // Exact match
    for (var i = 0; i < disks.length; i++) {
        var d = disks[i];
        if (d.name.toLowerCase() === pat) return d.name;
    }
    // Prefix match
    for (var i = 0; i < disks.length; i++) {
        var d = disks[i];
        if (d.name.toLowerCase().indexOf(pat) === 0) return d.name;
    }
    // Contains match
    for (var i = 0; i < disks.length; i++) {
        var d = disks[i];
        if (d.name.toLowerCase().indexOf(pat) !== -1) return d.name;
    }
    return null;
}

// ── OS Template Management ──

// Populate template image dropdown from disk list
function populateTplImageSelect(selectedValue) {
    var sel = document.getElementById('tpl-image');
    if (!sel) return;
    var disks = window._diskList || [];
    var current = selectedValue !== undefined ? selectedValue : sel.value;
    sel.innerHTML = '<option value="">-- no image --</option>';
    disks.forEach(function(d) {
        if (d.name.indexOf('template-') !== 0) return; // only show template images
        var opt = document.createElement('option');
        opt.value = d.name;
        var sizeInfo = d.disk_size || formatSize(d.size);
        opt.textContent = d.name + '.qcow2 (' + sizeInfo + ')';
        sel.appendChild(opt);
    });
    if (current) sel.value = current;
}

// Upload qcow2 image for templates (reuses /api/image/upload)
window.uploadTemplateImage = function() {
    var fileInput = document.getElementById('tpl-image-file');
    if (!fileInput.files.length) { alert('Select a file'); return; }
    var file = fileInput.files[0];
    var progressDiv = document.getElementById('tpl-upload-progress');
    var progressBar = document.getElementById('tpl-progress-bar');
    var progressText = document.getElementById('tpl-progress-text');
    progressDiv.style.display = '';
    progressBar.value = 0;
    progressText.textContent = '0%';

    var xhr = new XMLHttpRequest();
    xhr.open('POST', '/api/image/upload', true);
    var key = localStorage.getItem('vmcontrol_api_key') || '';
    if (key) xhr.setRequestHeader('X-API-Key', key);
    var safeName = file.name.replace(/[^a-zA-Z0-9._-]/g, '_');
    if (safeName.indexOf('template-') !== 0) safeName = 'template-' + safeName;
    xhr.setRequestHeader('X-Filename', safeName);
    xhr.upload.onprogress = function(e) {
        if (e.lengthComputable) {
            var pct = Math.round(e.loaded / e.total * 100);
            progressBar.value = pct;
            progressText.textContent = pct + '%';
        }
    };
    xhr.onload = function() {
        progressDiv.style.display = 'none';
        fileInput.value = '';
        if (xhr.status === 200) {
            // Reload disk list then refresh the image dropdown
            loadDiskList().then(function() { populateTplImageSelect(); });
        } else {
            alert('Upload failed: ' + xhr.responseText);
        }
    };
    xhr.onerror = function() {
        progressDiv.style.display = 'none';
        alert('Upload error');
    };
    xhr.send(file);
};

function renderOsTemplateList() {
    var listDiv = document.getElementById('os-template-list');
    if (!listDiv) return;
    var list = window._osTemplateList || [];
    if (list.length === 0) {
        listDiv.innerHTML = '<em>No templates</em>';
        return;
    }
    listDiv.innerHTML = '<table style="width:100%;border-collapse:collapse;">' +
        '<tr style="border-bottom:1px solid #30363d;"><th style="text-align:left;padding:4px;">Key</th><th>Name</th><th>vCPUs</th><th>Memory</th><th>Windows</th><th>Arch</th><th>Image</th><th></th></tr>' +
        list.map(function(t) {
            return '<tr style="border-bottom:1px solid #21262d;">' +
                '<td style="padding:4px;">' + escapeHtml(t.key) + '</td>' +
                '<td style="padding:4px;">' + escapeHtml(t.name) + '</td>' +
                '<td style="padding:4px;text-align:center;">' + escapeHtml(t.vcpus) + '</td>' +
                '<td style="padding:4px;text-align:center;">' + escapeHtml(t.memory) + '</td>' +
                '<td style="padding:4px;text-align:center;">' + (t.is_windows === '1' ? 'Yes' : 'No') + '</td>' +
                '<td style="padding:4px;text-align:center;">' + escapeHtml(t.arch || 'x86_64') + '</td>' +
                '<td style="padding:4px;">' + escapeHtml(t.image) + '</td>' +
                '<td style="padding:4px;white-space:nowrap;">' +
                    '<button class="btn-remove" style="background:#1f6feb;margin-right:4px;" onclick="editOsTemplate(' + t.id + ')">Edit</button>' +
                    '<button class="btn-remove" onclick="deleteOsTemplate(' + t.id + ')">X</button>' +
                '</td></tr>';
        }).join('') +
        '</table>';
}

window.saveOsTemplate = async function() {
    var id = document.getElementById('tpl-edit-id').value;
    var data = {
        key: val('tpl-key'),
        name: val('tpl-name'),
        vcpus: val('tpl-vcpus'),
        memory: val('tpl-memory'),
        is_windows: val('tpl-is-windows'),
        arch: val('tpl-arch'),
        image: val('tpl-image'),
    };
    if (!data.key) { alert('Key is required'); return; }
    if (!data.name) data.name = data.key;

    var url = id ? '/api/os-templates/update' : '/api/os-templates/create';
    if (id) data.id = parseInt(id);

    try {
        var response = await apiFetch(url, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(data),
        });
        var result = await safeJson(response);
        if (result && result.success) {
            resetTplForm();
            await loadOsTemplates();
        } else {
            alert('Error: ' + (result ? result.message : 'Unknown'));
        }
    } catch (e) {
        alert('Error: ' + e.message);
    }
};

window.editOsTemplate = function(id) {
    var list = window._osTemplateList || [];
    var t = list.find(function(x) { return x.id === id; });
    if (!t) return;
    document.getElementById('tpl-edit-id').value = t.id;
    document.getElementById('tpl-key').value = t.key;
    document.getElementById('tpl-name').value = t.name;
    document.getElementById('tpl-vcpus').value = t.vcpus;
    document.getElementById('tpl-memory').value = t.memory;
    document.getElementById('tpl-is-windows').value = t.is_windows;
    document.getElementById('tpl-arch').value = t.arch || 'x86_64';
    document.getElementById('tpl-image').value = t.image;
    document.getElementById('tpl-form-legend').textContent = 'Edit Template: ' + t.name;
};

window.deleteOsTemplate = async function(id) {
    if (!confirm('Delete this template?')) return;
    try {
        var response = await apiFetch('/api/os-templates/delete', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ id: id }),
        });
        var result = await safeJson(response);
        if (result && result.success) {
            await loadOsTemplates();
        } else {
            alert('Error: ' + (result ? result.message : 'Unknown'));
        }
    } catch (e) {
        alert('Error: ' + e.message);
    }
};

window.resetTplForm = function() {
    document.getElementById('tpl-edit-id').value = '';
    document.getElementById('tpl-key').value = '';
    document.getElementById('tpl-name').value = '';
    document.getElementById('tpl-vcpus').value = '2';
    document.getElementById('tpl-memory').value = '2048';
    document.getElementById('tpl-is-windows').value = '0';
    document.getElementById('tpl-arch').value = 'x86_64';
    document.getElementById('tpl-image').value = '';
    document.getElementById('tpl-form-legend').textContent = 'Add Template';
};

// Tab switching
document.querySelectorAll('.tab').forEach(function(tab) {
    tab.addEventListener('click', function() {
        document.querySelectorAll('.tab').forEach(function(t) { t.classList.remove('active'); });
        document.querySelectorAll('.tab-panel').forEach(function(p) { p.classList.remove('active'); });
        tab.classList.add('active');
        document.getElementById('tab-' + tab.dataset.tab).classList.add('active');
        // Auto-load MDS config + SSH key list when switching to metadata tab
        if (tab.dataset.tab === 'metadata') { loadSshKeyList(); loadMdsConfig(); }
        // Auto-load ISO list when switching to mountiso tab
        if (tab.dataset.tab === 'mountiso') { loadIsoList(); }
        // Auto-load VM list when switching to vmlist tab
        if (tab.dataset.tab === 'vmlist') { loadVmListTable(); }
        // Auto-load image list when switching to listimage tab
        if (tab.dataset.tab === 'listimage') { loadImageList(); }
        // Auto-load disk list when switching to createdisk tab
        if (tab.dataset.tab === 'createdisk') { loadDiskList(); }
        // Auto-load backup list when switching to backup tab
        if (tab.dataset.tab === 'backup') { loadBackupList(); }
        // Auto-load DHCP table when switching to dhcp tab
        if (tab.dataset.tab === 'dhcp') { loadDhcpTable(); }
        // Auto-load internal network when switching to internal-net tab
        if (tab.dataset.tab === 'internal-net') { loadInternalNetwork(); }
        // Auto-load switch list when switching to switches tab
        if (tab.dataset.tab === 'switches') { loadSwitchList(); }
        // Auto-load OS template list when switching to os-templates tab
        if (tab.dataset.tab === 'os-templates') { loadDiskList().then(function() { loadOsTemplates(); }); }
    });
});

// Switch to a tab by name
function switchTab(tabName) {
    document.querySelectorAll('.tab').forEach(function(t) { t.classList.remove('active'); });
    document.querySelectorAll('.tab-panel').forEach(function(p) { p.classList.remove('active'); });
    var btn = document.querySelector('.tab[data-tab="' + tabName + '"]');
    if (btn) btn.classList.add('active');
    var panel = document.getElementById('tab-' + tabName);
    if (panel) panel.classList.add('active');
}

// Safe JSON parse from response
async function safeJson(response) {
    var text = await response.text();
    try {
        return JSON.parse(text);
    } catch (e) {
        throw new Error('Server returned non-JSON (HTTP ' + response.status + '): ' + text.substring(0, 200));
    }
}

// API call helper
async function apiCall(operation, payload) {
    var statusEl = document.getElementById('status-indicator');
    var outputEl = document.getElementById('output');

    statusEl.className = 'loading';
    statusEl.textContent = 'Executing ' + operation + '...';
    outputEl.textContent = '';

    document.querySelectorAll('.execute-btn').forEach(function(b) { b.disabled = true; });

    try {
        var apiPath = (operation.startsWith('vnc/') || operation.startsWith('disk/') || operation.startsWith('iso/') || operation.startsWith('backup/') || operation.startsWith('switch/') || operation.startsWith('group/') || operation.startsWith('sshkey/')) ? '/api/' + operation : '/api/vm/' + operation;
        console.log('apiCall:', operation, '->', apiPath);
        var response = await apiFetch(apiPath, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(payload),
        });
        var data = await safeJson(response);

        if (data.success) {
            statusEl.className = 'success';
            statusEl.textContent = data.message;
            outputEl.textContent = data.output || '(no output)';
            return true;
        } else {
            statusEl.className = 'error';
            statusEl.textContent = 'Error: ' + data.message;
            outputEl.textContent = data.output || '';
            return false;
        }
    } catch (err) {
        statusEl.className = 'error';
        statusEl.textContent = 'Error [' + operation + ']: ' + err.message;
        return false;
    } finally {
        document.querySelectorAll('.execute-btn').forEach(function(b) { b.disabled = false; });
    }
}

// Helper
function val(id) {
    return document.getElementById(id).value;
}

// SimpleCmd operations (smac only)
async function executeSimple(operation) {
    var ok = await apiCall(operation, {
        smac: val(operation + '-smac'),
    });
    if (ok) {
        loadVmList();
        loadVmListTable();
    }
}

// ======== Backup Management ========

async function executeBackup() {
    var ok = await apiCall('backup', {
        smac: val('backup-smac'),
    });
    if (ok) {
        loadBackupList();
    }
}

async function loadBackupList() {
    try {
        var response = await apiFetch('/api/backup/list');
        var backups = await safeJson(response);
        var listDiv = document.getElementById('backup-list');
        if (!listDiv) return;
        if (backups.length === 0) {
            listDiv.innerHTML = '<em>No backups</em>';
            return;
        }
        var html = '<table style="width:100%;border-collapse:collapse;font-size:0.85rem;">' +
            '<tr style="border-bottom:2px solid #30363d;">' +
            '<th style="text-align:left;padding:6px 8px;color:#58a6ff;">VM</th>' +
            '<th style="text-align:left;padding:6px 8px;color:#58a6ff;">Date & Time</th>' +
            '<th style="text-align:right;padding:6px 8px;color:#58a6ff;">Size</th>' +
            '<th style="text-align:right;padding:6px 8px;color:#58a6ff;"></th>' +
            '</tr>';
        backups.forEach(function(b) {
            html += '<tr style="border-bottom:1px solid #21262d;">' +
                '<td style="padding:6px 8px;">' + escapeHtml(b.vm_name) + '</td>' +
                '<td style="padding:6px 8px;">' + (b.datetime ? escapeHtml(b.datetime) : '<em>unknown</em>') + '</td>' +
                '<td style="padding:6px 8px;text-align:right;">' + escapeHtml(formatSize(b.size)) + '</td>' +
                '<td style="padding:6px 8px;text-align:right;">' +
                '<button class="btn-remove" onclick="deleteBackup(\'' + b.filename.replace(/'/g, "\\'") + '\')">X</button>' +
                '</td></tr>';
        });
        html += '</table>';
        listDiv.innerHTML = html;
    } catch (err) {
        console.error('Failed to load backup list:', err);
    }
}

async function deleteBackup(filename) {
    if (!confirm('Delete backup "' + filename + '"?')) return;
    var ok = await apiCall('backup/delete', { filename: filename });
    if (ok) {
        loadBackupList();
    }
}

// Collect VM config from the Create/Edit form
function collectVmConfig() {
    var adapterRows = document.querySelectorAll('#start-network-adapters .adapter-row');
    var network_adapters = Array.from(adapterRows).map(function(row) {
        return {
            netid: row.querySelector('.adapter-netid').value,
            mac: row.querySelector('.adapter-mac').value,
            vlan: row.querySelector('.adapter-vlan').value,
            mode: row.querySelector('.adapter-mode').value,
            switch_name: row.querySelector('.adapter-switch') ? row.querySelector('.adapter-switch').value : '',
            bridge_iface: row.querySelector('.adapter-bridge-iface') ? row.querySelector('.adapter-bridge-iface').value : '',
        };
    });

    var diskRows = document.querySelectorAll('#start-disks .disk-row');
    var disks = Array.from(diskRows).map(function(row) {
        var presetSel = row.querySelector('.disk-iops-preset');
        var presetKey = presetSel ? presetSel.value : 'standard';
        var p = IOPS_PRESETS[presetKey];
        return {
            diskid: row.querySelector('.disk-diskid').value,
            diskname: row.querySelector('.disk-diskname').value,
            'iops-total': p ? p.total : row.querySelector('.disk-iops-total').value,
            'iops-total-max': p ? p.max : row.querySelector('.disk-iops-total-max').value,
            'iops-total-max-length': p ? p.length : row.querySelector('.disk-iops-total-max-length').value,
        };
    }).filter(function(d) { return d.diskname; }); // filter out empty disk selections

    var pciRows = document.querySelectorAll('#start-pci-devices .pci-row');
    var pci_devices = Array.from(pciRows).map(function(row) {
        return { host: row.querySelector('.pci-host').value.trim() };
    }).filter(function(p) { return p.host; });

    return {
        cpu: {
            vcpus: val('start-vcpus'),
        },
        memory: { size: val('start-memory-size') },
        features: { is_windows: val('start-is-windows'), arch: val('start-arch'), cloudinit: val('start-cloudinit') },
        network_adapters: network_adapters,
        disks: disks,
        pci_devices: pci_devices,
    };
}

// Create VM — save config to DB + create disk
async function executeCreateVm() {
    try {
    var vmName = val('create-vm-name').trim();
    if (!vmName) {
        alert('Please enter a VM-NAME');
        return;
    }
    var config = collectVmConfig();
    // Require at least one disk
    if (!config.disks || config.disks.length === 0) {
        alert('Please select at least one disk');
        return;
    }
    // Frontend MAC uniqueness check
    var macErr = validateMacUniqueness(config, null);
    if (macErr) { alert(macErr); return; }
    var ok = await apiCall('create-config', {
        smac: vmName,
        config: config,
    });
    if (ok) {
        loadUsedMacs(); // refresh MAC cache
        // Set group if specified
        var groupName = getCreateFormGroup();
        if (groupName) {
            await setVmGroup(vmName, groupName);
        }
        loadVmList();
        loadVmListTable();
        loadGroupList();
        // Reset edit mode
        window._editingVm = null;
        document.getElementById('create-title').textContent = 'Create VM';
        document.getElementById('create-submit-btn').textContent = 'Create VM';
        document.getElementById('create-submit-btn').setAttribute('onclick', 'executeCreateVm()');
        document.getElementById('create-vm-name').disabled = false;
        document.getElementById('create-os-template').value = 'custom';
        document.getElementById('create-group').value = '';
        document.getElementById('create-group-new').value = '';
    }
    } catch (err) {
        document.getElementById('status-indicator').className = 'error';
        document.getElementById('status-indicator').textContent = 'Error: ' + err.message;
        document.getElementById('output').textContent = err.stack || '';
        console.error('executeCreateVm error:', err);
    }
}

// Update VM config (edit mode)
async function executeUpdateVm() {
    try {
    var vmName = val('create-vm-name').trim();
    if (!vmName) {
        alert('Please enter a VM-NAME');
        return;
    }
    var config = collectVmConfig();
    // Require at least one disk
    if (!config.disks || config.disks.length === 0) {
        alert('Please select at least one disk');
        return;
    }
    // Frontend MAC uniqueness check (exclude this VM)
    var macErr = validateMacUniqueness(config, vmName);
    if (macErr) { alert(macErr); return; }
    var ok = await apiCall('update-config', {
        smac: vmName,
        config: config,
    });
    if (ok) {
        loadUsedMacs(); // refresh MAC cache
        // Set group
        var groupName = getCreateFormGroup();
        await setVmGroup(vmName, groupName);
        loadVmList();
        loadVmListTable();
        loadGroupList();
        // Reset edit mode
        window._editingVm = null;
        document.getElementById('create-title').textContent = 'Create VM';
        document.getElementById('create-submit-btn').textContent = 'Create VM';
        document.getElementById('create-submit-btn').setAttribute('onclick', 'executeCreateVm()');
        document.getElementById('create-vm-name').disabled = false;
        document.getElementById('create-os-template').value = 'custom';
        document.getElementById('create-group').value = '';
        document.getElementById('create-group-new').value = '';
        // Switch to VM List tab after saving
        switchTab('vmlist');
    }
    } catch (err) {
        document.getElementById('status-indicator').className = 'error';
        document.getElementById('status-indicator').textContent = 'Error: ' + err.message;
        document.getElementById('output').textContent = err.stack || '';
        console.error('executeUpdateVm error:', err);
    }
}

// ──────────────────────────────────────────
// MAC Address Uniqueness
// ──────────────────────────────────────────
var _allUsedMacs = []; // [{mac, vm_name}]

async function loadUsedMacs() {
    try {
        var res = await apiFetch('/api/mac/list');
        var data = await safeJson(res);
        _allUsedMacs = (data && data.macs) ? data.macs : [];
    } catch (e) {
        _allUsedMacs = [];
    }
}

// Generate random MAC address (52:54:00:xx:xx:xx) — avoids collisions with existing MACs
function genMac() {
    var hex = '0123456789abcdef';
    var usedSet = {};
    for (var i = 0; i < _allUsedMacs.length; i++) {
        usedSet[_allUsedMacs[i].mac] = true;
    }
    // Also collect MACs already in the current form
    var formMacs = document.querySelectorAll('#start-network-adapters .adapter-mac');
    for (var j = 0; j < formMacs.length; j++) {
        usedSet[formMacs[j].value.toLowerCase()] = true;
    }
    for (var attempt = 0; attempt < 1000; attempt++) {
        var mac = '52:54:00';
        for (var k = 0; k < 3; k++) {
            mac += ':' + hex[Math.floor(Math.random() * 16)] + hex[Math.floor(Math.random() * 16)];
        }
        if (!usedSet[mac]) return mac;
    }
    // Fallback (extremely unlikely)
    return '52:54:00:ff:ff:ff';
}

// Validate MAC uniqueness before submit.
// excludeVm: when editing, exclude this VM's MACs from the check.
function validateMacUniqueness(config, excludeVm) {
    var adapters = config.network_adapters || [];
    var newMacs = [];
    for (var i = 0; i < adapters.length; i++) {
        var mac = (adapters[i].mac || '').toLowerCase().trim();
        if (!mac) continue;
        // Check for duplicates within the form
        if (newMacs.indexOf(mac) !== -1) {
            return 'Duplicate MAC address "' + mac + '" within this VM config';
        }
        newMacs.push(mac);
    }
    // Check against all existing VMs
    for (var j = 0; j < newMacs.length; j++) {
        for (var k = 0; k < _allUsedMacs.length; k++) {
            if (newMacs[j] === _allUsedMacs[k].mac && _allUsedMacs[k].vm_name !== excludeVm) {
                return 'MAC address "' + newMacs[j] + '" is already used by VM "' + _allUsedMacs[k].vm_name + '"';
            }
        }
    }
    return null; // OK
}

// Dynamic rows - Network Adapter
function addNetworkAdapter(existing) {
    var container = document.getElementById('start-network-adapters');
    var count = container.querySelectorAll('.adapter-row').length;
    var row = document.createElement('div');
    row.className = 'adapter-row';

    var mode = (existing && existing.mode) || 'nat';
    var switchName = (existing && existing.switch_name) || '';
    var bridgeIface = (existing && existing.bridge_iface) || '';
    var switchDisplay = mode === 'switch' ? '' : 'display:none;';
    var bridgeDisplay = mode === 'bridge' ? '' : 'display:none;';
    var vlanOpacity = mode === 'switch' ? '1' : '0.4';

    row.innerHTML =
        '<input class="adapter-netid" placeholder="Net ID" value="' + (existing ? existing.netid : count) + '" readonly style="opacity:0.6;cursor:default;">' +
        '<input class="adapter-mac" placeholder="MAC" value="' + (existing ? existing.mac : genMac()) + '">' +
        '<input class="adapter-vlan" placeholder="VLAN" style="opacity:' + vlanOpacity + '" value="' + (existing ? (existing.vlan || '0') : '0') + '">' +
        '<select class="adapter-mode" onchange="onNetModeChange(this)">' +
            '<option value="nat"' + (mode === 'nat' ? ' selected' : '') + '>NAT</option>' +
            '<option value="switch"' + (mode === 'switch' ? ' selected' : '') + '>Switch</option>' +
            '<option value="bridge"' + (mode === 'bridge' ? ' selected' : '') + '>Bridge</option>' +
        '</select>' +
        '<select class="adapter-switch" style="' + switchDisplay + '"><option value="">-- switch --</option></select>' +
        '<input class="adapter-bridge-iface" placeholder="Interface (optional)" style="' + bridgeDisplay + 'max-width:140px;" value="' + escapeHtml(bridgeIface) + '">' +
        '<button type="button" class="btn-remove" onclick="this.parentElement.remove()">X</button>';
    container.appendChild(row);
    populateSwitchSelect(row.querySelector('.adapter-switch'), switchName);
    updateBridgeWarning();
}

function onNetModeChange(selectEl) {
    var row = selectEl.closest('.adapter-row');
    var switchSelect = row.querySelector('.adapter-switch');
    var bridgeInput = row.querySelector('.adapter-bridge-iface');
    var vlanInput = row.querySelector('.adapter-vlan');
    if (selectEl.value === 'switch') {
        switchSelect.style.display = '';
        if (bridgeInput) bridgeInput.style.display = 'none';
        if (vlanInput) vlanInput.style.opacity = '1';
    } else if (selectEl.value === 'bridge') {
        switchSelect.style.display = 'none';
        switchSelect.value = '';
        if (bridgeInput) bridgeInput.style.display = '';
        if (vlanInput) vlanInput.style.opacity = '0.4';
    } else {
        // NAT
        switchSelect.style.display = 'none';
        switchSelect.value = '';
        if (bridgeInput) bridgeInput.style.display = 'none';
        if (vlanInput) vlanInput.style.opacity = '0.4';
    }
    updateBridgeWarning();
}

function updateBridgeWarning() {
    var adapters = document.querySelectorAll('#start-network-adapters .adapter-mode');
    var hasBridge = false;
    adapters.forEach(function(sel) {
        if (sel.value === 'bridge') hasBridge = true;
    });
    var warn = document.getElementById('bridge-sudo-warning');
    if (warn) warn.style.display = hasBridge ? '' : 'none';
}

// IOPS Presets — total, max, max_length
var IOPS_PRESETS = {
    'low':      { total: '3200',  max: '3840',  length: '60', label: 'Low (3.2K)' },
    'standard': { total: '9600',  max: '11520', length: '60', label: 'Standard (9.6K)' },
    'high':     { total: '19200', max: '23040', length: '60', label: 'High (19.2K)' },
    'ultra':    { total: '38400', max: '46080', length: '60', label: 'Ultra (38.4K)' },
    'max':      { total: '76800', max: '92160', length: '60', label: 'Max (76.8K)' },
    'unlimited':{ total: '0',     max: '0',     length: '0',  label: 'Unlimited' },
};

// Match existing IOPS values to a preset key, or 'custom'
function matchIopsPreset(total, max, length) {
    for (var key in IOPS_PRESETS) {
        var p = IOPS_PRESETS[key];
        if (p.total === total && p.max === max && p.length === length) return key;
    }
    return 'custom';
}

// Build IOPS select HTML with optional selected key
function buildIopsSelect(selectedKey) {
    var html = '';
    for (var key in IOPS_PRESETS) {
        html += '<option value="' + key + '"' + (key === selectedKey ? ' selected' : '') + '>' + IOPS_PRESETS[key].label + '</option>';
    }
    html += '<option value="custom"' + (selectedKey === 'custom' ? ' selected' : '') + '>Custom...</option>';
    return html;
}

// Handle IOPS preset change — show/hide custom inputs
function onIopsPresetChange(selectEl) {
    var row = selectEl.closest('.disk-row');
    var customDiv = row.querySelector('.disk-iops-custom');
    if (selectEl.value === 'custom') {
        customDiv.style.display = '';
    } else {
        customDiv.style.display = 'none';
        var p = IOPS_PRESETS[selectEl.value];
        if (p) {
            row.querySelector('.disk-iops-total').value = p.total;
            row.querySelector('.disk-iops-total-max').value = p.max;
            row.querySelector('.disk-iops-total-max-length').value = p.length;
        }
    }
}

// Dynamic rows - Disk (with dropdown for disk name)
function addDisk(selectedValue, iopsKey) {
    var container = document.getElementById('start-disks');
    var count = container.querySelectorAll('.disk-row').length;
    var presetKey = iopsKey || 'standard';
    var preset = IOPS_PRESETS[presetKey] || IOPS_PRESETS['standard'];
    var customDisplay = presetKey === 'custom' ? '' : 'display:none;';
    var row = document.createElement('div');
    row.className = 'disk-row';
    row.innerHTML =
        '<input class="disk-diskid" placeholder="Disk ID" value="' + count + '" readonly style="opacity:0.6;cursor:default;">' +
        '<select class="disk-diskname"><option value="">-- select disk --</option></select>' +
        '<select class="disk-iops-preset" onchange="onIopsPresetChange(this)">' + buildIopsSelect(presetKey) + '</select>' +
        '<span class="disk-iops-custom" style="' + customDisplay + '">' +
        '<input class="disk-iops-total" placeholder="IOPS" value="' + preset.total + '" style="width:70px;">' +
        '<input class="disk-iops-total-max" placeholder="Max" value="' + preset.max + '" style="width:70px;">' +
        '<input class="disk-iops-total-max-length" placeholder="Len" value="' + preset.length + '" style="width:50px;">' +
        '</span>' +
        '<button type="button" class="btn-remove" onclick="this.parentElement.remove()">X</button>';
    container.appendChild(row);
    populateDiskSelect(row.querySelector('.disk-diskname'), selectedValue || '');
}

// ======== PCI Passthrough (VFIO) ========

window._vfioDevices = [];

async function loadVfioDevices() {
    try {
        var res = await apiFetch('/api/devices/vfio');
        window._vfioDevices = await safeJson(res);
    } catch(e) { window._vfioDevices = []; }
}

function buildVfioOptions(selectedAddr) {
    var html = '<option value="">-- select or type below --</option>';
    window._vfioDevices.forEach(function(d) {
        var label = d.address;
        if (d.description) label += ' — ' + d.description.substring(0, 60);
        var sel = (selectedAddr && d.address === selectedAddr) ? ' selected' : '';
        html += '<option value="' + d.address + '"' + sel + '>' + label + '</option>';
    });
    return html;
}

function addPciDevice(existing) {
    var container = document.getElementById('start-pci-devices');
    var row = document.createElement('div');
    row.className = 'pci-row';
    row.style.cssText = 'display:flex;gap:6px;align-items:center;margin-bottom:4px;';
    var addr = (existing && existing.host) || '';
    var hasVfio = window._vfioDevices.length > 0;
    if (hasVfio) {
        row.innerHTML =
            '<select class="pci-select" onchange="onPciSelectChange(this)" style="min-width:200px;">' + buildVfioOptions(addr) + '</select>' +
            '<input class="pci-host" placeholder="PCI Address (0000:01:00.0)" value="' + addr + '" style="width:180px;">' +
            '<button type="button" class="btn-remove" onclick="this.parentElement.remove()">X</button>';
    } else {
        row.innerHTML =
            '<input class="pci-host" placeholder="PCI Address (0000:01:00.0)" value="' + addr + '" style="width:250px;">' +
            '<button type="button" class="btn-remove" onclick="this.parentElement.remove()">X</button>';
    }
    container.appendChild(row);
}

function onPciSelectChange(sel) {
    var row = sel.closest('.pci-row');
    var input = row.querySelector('.pci-host');
    if (input && sel.value) input.value = sel.value;
}

// ======== Disk Management ========

// Load disk list from API and cache it
async function loadDiskList() {
    try {
        var response = await apiFetch('/api/disk/list');
        var disks = await safeJson(response);
        window._diskList = disks;
        // Render file list in Create Disk tab
        var listDiv = document.getElementById('disk-file-list');
        if (!listDiv) return;
        if (disks.length === 0) {
            listDiv.innerHTML = '<em>No disk files</em>';
        } else {
            listDiv.innerHTML = disks.map(function(d) {
                var safeName = d.name.replace(/'/g, "\\'");
                var ownerText = d.owner ? ' <small style="color:#58a6ff;">[' + escapeHtml(d.owner) + ']</small>' : ' <small style="color:#3fb950;">[free]</small>';
                var exportBtn = '<span class="export-dropdown">' +
                    '<button class="btn-export" onclick="this.parentElement.classList.toggle(\'open\')" title="Export / Download">Export ▾</button>' +
                    '<span class="export-dropdown-content">' +
                    '<a href="javascript:void(0)" onclick="exportDisk(\'' + safeName + '\', \'qcow2\')">⬇ qcow2 (original)</a>' +
                    '<a href="javascript:void(0)" onclick="exportDisk(\'' + safeName + '\', \'vmdk\')">⬇ VMDK (VMware)</a>' +
                    '<a href="javascript:void(0)" onclick="exportDisk(\'' + safeName + '\', \'vdi\')">⬇ VDI (VirtualBox)</a>' +
                    '<a href="javascript:void(0)" onclick="exportDisk(\'' + safeName + '\', \'vhdx\')">⬇ VHDX (Hyper-V)</a>' +
                    '<a href="javascript:void(0)" onclick="exportDisk(\'' + safeName + '\', \'raw\')">⬇ Raw image</a>' +
                    '</span></span>';
                var resizeBtn = '<button class="btn-clone" onclick="resizeDisk(\'' + safeName + '\', \'' + (d.disk_size || '').replace(/'/g, "\\'") + '\')">Resize</button>';
                var cloneBtn = '<button class="btn-clone" onclick="cloneDisk(\'' + safeName + '\')">Clone</button>';
                var cloneTplBtn = '<button class="btn-clone" onclick="cloneDiskAsTemplate(\'' + safeName + '\')" title="Clone as template image">→ Template</button>';
                var deleteBtn = d.owner ? '' : '<button class="btn-remove" onclick="deleteDisk(\'' + safeName + '\')">X</button>';
                var sizeInfo = d.disk_size ? d.disk_size : formatSize(d.size);
                return '<div style="display:flex;justify-content:space-between;align-items:center;padding:4px 0;border-bottom:1px solid #333;">' +
                    '<span>' + escapeHtml(d.name) + '.qcow2 <small>(' + escapeHtml(sizeInfo) + ')</small>' + ownerText + '</span>' +
                    '<span>' + exportBtn + ' ' + resizeBtn + ' ' + cloneBtn + ' ' + cloneTplBtn + ' ' + deleteBtn + '</span>' +
                    '</div>';
            }).join('');
        }
        // Refresh any disk selects on the page
        refreshAllDiskSelects();
    } catch (err) {
        console.error('Failed to load disk list:', err);
    }
}

// Populate a single disk <select> with available (free) disks + disks owned by current editing VM
function populateDiskSelect(selectEl, selectedValue) {
    var disks = window._diskList || [];
    var editingVm = window._editingVm || '';
    var current = selectedValue || selectEl.value;
    selectEl.innerHTML = '<option value="">-- select disk --</option>';
    disks.forEach(function(d) {
        if (d.name.indexOf('template-') === 0) return; // skip template images
        // Show disk if: free (no owner) OR owned by the VM being edited OR matches current selection
        if (!d.owner || d.owner === editingVm || d.name === current) {
            var opt = document.createElement('option');
            opt.value = d.name;
            var label = d.name + ' (' + (d.disk_size || formatSize(d.size)) + ')';
            if (d.owner && d.owner !== editingVm) {
                label += ' [' + d.owner + ']';
            }
            opt.textContent = label;
            selectEl.appendChild(opt);
        }
    });
    if (current) selectEl.value = current;
}

// Refresh all disk selects on the Create VM form
function refreshAllDiskSelects() {
    var selects = document.querySelectorAll('#start-disks .disk-diskname');
    selects.forEach(function(sel) {
        populateDiskSelect(sel);
    });
}

// Create a new disk
async function executeCreateDisk() {
    var name = val('createdisk-name').trim();
    if (!name) {
        alert('Please enter a Disk Name');
        return;
    }
    var ok = await apiCall('disk/create', {
        name: name,
        size: val('createdisk-size'),
    });
    if (ok) {
        document.getElementById('createdisk-name').value = '';
        loadDiskList();
    }
}

// Resize a disk
async function resizeDisk(name, currentSize) {
    var newSize = prompt('Resize disk: ' + name + '.qcow2\nCurrent size: ' + currentSize + '\nEnter new size (e.g. 20G, 512M):', currentSize || '40G');
    if (!newSize) return;
    var ok = await apiCall('disk/resize', { name: name, size: newSize });
    if (ok) loadDiskList();
}

// Delete a disk file
async function deleteDisk(name) {
    if (!confirm('Delete disk: ' + name + '.qcow2?')) return;
    var statusEl = document.getElementById('status-indicator');
    statusEl.className = 'loading';
    statusEl.textContent = 'Deleting ' + name + '...';
    try {
        var response = await apiFetch('/api/disk/delete', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ name: name }),
        });
        var data = await safeJson(response);
        if (data.success) {
            statusEl.className = 'success';
            statusEl.textContent = data.message;
            loadDiskList();
        } else {
            statusEl.className = 'error';
            statusEl.textContent = 'Error: ' + data.message;
        }
    } catch (err) {
        statusEl.className = 'error';
        statusEl.textContent = 'Network error: ' + err.message;
    }
}

// Clone a disk image
async function cloneDisk(source) {
    var newName = prompt('Clone "' + source + '" to new name:', source + '-clone');
    if (!newName) return;
    newName = newName.trim();
    if (!newName) return;
    var statusEl = document.getElementById('status-indicator');
    statusEl.className = 'loading';
    statusEl.textContent = 'Cloning ' + source + ' -> ' + newName + '...';
    try {
        var response = await apiFetch('/api/disk/clone', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ source: source, name: newName }),
        });
        var data = await safeJson(response);
        if (data.success) {
            statusEl.className = 'success';
            statusEl.textContent = data.message;
            loadDiskList();
        } else {
            statusEl.className = 'error';
            statusEl.textContent = 'Error: ' + data.message;
        }
    } catch (err) {
        statusEl.className = 'error';
        statusEl.textContent = 'Network error: ' + err.message;
    }
}

// Clone disk as template image (auto-prefix "template-")
async function cloneDiskAsTemplate(source) {
    var baseName = source.replace(/^template-/, '');
    var defaultName = 'template-' + baseName;
    var newName = prompt('Clone "' + source + '" as template image:', defaultName);
    if (!newName) return;
    newName = newName.trim();
    if (!newName) return;
    if (newName.indexOf('template-') !== 0) newName = 'template-' + newName;
    var statusEl = document.getElementById('status-indicator');
    statusEl.className = 'loading';
    statusEl.textContent = 'Cloning ' + source + ' -> ' + newName + ' (template)...';
    try {
        var response = await apiFetch('/api/disk/clone', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ source: source, name: newName }),
        });
        var data = await safeJson(response);
        if (data.success) {
            statusEl.className = 'success';
            statusEl.textContent = data.message;
            loadDiskList();
        } else {
            statusEl.className = 'error';
            statusEl.textContent = 'Error: ' + data.message;
        }
    } catch (err) {
        statusEl.className = 'error';
        statusEl.textContent = 'Network error: ' + err.message;
    }
}

// Export/download a disk in the specified format
function exportDisk(name, format) {
    // Close dropdown
    document.querySelectorAll('.export-dropdown.open').forEach(function(el) { el.classList.remove('open'); });
    var statusEl = document.getElementById('status-indicator');
    statusEl.className = 'loading';
    var label = format === 'qcow2' ? format : format.toUpperCase();
    statusEl.textContent = 'Downloading ' + name + '.' + format + '...';
    // Build download URL with API key
    var url = '/api/disk/export/' + encodeURIComponent(name);
    if (format && format !== 'qcow2') url += '?format=' + format;
    // Use fetch with API key header, then trigger download via blob
    apiFetch(url).then(function(response) {
        if (!response.ok) {
            return response.json().then(function(data) {
                throw new Error(data.message || 'Export failed');
            });
        }
        return response.blob();
    }).then(function(blob) {
        var a = document.createElement('a');
        a.href = URL.createObjectURL(blob);
        a.download = name + '.' + format;
        document.body.appendChild(a);
        a.click();
        document.body.removeChild(a);
        URL.revokeObjectURL(a.href);
        statusEl.className = 'success';
        statusEl.textContent = 'Downloaded ' + name + '.' + format;
    }).catch(function(err) {
        statusEl.className = 'error';
        statusEl.textContent = 'Export error: ' + err.message;
    });
}

// Close export dropdowns when clicking outside
document.addEventListener('click', function(e) {
    if (!e.target.closest('.export-dropdown')) {
        document.querySelectorAll('.export-dropdown.open').forEach(function(el) { el.classList.remove('open'); });
    }
});

// ======== Switch Management ========

async function loadSwitchList() {
    try {
        var response = await apiFetch('/api/switch/list');
        var switches = await safeJson(response);
        window._switchList = switches;

        // Build map: switch_name -> [vm names]
        var switchVmMap = {};
        switches.forEach(function(sw) { switchVmMap[sw.name] = []; });
        var vms = window._vmList || [];
        vms.forEach(function(vm) {
            var config = {};
            try { config = JSON.parse(vm.config); } catch(e) {}
            var adapters = config.network_adapters || [];
            adapters.forEach(function(a) {
                if (a.mode === 'switch' && a.switch_name && switchVmMap[a.switch_name] !== undefined) {
                    var label = vm.smac + ':vlan' + (a.vlan || '0');
                    if (switchVmMap[a.switch_name].indexOf(label) === -1) {
                        switchVmMap[a.switch_name].push(label);
                    }
                }
            });
        });

        var listDiv = document.getElementById('switch-list');
        if (listDiv) {
            if (switches.length === 0) {
                listDiv.innerHTML = '<em>No switches created. Create one to enable inter-VM networking.</em>';
            } else {
                listDiv.innerHTML = switches.map(function(sw) {
                    var safeName = sw.name.replace(/'/g, "\\'");
                    var vmList = switchVmMap[sw.name] || [];
                    var vmHtml = vmList.length > 0
                        ? ' <small style="color:#3fb950;">(' + vmList.map(escapeHtml).join(', ') + ')</small>'
                        : ' <small style="color:#8b949e;">(no VMs)</small>';
                    return '<div style="display:flex;justify-content:space-between;align-items:center;padding:4px 0;border-bottom:1px solid #333;">' +
                        '<span>' + escapeHtml(sw.name) +
                        ' <small style="color:#58a6ff;">[mcast port ' + sw.mcast_port + ']</small>' +
                        vmHtml + '</span>' +
                        '<span>' +
                        '<button class="btn-clone" onclick="renameSwitch(' + sw.id + ', \'' + safeName + '\')">Rename</button> ' +
                        '<button class="btn-remove" onclick="deleteSwitch(' + sw.id + ', \'' + safeName + '\')">X</button>' +
                        '</span></div>';
                }).join('');
            }
        }
        refreshAllSwitchSelects();
    } catch (err) {
        console.error('Failed to load switch list:', err);
    }
}

async function executeCreateSwitch() {
    var name = val('switch-name').trim();
    if (!name) { alert('Please enter a Switch Name'); return; }
    var ok = await apiCall('switch/create', { name: name });
    if (ok) {
        document.getElementById('switch-name').value = '';
        loadSwitchList();
    }
}

async function deleteSwitch(id, name) {
    if (!confirm('Delete switch "' + name + '"?')) return;
    var ok = await apiCall('switch/delete', { id: id });
    if (ok) loadSwitchList();
}

async function renameSwitch(id, currentName) {
    var newName = prompt('Rename switch "' + currentName + '" to:', currentName);
    if (!newName || newName === currentName) return;
    var ok = await apiCall('switch/rename', { id: id, name: newName.trim() });
    if (ok) loadSwitchList();
}

function populateSwitchSelect(selectEl, selectedValue) {
    var switches = window._switchList || [];
    var current = selectedValue || selectEl.value;
    selectEl.innerHTML = '<option value="">-- switch --</option>';
    switches.forEach(function(sw) {
        var opt = document.createElement('option');
        opt.value = sw.name;
        opt.textContent = sw.name + ' (port ' + sw.mcast_port + ')';
        selectEl.appendChild(opt);
    });
    if (current) selectEl.value = current;
}

function refreshAllSwitchSelects() {
    var selects = document.querySelectorAll('#start-network-adapters .adapter-switch');
    selects.forEach(function(sel) { populateSwitchSelect(sel); });
}

// ======== Internal Network (VM-to-VM) ========

async function loadInternalNetwork() {
    var infoEl = document.getElementById('internal-net-info');
    var tbody = document.querySelector('#internal-net-table tbody');
    if (!infoEl || !tbody) return;
    infoEl.textContent = 'Loading...';
    tbody.innerHTML = '';
    try {
        var response = await apiFetch('/api/internal-network');
        var data = await safeJson(response);
        var members = data.members || [];
        var total = data.total || 0;
        var nextIp = data.next_available || '';
        infoEl.innerHTML = '<strong>Used:</strong> ' + total + ' / 245 &nbsp;&nbsp; <strong>Next available:</strong> ' + (nextIp || 'none') + ' &nbsp;&nbsp; <strong>Subnet:</strong> 192.168.100.0/24';
        if (members.length === 0) {
            tbody.innerHTML = '<tr><td colspan="5" style="text-align:center;opacity:0.6;">No VMs with internal IP assigned</td></tr>';
        } else {
            members.forEach(function(m) {
                var statusClass = m.status === 'running' ? 'success' : '';
                var tr = document.createElement('tr');
                tr.innerHTML = '<td>' + (m.vm_name || '') + '</td>'
                    + '<td><code>' + (m.internal_ip || '') + '</code></td>'
                    + '<td><code style="font-size:0.85em;">' + (m.internal_mac || '') + '</code></td>'
                    + '<td>' + (m.hostname || '') + '</td>'
                    + '<td><span class="' + statusClass + '">' + (m.status || 'stopped') + '</span></td>';
                tbody.appendChild(tr);
            });
        }
    } catch (err) {
        infoEl.textContent = 'Error: ' + err.message;
    }
}

// ======== Image Management ========

// Load image list
async function loadImageList() {
    try {
        var response = await apiFetch('/api/image/list');
        var images = await safeJson(response);
        var listDiv = document.getElementById('image-file-list');
        if (!listDiv) return;
        if (images.length === 0) {
            listDiv.innerHTML = '<em>No image files</em>';
        } else {
            listDiv.innerHTML = images.map(function(img) {
                return '<div style="display:flex;justify-content:space-between;align-items:center;padding:4px 0;border-bottom:1px solid #333;">' +
                    '<span>' + escapeHtml(img.name) + ' <small>(' + escapeHtml(formatSize(img.size)) + ')</small></span>' +
                    '<button class="btn-remove" onclick="deleteImage(\'' + img.name.replace(/'/g, "\\'") + '\')">X</button>' +
                    '</div>';
            }).join('');
        }
    } catch (err) {
        console.error('Failed to load image list:', err);
    }
}

// Upload image file
async function uploadImage() {
    var fileInput = document.getElementById('image-file-input');
    if (!fileInput.files.length) {
        alert('Please select an image file');
        return;
    }
    var file = fileInput.files[0];
    var statusEl = document.getElementById('status-indicator');
    var progressDiv = document.getElementById('image-upload-progress');
    var progressBar = document.getElementById('image-progress-bar');
    var progressText = document.getElementById('image-progress-text');

    progressDiv.style.display = 'block';
    progressBar.value = 0;
    progressText.textContent = 'Uploading...';
    statusEl.className = 'loading';
    statusEl.textContent = 'Uploading ' + file.name + '...';

    var xhr = new XMLHttpRequest();
    xhr.open('POST', '/api/image/upload');
    xhr.setRequestHeader('X-Filename', file.name);
    xhr.setRequestHeader('Content-Type', 'application/octet-stream');
    if (getApiKey()) xhr.setRequestHeader('X-API-Key', getApiKey());

    xhr.upload.onprogress = function(e) {
        if (e.lengthComputable) {
            var pct = Math.round(e.loaded / e.total * 100);
            progressBar.value = pct;
            progressText.textContent = pct + '% (' + formatSize(e.loaded) + ' / ' + formatSize(e.total) + ')';
        }
    };

    xhr.onload = function() {
        progressDiv.style.display = 'none';
        try {
            var data = JSON.parse(xhr.responseText);
            if (data.success) {
                statusEl.className = 'success';
                statusEl.textContent = data.message;
                fileInput.value = '';
                loadImageList();
            } else {
                statusEl.className = 'error';
                statusEl.textContent = 'Error: ' + data.message;
            }
        } catch (e) {
            statusEl.className = 'error';
            statusEl.textContent = 'Upload failed';
        }
    };

    xhr.onerror = function() {
        progressDiv.style.display = 'none';
        statusEl.className = 'error';
        statusEl.textContent = 'Network error during upload';
    };

    xhr.send(file);
}

// Delete image file
async function deleteImage(name) {
    if (!confirm('Delete image: ' + name + '?')) return;
    var statusEl = document.getElementById('status-indicator');
    statusEl.className = 'loading';
    statusEl.textContent = 'Deleting ' + name + '...';
    try {
        var response = await apiFetch('/api/image/delete', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ name: name }),
        });
        var data = await safeJson(response);
        if (data.success) {
            statusEl.className = 'success';
            statusEl.textContent = data.message;
            loadImageList();
        } else {
            statusEl.className = 'error';
            statusEl.textContent = 'Error: ' + data.message;
        }
    } catch (err) {
        statusEl.className = 'error';
        statusEl.textContent = 'Error: ' + err.message;
    }
}

// Mount ISO
async function executeMountIso() {
    var smac = val('mountiso-smac');
    var ok = await apiCall('mountiso', {
        smac: smac,
        isoname: val('mountiso-isoname'),
        drive: val('mountiso-drive'),
    });
    if (ok) {
        await apiCall('reset', { smac: smac });
    }
}

// Unmount ISO
function executeUnmountIso() {
    apiCall('unmountiso', {
        smac: val('mountiso-smac'),
        drive: val('mountiso-drive'),
    });
}

// Load ISO list and populate dropdown + file list
async function loadIsoList() {
    try {
        var response = await apiFetch('/api/iso/list');
        var isos = await safeJson(response);
        // Populate dropdown
        var sel = document.getElementById('mountiso-isoname');
        var current = sel.value;
        sel.innerHTML = '<option value="">-- select ISO --</option>';
        isos.forEach(function(iso) {
            var opt = document.createElement('option');
            opt.value = iso.name;
            opt.textContent = iso.name + ' (' + formatSize(iso.size) + ')';
            sel.appendChild(opt);
        });
        if (current) sel.value = current;
        // Populate file list
        var listDiv = document.getElementById('iso-file-list');
        if (isos.length === 0) {
            listDiv.innerHTML = '<em>No ISO files</em>';
        } else {
            listDiv.innerHTML = isos.map(function(iso) {
                return '<div style="display:flex;justify-content:space-between;align-items:center;padding:4px 0;border-bottom:1px solid #333;">' +
                    '<span>' + escapeHtml(iso.name) + ' <small>(' + escapeHtml(formatSize(iso.size)) + ')</small></span>' +
                    '<button class="btn-remove" onclick="deleteIso(\'' + iso.name.replace(/'/g, "\\'") + '\')">X</button>' +
                    '</div>';
            }).join('');
        }
    } catch (err) {
        console.error('Failed to load ISO list:', err);
    }
}

// Format bytes to human readable
function formatSize(bytes) {
    if (bytes < 1024) return bytes + ' B';
    if (bytes < 1048576) return (bytes / 1024).toFixed(1) + ' KB';
    if (bytes < 1073741824) return (bytes / 1048576).toFixed(1) + ' MB';
    return (bytes / 1073741824).toFixed(2) + ' GB';
}

// Upload ISO file
async function uploadIso() {
    var fileInput = document.getElementById('iso-file-input');
    if (!fileInput.files.length) {
        alert('Please select an ISO file');
        return;
    }
    var file = fileInput.files[0];
    if (!file.name.endsWith('.iso')) {
        alert('File must be an .iso file');
        return;
    }
    var statusEl = document.getElementById('status-indicator');
    var progressDiv = document.getElementById('iso-upload-progress');
    var progressBar = document.getElementById('iso-progress-bar');
    var progressText = document.getElementById('iso-progress-text');

    progressDiv.style.display = 'block';
    progressBar.value = 0;
    progressText.textContent = 'Uploading...';
    statusEl.className = 'loading';
    statusEl.textContent = 'Uploading ' + file.name + '...';

    var xhr = new XMLHttpRequest();
    xhr.open('POST', '/api/iso/upload');
    xhr.setRequestHeader('X-Filename', file.name);
    xhr.setRequestHeader('Content-Type', 'application/octet-stream');
    if (getApiKey()) xhr.setRequestHeader('X-API-Key', getApiKey());

    xhr.upload.onprogress = function(e) {
        if (e.lengthComputable) {
            var pct = Math.round(e.loaded / e.total * 100);
            progressBar.value = pct;
            progressText.textContent = pct + '% (' + formatSize(e.loaded) + ' / ' + formatSize(e.total) + ')';
        }
    };

    xhr.onload = function() {
        progressDiv.style.display = 'none';
        try {
            var data = JSON.parse(xhr.responseText);
            if (data.success) {
                statusEl.className = 'success';
                statusEl.textContent = data.message;
                fileInput.value = '';
                loadIsoList();
            } else {
                statusEl.className = 'error';
                statusEl.textContent = 'Error: ' + data.message;
            }
        } catch (e) {
            statusEl.className = 'error';
            statusEl.textContent = 'Upload failed';
        }
    };

    xhr.onerror = function() {
        progressDiv.style.display = 'none';
        statusEl.className = 'error';
        statusEl.textContent = 'Network error during upload';
    };

    xhr.send(file);
}

// Delete ISO file
async function deleteIso(name) {
    if (!confirm('Delete ISO: ' + name + '?')) return;
    var statusEl = document.getElementById('status-indicator');
    statusEl.className = 'loading';
    statusEl.textContent = 'Deleting ' + name + '...';
    try {
        var response = await apiFetch('/api/iso/delete', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ name: name }),
        });
        var data = await safeJson(response);
        if (data.success) {
            statusEl.className = 'success';
            statusEl.textContent = data.message;
            loadIsoList();
        } else {
            statusEl.className = 'error';
            statusEl.textContent = 'Error: ' + data.message;
        }
    } catch (err) {
        statusEl.className = 'error';
        statusEl.textContent = 'Network error: ' + err.message;
    }
}

// Live migrate
function executeLiveMigrate() {
    apiCall('livemigrate', {
        smac: val('livemigrate-smac'),
        to_node_ip: val('livemigrate-to-node-ip'),
    });
}

// VNC operations (from VM List actions)
// VNC active tracking
if (!window._vncActive) window._vncActive = {};

// Get VNC port from VM config cache
function getVmVncPort(smac) {
    var vms = window._vmListData || [];
    for (var i = 0; i < vms.length; i++) {
        if (vms[i].smac === smac) {
            try {
                var cfg = JSON.parse(vms[i].config);
                return cfg.vnc_port || null;
            } catch(e) {}
        }
    }
    return null;
}

async function vmVncStart(smac) {
    var port = getVmVncPort(smac);
    if (!port) { alert('No VNC port assigned for ' + smac); return; }
    // Generate one-time VNC token
    try {
        var response = await apiFetch('/api/vnc/token', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ smac: smac }),
        });
        var data = await safeJson(response);
        if (data && data.success && data.token) {
            window._vncActive[smac] = true;
            loadVmListTable();
            window.open('/vnc.html?token=' + encodeURIComponent(data.token), '_blank');
            return;
        }
    } catch (e) {}
    // Fallback if token generation fails
    window._vncActive[smac] = true;
    loadVmListTable();
    window.open('/vnc.html?smac=' + encodeURIComponent(smac), '_blank');
}

async function vmVncStop(smac) {
    var port = getVmVncPort(smac);
    if (!port) { alert('No VNC port assigned for ' + smac); return; }
    var ok = await apiCall('vnc/stop', { smac: smac, novncport: String(port) });
    if (ok) {
        delete window._vncActive[smac];
        loadVmListTable();
    }
}

// MDS Config - Load (per-VM)
// Generate unique Instance ID: i- + 17 hex (timestamp + random)
function genInstanceId() {
    var ts = Date.now().toString(16);
    var rand = '';
    for (var i = ts.length; i < 17; i++) rand += Math.floor(Math.random() * 16).toString(16);
    return 'i-' + ts + rand;
}

// Generate unique AMI ID: ami- + 8 hex
function genAmiId() {
    var hex = '';
    for (var i = 0; i < 8; i++) hex += Math.floor(Math.random() * 16).toString(16);
    return 'ami-' + hex;
}

// Collect all used Local IPv4 addresses from all VMs
function getUsedIpv4s(excludeSmac) {
    var used = [];
    var vms = window._vmList || [];
    vms.forEach(function(vm) {
        if (vm.smac === excludeSmac) return;
        try {
            var cfg = typeof vm.config === 'string' ? JSON.parse(vm.config) : vm.config;
            if (cfg && cfg.mds && cfg.mds.local_ipv4) {
                used.push(cfg.mds.local_ipv4);
            }
        } catch (e) {}
    });
    return used;
}

// Collect all used Internal IPs (VM-to-VM) from all VMs
function getUsedInternalIps(excludeSmac) {
    var used = [];
    var vms = window._vmList || [];
    vms.forEach(function(vm) {
        if (vm.smac === excludeSmac) return;
        try {
            var cfg = typeof vm.config === 'string' ? JSON.parse(vm.config) : vm.config;
            if (cfg && cfg.mds && cfg.mds.internal_ip) {
                used.push(cfg.mds.internal_ip);
            }
        } catch (e) {}
    });
    return used;
}

// Generate a unique Internal IP: 192.168.100.{N} where N starts from 10
function genUniqueInternalIp(excludeSmac) {
    var used = getUsedInternalIps(excludeSmac);
    for (var n = 10; n <= 254; n++) {
        var ip = '192.168.100.' + n;
        if (used.indexOf(ip) === -1) return ip;
    }
    return '192.168.100.10';
}

// Generate a unique Local IPv4: 10.0.{N}.10 where N starts from 1
function genUniqueIpv4(excludeSmac) {
    var used = getUsedIpv4s(excludeSmac);
    var n = 1;
    while (n < 255) {
        var ip = '10.0.' + n + '.10';
        if (used.indexOf(ip) === -1) return ip;
        n++;
    }
    return '10.0.1.10';
}

// ======== SSH Key Management ========

async function loadSshKeyList() {
    try {
        var response = await apiFetch('/api/sshkey/list');
        var keys = await safeJson(response);
        window._sshKeyList = keys;
        var select = document.getElementById('mds-ssh-key-select');
        if (!select) return;
        var currentVal = select.value;
        select.innerHTML = '<option value="">-- paste or select saved key --</option>';
        keys.forEach(function(k) {
            var opt = document.createElement('option');
            opt.value = k.id;
            opt.textContent = k.name;
            opt.dataset.pubkey = k.pubkey;
            select.appendChild(opt);
        });
        if (currentVal) select.value = currentVal;
    } catch (err) {
        console.error('Failed to load SSH keys:', err);
    }
}

function onSshKeySelect() {
    var select = document.getElementById('mds-ssh-key-select');
    var input = document.getElementById('mds-ssh-pubkey');
    if (!select || !input) return;
    var opt = select.options[select.selectedIndex];
    if (opt && opt.dataset.pubkey) {
        input.value = opt.dataset.pubkey;
    }
}

function matchSshKeyByPubkey(pubkey) {
    if (!pubkey || !window._sshKeyList) return '';
    for (var i = 0; i < window._sshKeyList.length; i++) {
        if (window._sshKeyList[i].pubkey.trim() === pubkey.trim()) {
            return String(window._sshKeyList[i].id);
        }
    }
    return '';
}

async function saveSshKey() {
    var pubkey = val('mds-ssh-pubkey').trim();
    if (!pubkey) { alert('Please enter an SSH Public Key first'); return; }
    var name = prompt('Enter a name for this SSH key:');
    if (!name || !name.trim()) return;
    var ok = await apiCall('sshkey/create', { name: name.trim(), pubkey: pubkey });
    if (ok) {
        await loadSshKeyList();
        // Auto-select the newly saved key
        var matched = matchSshKeyByPubkey(pubkey);
        if (matched) document.getElementById('mds-ssh-key-select').value = matched;
    }
}

async function deleteSshKey() {
    var select = document.getElementById('mds-ssh-key-select');
    if (!select || !select.value) { alert('Please select an SSH key to delete'); return; }
    var name = select.options[select.selectedIndex].textContent;
    if (!confirm('Delete SSH key "' + name + '"?')) return;
    var ok = await apiCall('sshkey/delete', { id: parseInt(select.value) });
    if (ok) {
        document.getElementById('mds-ssh-pubkey').value = '';
        await loadSshKeyList();
    }
}

async function loadMdsConfig() {
    var smac = val('metadata-smac');
    if (!smac) return;
    var statusEl = document.getElementById('status-indicator');
    var outputEl = document.getElementById('output');
    statusEl.className = 'loading';
    statusEl.textContent = 'Loading MDS config for ' + smac + '...';
    try {
        var response = await apiFetch('/api/vm/' + encodeURIComponent(smac) + '/mds');
        var data = await safeJson(response);
        if (data.success && data.output) {
            var config = JSON.parse(data.output);
            // Auto-generate unique IDs if empty or still default placeholder
            var iid = config.instance_id;
            document.getElementById('mds-instance-id').value = (!iid || iid === 'i-0000000000000001') ? genInstanceId() : iid;
            var hp = config.hostname_prefix;
            document.getElementById('mds-hostname-prefix').value = (!hp || hp === 'vm') ? smac : hp;
            // Auto-generate unique IPv4 if empty or default
            var ipv4 = config.local_ipv4;
            document.getElementById('mds-local-ipv4').value = (!ipv4 || ipv4 === '10.0.0.1') ? genUniqueIpv4(smac) : ipv4;
            // Auto-generate unique Internal IP if empty
            var intIp = config.internal_ip;
            document.getElementById('mds-internal-ip').value = (!intIp) ? genUniqueInternalIp(smac) : intIp;
            document.getElementById('mds-vlan').value = config.vlan || '0';
            // Default MAC: pull from VM's Network Adapter 0
            var vmMac = '';
            var vms = window._vmList || [];
            for (var i = 0; i < vms.length; i++) {
                if (vms[i].smac === smac) {
                    try {
                        var vmCfg = typeof vms[i].config === 'string' ? JSON.parse(vms[i].config) : vms[i].config;
                        var adapters = vmCfg.network_adapters || [];
                        if (adapters.length > 0) vmMac = adapters[0].mac || '';
                    } catch (e) {}
                    break;
                }
            }
            var defMac = config.default_mac;
            document.getElementById('mds-default-mac').value = vmMac || defMac || '';
            document.getElementById('mds-ssh-pubkey').value = config.ssh_pubkey || '';
            // Auto-match SSH key from saved keys dropdown
            var matchedKey = matchSshKeyByPubkey(config.ssh_pubkey);
            var keySelect = document.getElementById('mds-ssh-key-select');
            if (keySelect) keySelect.value = matchedKey;
            // Don't show saved password — leave field empty (enter new to change)
            document.getElementById('mds-root-password').value = '';
            document.getElementById('mds-userdata-extra').value = config.userdata_extra || '';
            statusEl.className = 'success';
            statusEl.textContent = 'MDS config loaded for ' + smac;
            outputEl.textContent = data.output;
        } else {
            statusEl.className = 'error';
            statusEl.textContent = 'Error: ' + data.message;
        }
    } catch (err) {
        statusEl.className = 'error';
        statusEl.textContent = 'Error: ' + err.message;
    }
}

// MDS Config - Save (per-VM)
async function saveMdsConfig() {
    var smac = val('metadata-smac');
    if (!smac) {
        alert('Please select a VM first');
        return;
    }
    // Validate root password minimum length (if provided)
    var pw = val('mds-root-password');
    if (pw && pw.length < 6) {
        alert('Root Password must be at least 6 characters.');
        return;
    }
    // Validate unique IPv4
    var newIp = val('mds-local-ipv4');
    var usedIps = getUsedIpv4s(smac);
    if (newIp && usedIps.indexOf(newIp) !== -1) {
        alert('Local IPv4 "' + newIp + '" is already used by another VM. Please choose a different IP.');
        return;
    }
    var statusEl = document.getElementById('status-indicator');
    var outputEl = document.getElementById('output');
    statusEl.className = 'loading';
    statusEl.textContent = 'Saving MDS config for ' + smac + '...';
    var payload = {
        instance_id: val('mds-instance-id'),
        ami_id: '',
        hostname_prefix: val('mds-hostname-prefix'),
        local_ipv4: val('mds-local-ipv4'),
        internal_ip: val('mds-internal-ip'),
        vlan: val('mds-vlan') || '0',
        ssh_pubkey: val('mds-ssh-pubkey'),
        root_password: val('mds-root-password'),
        userdata_extra: document.getElementById('mds-userdata-extra').value,
        default_mac: val('mds-default-mac'),
        kea_socket_path: '',
    };
    try {
        var response = await apiFetch('/api/vm/' + encodeURIComponent(smac) + '/mds', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(payload),
        });
        var data = await safeJson(response);
        if (data.success) {
            statusEl.className = 'success';
            statusEl.textContent = data.message;
            // Clear root password after save (use-once)
            document.getElementById('mds-root-password').value = '';
            outputEl.textContent = JSON.stringify(payload, null, 2);
        } else {
            statusEl.className = 'error';
            statusEl.textContent = 'Error: ' + data.message;
        }
    } catch (err) {
        statusEl.className = 'error';
        statusEl.textContent = 'Error: ' + err.message;
    }
}

// Load VM list from database and populate all SMAC dropdowns
async function loadVmList() {
    try {
        var response = await apiFetch('/api/vm/list');
        var vms = await safeJson(response);
        window._vmList = vms;
        var selects = [
            'listimage-smac', 'mountiso-smac',
            'livemigrate-smac', 'backup-smac',
            'metadata-smac'
        ];
        selects.forEach(function(id) {
            var el = document.getElementById(id);
            if (!el) return;
            var current = el.value;
            el.innerHTML = '<option value="">-- select VM --</option>';
            vms.forEach(function(vm) {
                var opt = document.createElement('option');
                opt.value = vm.smac;
                opt.textContent = vm.smac;
                el.appendChild(opt);
            });
            if (current) el.value = current;
        });
    } catch (err) {
        console.error('Failed to load VM list:', err);
    }
}

// ======== VM List Table ========

// Load and render VM list table
async function loadVmListTable() {
    var tbody = document.getElementById('vm-list-body');
    try {
        var response = await apiFetch('/api/vm/list');
        var vms = await safeJson(response);
        window._vmList = vms;
        window._vmListData = vms; // Cache for VNC port lookup

        if (vms.length === 0) {
            tbody.innerHTML = '<tr><td colspan="7"><em>No VMs created yet. Go to Create VM tab to add one.</em></td></tr>';
            return;
        }

        // Group VMs by group_name
        var grouped = {};
        var groupOrder = [];
        vms.forEach(function(vm) {
            var g = vm.group_name || '';
            if (!grouped[g]) {
                grouped[g] = [];
                groupOrder.push(g);
            }
            grouped[g].push(vm);
        });

        var html = '';
        groupOrder.forEach(function(groupName) {
            var groupLabel = groupName || '(Ungrouped)';
            var groupVms = grouped[groupName];
            // Group header row
            html += '<tr class="group-header-row">' +
                '<td colspan="7" style="background:#161b22;padding:8px 10px;font-weight:bold;color:#58a6ff;border-bottom:2px solid #30363d;font-size:0.9rem;">' +
                '📁 ' + escapeHtml(groupLabel) + ' <small style="color:#8b949e;font-weight:normal;">(' + groupVms.length + ')</small>' +
                '</td></tr>';

            groupVms.forEach(function(vm) {
                var config = {};
                try { config = JSON.parse(vm.config); } catch(e) {}
                var cpuText = config.cpu ? (config.cpu.vcpus && config.cpu.vcpus !== '0' ? escapeHtml(config.cpu.vcpus) + ' vCPU' : escapeHtml(config.cpu.cores || '1') + 'c/' + escapeHtml(config.cpu.threads || '1') + 't') : '-';
                var memText = config.memory ? escapeHtml(config.memory.size) + 'MB' : '-';
                var isStopped = vm.status !== 'running';
                var diskText = (config.disks && config.disks.length > 0) ? config.disks.map(function(d) {
                    var dname = d.diskname || '-';
                    var safeDname = dname.replace(/'/g, "\\'");
                    if (!isStopped) return escapeHtml(dname);
                    return escapeHtml(dname) +
                        ' <span class="export-dropdown">' +
                        '<button class="btn-export btn-export-sm" onclick="event.stopPropagation();this.parentElement.classList.toggle(\'open\')" title="Export">⬇</button>' +
                        '<span class="export-dropdown-content">' +
                        '<a href="javascript:void(0)" onclick="exportDisk(\'' + safeDname + '\', \'qcow2\')">⬇ qcow2</a>' +
                        '<a href="javascript:void(0)" onclick="exportDisk(\'' + safeDname + '\', \'vmdk\')">⬇ VMDK</a>' +
                        '<a href="javascript:void(0)" onclick="exportDisk(\'' + safeDname + '\', \'vdi\')">⬇ VDI</a>' +
                        '<a href="javascript:void(0)" onclick="exportDisk(\'' + safeDname + '\', \'vhdx\')">⬇ VHDX</a>' +
                        '<a href="javascript:void(0)" onclick="exportDisk(\'' + safeDname + '\', \'raw\')">⬇ Raw</a>' +
                        '</span></span>';
                }).join(', ') : '-';
                var vncPort = config.vnc_port || '-';
                var statusClass = vm.status === 'running' ? 'status-running' : 'status-stopped';
                var statusText = vm.status || 'stopped';

                // Group cell — click to show dropdown
                var groupCell = '<span class="group-label" onclick="this.innerHTML=showGroupDropdown(\'' + escapeHtml(vm.smac).replace(/'/g, "\\'") + '\', \'' + escapeHtml(vm.group_name || '').replace(/'/g, "\\'") + '\')" style="cursor:pointer;color:#8b949e;font-size:0.85rem;" title="Click to change group">' +
                    escapeHtml(vm.group_name || '(Ungrouped)') + '</span>';

                var actions = '';
                if (vm.status === 'running') {
                    actions += '<button class="btn-vm-action btn-vm-stop" onclick="vmAction(\'stop\',\'' + vm.smac + '\')">Stop</button> ';
                    actions += '<button class="btn-vm-action btn-vm-reset" onclick="vmAction(\'reset\',\'' + vm.smac + '\')">Reset</button> ';
                    actions += '<button class="btn-vm-action btn-vm-powerdown" onclick="vmAction(\'powerdown\',\'' + vm.smac + '\')">Powerdown</button> ';
                    if (window._vncActive[vm.smac]) {
                        actions += '<button class="btn-vm-action btn-vm-vncstop" onclick="vmVncStop(\'' + vm.smac + '\')">VNC Stop</button> ';
                    } else {
                        actions += '<button class="btn-vm-action btn-vm-vnc" onclick="vmVncStart(\'' + vm.smac + '\')">VNC</button> ';
                    }
                } else {
                    // VM stopped → clear VNC active state
                    delete window._vncActive[vm.smac];
                    actions += '<button class="btn-vm-action btn-vm-start" onclick="vmAction(\'start\',\'' + vm.smac + '\')">Start</button> ';
                }
                actions += '<button class="btn-vm-action btn-vm-edit" onclick="editVm(\'' + vm.smac + '\')">Edit</button> ';
                actions += '<button class="btn-vm-action btn-vm-delete" onclick="deleteVmFromList(\'' + vm.smac + '\')">Delete</button>';

                var nameCell = vm.status === 'running'
                    ? '<a href="javascript:void(0)" class="vm-name-link" onclick="vmVncStart(\'' + vm.smac.replace(/'/g, "\\'") + '\')">' + escapeHtml(vm.smac) + '</a>'
                    : escapeHtml(vm.smac);

                html += '<tr>' +
                    '<td>' + nameCell + '</td>' +
                    '<td>' + groupCell + '</td>' +
                    '<td>' + cpuText + '</td>' +
                    '<td>' + memText + '</td>' +
                    '<td>' + diskText + '</td>' +
                    '<td><small style="color:#58a6ff;">:' + escapeHtml(vncPort) + '</small> <span class="' + statusClass + '">' + escapeHtml(statusText) + '</span></td>' +
                    '<td>' + actions + '</td>' +
                    '</tr>';
            });
        });

        tbody.innerHTML = html;
    } catch (err) {
        tbody.innerHTML = '<tr><td colspan="7">Error loading VM list</td></tr>';
    }
}

// Start/Stop VM from list
async function vmAction(action, smac) {
    var ok = await apiCall(action, { smac: smac });
    if (ok) {
        loadVmListTable();
        loadVmList();
    }
}

// Edit VM — load config into Create form
async function editVm(smac) {
    try {
        var response = await apiFetch('/api/vm/get/' + encodeURIComponent(smac));
        var vm = await safeJson(response);
        if (vm.smac) {
            window._editingVm = smac;
            // Switch to create tab
            switchTab('create');
            // Reset template to custom when editing
            document.getElementById('create-os-template').value = 'custom';
            // Fill form
            document.getElementById('create-vm-name').value = vm.smac;
            document.getElementById('create-vm-name').disabled = true;
            document.getElementById('create-title').textContent = 'Edit VM: ' + vm.smac;
            document.getElementById('create-submit-btn').textContent = 'Save Changes';
            document.getElementById('create-submit-btn').setAttribute('onclick', 'executeUpdateVm()');

            // Fill group
            await loadGroupList();
            var groupSel = document.getElementById('create-group');
            var groupName = vm.group_name || '';
            // Ensure group option exists
            if (groupName && !Array.from(groupSel.options).some(function(o) { return o.value === groupName; })) {
                var opt = document.createElement('option');
                opt.value = groupName;
                opt.textContent = groupName;
                groupSel.appendChild(opt);
            }
            groupSel.value = groupName;
            document.getElementById('create-group-new').value = '';

            var config = {};
            try { config = JSON.parse(vm.config); } catch(e) {}

            // Fill CPU/Memory/Features
            if (config.cpu) {
                // Backward compat: compute vcpus from sockets*cores*threads if vcpus not set
                var vcpus = config.cpu.vcpus;
                if (!vcpus || vcpus === '0') {
                    var s = parseInt(config.cpu.sockets || '1');
                    var c = parseInt(config.cpu.cores || '1');
                    var t = parseInt(config.cpu.threads || '1');
                    vcpus = String(s * c * t);
                }
                document.getElementById('start-vcpus').value = vcpus;
            }
            if (config.memory) {
                document.getElementById('start-memory-size').value = config.memory.size || '2048';
            }
            if (config.features) {
                document.getElementById('start-is-windows').value = config.features.is_windows || '0';
                document.getElementById('start-arch').value = config.features.arch || 'x86_64';
                document.getElementById('start-cloudinit').value = config.features.cloudinit || '1';
            }

            // Fill Network Adapters
            var adapterContainer = document.getElementById('start-network-adapters');
            adapterContainer.innerHTML = '';
            if (config.network_adapters && config.network_adapters.length > 0) {
                config.network_adapters.forEach(function(adapter) {
                    addNetworkAdapter(adapter);
                });
            } else {
                addNetworkAdapter();
            }

            // Fill Disks
            var diskContainer = document.getElementById('start-disks');
            diskContainer.innerHTML = '';
            if (config.disks && config.disks.length > 0) {
                config.disks.forEach(function(disk) {
                    var iTotal = disk['iops-total'] || '9600';
                    var iMax = disk['iops-total-max'] || '11520';
                    var iLen = disk['iops-total-max-length'] || '60';
                    var presetKey = matchIopsPreset(iTotal, iMax, iLen);
                    var customDisplay = presetKey === 'custom' ? '' : 'display:none;';
                    var row = document.createElement('div');
                    row.className = 'disk-row';
                    row.innerHTML =
                        '<input class="disk-diskid" placeholder="Disk ID" value="' + (disk.diskid || '0') + '" readonly style="opacity:0.6;cursor:default;">' +
                        '<select class="disk-diskname"><option value="">-- select disk --</option></select>' +
                        '<select class="disk-iops-preset" onchange="onIopsPresetChange(this)">' + buildIopsSelect(presetKey) + '</select>' +
                        '<span class="disk-iops-custom" style="' + customDisplay + '">' +
                        '<input class="disk-iops-total" placeholder="IOPS" value="' + iTotal + '" style="width:70px;">' +
                        '<input class="disk-iops-total-max" placeholder="Max" value="' + iMax + '" style="width:70px;">' +
                        '<input class="disk-iops-total-max-length" placeholder="Len" value="' + iLen + '" style="width:50px;">' +
                        '</span>' +
                        '<button type="button" class="btn-remove" onclick="this.parentElement.remove()">X</button>';
                    diskContainer.appendChild(row);
                    populateDiskSelect(row.querySelector('.disk-diskname'), disk.diskname || '');
                });
            } else {
                addDisk();
            }

            // Fill PCI Devices
            var pciContainer = document.getElementById('start-pci-devices');
            pciContainer.innerHTML = '';
            if (config.pci_devices && config.pci_devices.length > 0) {
                config.pci_devices.forEach(function(pci) {
                    addPciDevice(pci);
                });
            }
        }
    } catch (err) {
        alert('Failed to load VM config: ' + err.message);
    }
}

// Delete VM from list
async function deleteVmFromList(smac) {
    if (!confirm('Delete VM "' + smac + '"? This will also delete the disk file.')) return;
    var ok = await apiCall('delete', { smac: smac });
    if (ok) {
        loadVmListTable();
        loadVmList();
    }
}

// ======== Group Management ========

// Load group list for dropdowns
async function loadGroupList() {
    try {
        var response = await apiFetch('/api/group/list');
        var groups = await safeJson(response);
        window._groupList = groups || [];
        // Populate create form group dropdown
        var sel = document.getElementById('create-group');
        if (sel) {
            var current = sel.value;
            sel.innerHTML = '<option value="">(Ungrouped)</option>';
            groups.forEach(function(g) {
                var opt = document.createElement('option');
                opt.value = g;
                opt.textContent = g;
                sel.appendChild(opt);
            });
            if (current) sel.value = current;
        }
    } catch (err) {
        console.error('Failed to load group list:', err);
    }
}

// Set VM group via API
async function setVmGroup(smac, groupName) {
    try {
        var response = await apiFetch('/api/vm/set-group', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ smac: smac, group_name: groupName }),
        });
        var data = await safeJson(response);
        if (data.success) {
            loadVmListTable();
            loadGroupList();
        }
    } catch (err) {
        console.error('Failed to set group:', err);
    }
}

// Show inline group change dropdown
function showGroupDropdown(smac, currentGroup) {
    var groups = window._groupList || [];
    var options = '<option value="">(Ungrouped)</option>';
    groups.forEach(function(g) {
        options += '<option value="' + escapeHtml(g) + '"' + (g === currentGroup ? ' selected' : '') + '>' + escapeHtml(g) + '</option>';
    });
    options += '<option value="__new__">+ New Group...</option>';
    var selectHtml = '<select onchange="onGroupChange(this, \'' + escapeHtml(smac).replace(/'/g, "\\'") + '\')" style="font-size:0.85rem;padding:2px 4px;">' + options + '</select>';
    return selectHtml;
}

function onGroupChange(selectEl, smac) {
    var val = selectEl.value;
    if (val === '__new__') {
        var newGroup = prompt('Enter new group name:');
        if (newGroup && newGroup.trim()) {
            setVmGroup(smac, newGroup.trim());
        } else {
            loadVmListTable(); // revert
        }
    } else {
        setVmGroup(smac, val);
    }
}

// Get selected group from Create form (dropdown or new input)
function getCreateFormGroup() {
    var newGroup = (document.getElementById('create-group-new').value || '').trim();
    if (newGroup) return newGroup;
    return document.getElementById('create-group').value || '';
}

// ──────────────────────────────────────────
// DHCP Table
// ──────────────────────────────────────────

async function loadDhcpTable() {
    var tbody = document.getElementById('dhcp-table-body');
    if (!tbody) return;
    try {
        var response = await apiFetch('/api/dhcp/list');
        var leases = await safeJson(response);
        if (leases.length === 0) {
            tbody.innerHTML = '<tr><td colspan="6"><em>No DHCP leases. Click "Sync from VMs" or add manually.</em></td></tr>';
            return;
        }
        var html = '';
        leases.forEach(function(l) {
            var sourceLabel = l.source === 'static'
                ? '<span style="color:#3fb950;">static</span>'
                : '<span style="color:#8b949e;">vm-config</span>';
            var actions = '';
            if (l.source === 'static') {
                actions = '<button class="btn-vm-action btn-vm-delete" onclick="deleteDhcpLease(\'' + escapeHtml(l.mac).replace(/'/g, "\\'") + '\')">Delete</button>';
            } else {
                actions = '<button class="btn-vm-action btn-vm-edit" onclick="promoteDhcpLease(\'' + escapeHtml(l.mac).replace(/'/g, "\\'") + '\',\'' + escapeHtml(l.ip).replace(/'/g, "\\'") + '\',\'' + escapeHtml(l.hostname).replace(/'/g, "\\'") + '\',\'' + escapeHtml(l.vm_name).replace(/'/g, "\\'") + '\')">Save as Static</button>';
            }
            html += '<tr>' +
                '<td><code>' + escapeHtml(l.mac) + '</code></td>' +
                '<td>' + escapeHtml(l.ip || '-') + '</td>' +
                '<td>' + escapeHtml(l.hostname || '-') + '</td>' +
                '<td>' + escapeHtml(l.vm_name || '-') + '</td>' +
                '<td>' + sourceLabel + '</td>' +
                '<td>' + actions + '</td>' +
                '</tr>';
        });
        tbody.innerHTML = html;
    } catch (err) {
        tbody.innerHTML = '<tr><td colspan="6">Error loading DHCP table</td></tr>';
    }
}

async function addDhcpLease() {
    var mac = document.getElementById('dhcp-mac').value.trim();
    var ip = document.getElementById('dhcp-ip').value.trim();
    var hostname = document.getElementById('dhcp-hostname').value.trim();
    var vmName = document.getElementById('dhcp-vm-name').value.trim();
    if (!mac) { alert('MAC address is required'); return; }
    if (!ip) { alert('IP address is required'); return; }
    var statusEl = document.getElementById('status-indicator');
    try {
        var response = await apiFetch('/api/dhcp/add', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ mac: mac, ip: ip, hostname: hostname, vm_name: vmName })
        });
        var result = await safeJson(response);
        statusEl.className = result.success ? 'success' : 'error';
        statusEl.textContent = result.message || '';
        if (result.success) {
            document.getElementById('dhcp-mac').value = '';
            document.getElementById('dhcp-ip').value = '';
            document.getElementById('dhcp-hostname').value = '';
            document.getElementById('dhcp-vm-name').value = '';
            loadDhcpTable();
        }
    } catch (err) {
        statusEl.className = 'error';
        statusEl.textContent = 'Error: ' + err.message;
    }
}

async function deleteDhcpLease(mac) {
    if (!confirm('Delete DHCP lease for ' + mac + '?')) return;
    var statusEl = document.getElementById('status-indicator');
    try {
        var response = await apiFetch('/api/dhcp/delete', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ mac: mac })
        });
        var result = await safeJson(response);
        statusEl.className = result.success ? 'success' : 'error';
        statusEl.textContent = result.message || '';
        loadDhcpTable();
    } catch (err) {
        statusEl.className = 'error';
        statusEl.textContent = 'Error: ' + err.message;
    }
}

function promoteDhcpLease(mac, ip, hostname, vmName) {
    document.getElementById('dhcp-mac').value = mac;
    document.getElementById('dhcp-ip').value = ip;
    document.getElementById('dhcp-hostname').value = hostname;
    document.getElementById('dhcp-vm-name').value = vmName;
}

async function syncDhcpFromVms() {
    var statusEl = document.getElementById('status-indicator');
    statusEl.className = 'loading';
    statusEl.textContent = 'Syncing DHCP leases from VM configs...';
    try {
        var response = await apiFetch('/api/dhcp/sync', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: '{}'
        });
        var result = await safeJson(response);
        statusEl.className = result.success ? 'success' : 'error';
        statusEl.textContent = result.message || '';
        loadDhcpTable();
    } catch (err) {
        statusEl.className = 'error';
        statusEl.textContent = 'Error: ' + err.message;
    }
}

// Init
window.addEventListener('DOMContentLoaded', function() {
    loadUsedMacs().then(function() {
        addNetworkAdapter(); // genMac() now avoids collisions
    });
    // Load disk list first, then add default disk row (so dropdown is populated)
    loadDiskList().then(function() {
        addDisk();
    });
    loadVmList();
    loadIsoList();
    loadVmListTable();
    loadSwitchList();
    loadGroupList();
    loadImageMappings();
    loadOsTemplates();
    loadVfioDevices();
    loadApikey();
});
