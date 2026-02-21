// OS Templates — image: default disk name pattern for auto-match
var OS_TEMPLATES = {
    'custom':          null,
    'ubuntu-server':   { sockets: '1', cores: '2', threads: '1', memory: '2048', is_windows: '0', image: 'ubuntu-server' },
    'ubuntu-desktop':  { sockets: '1', cores: '4', threads: '1', memory: '4096', is_windows: '0', image: 'ubuntu-desktop' },
    'debian':          { sockets: '1', cores: '2', threads: '1', memory: '1024', is_windows: '0', image: 'debian' },
    'centos-rocky':    { sockets: '1', cores: '2', threads: '1', memory: '2048', is_windows: '0', image: 'centos' },
    'windows-desktop': { sockets: '1', cores: '4', threads: '2', memory: '4096', is_windows: '1', image: 'windows-10' },
    'windows-server':  { sockets: '1', cores: '4', threads: '2', memory: '8192', is_windows: '1', image: 'windows-server' },
    'macos':           { sockets: '1', cores: '4', threads: '2', memory: '8192', is_windows: '0', image: 'macos' },
    'minimal-linux':   { sockets: '1', cores: '1', threads: '1', memory: '512',  is_windows: '0', image: 'minimal' },
};

// Load saved template-to-image mappings from localStorage
function loadImageMappings() {
    try { return JSON.parse(localStorage.getItem('os_template_images') || '{}'); }
    catch(e) { return {}; }
}
function saveImageMapping(templateKey, diskName) {
    var map = loadImageMappings();
    if (diskName) { map[templateKey] = diskName; }
    else { delete map[templateKey]; }
    localStorage.setItem('os_template_images', JSON.stringify(map));
}

// Populate Base Image dropdown with all available disk images
function populateBaseImageSelect(selectedValue) {
    var sel = document.getElementById('create-base-image');
    if (!sel) return;
    var disks = window._diskList || [];
    var current = selectedValue || sel.value;
    sel.innerHTML = '<option value="">-- no image --</option>';
    disks.forEach(function(d) {
        var opt = document.createElement('option');
        opt.value = d.name;
        var sizeInfo = d.disk_size || formatSize(d.size);
        var ownerInfo = d.owner ? ' [' + d.owner + ']' : '';
        opt.textContent = d.name + '.qcow2 (' + sizeInfo + ')' + ownerInfo;
        sel.appendChild(opt);
    });
    if (current) sel.value = current;
}

function applyOsTemplate() {
    var sel = document.getElementById('create-os-template');
    var tpl = OS_TEMPLATES[sel.value];
    var templateKey = sel.value;

    // Update template-info
    var infoDiv = document.getElementById('template-info');
    if (infoDiv) infoDiv.innerHTML = '';

    if (!tpl) {
        // Custom — clear base image selection
        document.getElementById('create-base-image').value = '';
        return;
    }

    // Fill CPU / Memory / Features
    document.getElementById('start-cpu-sockets').value = tpl.sockets;
    document.getElementById('start-cpu-cores').value = tpl.cores;
    document.getElementById('start-cpu-threads').value = tpl.threads;
    document.getElementById('start-memory-size').value = tpl.memory;
    document.getElementById('start-is-windows').value = tpl.is_windows;

    // Resolve base image: saved mapping → auto-match → none
    var savedMap = loadImageMappings();
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

    // Set base image dropdown
    var baseImgSel = document.getElementById('create-base-image');
    baseImgSel.value = imageName || '';

    // Auto-select disk in first disk row
    applyBaseImageToDisk(imageName);

    // Update info
    updateTemplateInfo(templateKey, imageName);
}

// When user manually changes Base Image dropdown
function onBaseImageChange() {
    var templateKey = document.getElementById('create-os-template').value;
    var imageName = document.getElementById('create-base-image').value;

    // Save mapping for this template
    if (templateKey && templateKey !== 'custom') {
        saveImageMapping(templateKey, imageName);
    }

    // Apply to first disk row
    applyBaseImageToDisk(imageName);

    // Update info
    updateTemplateInfo(templateKey, imageName);
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

// Show pairing status
function updateTemplateInfo(templateKey, imageName) {
    var infoDiv = document.getElementById('template-info');
    if (!infoDiv) return;
    if (!templateKey || templateKey === 'custom') { infoDiv.innerHTML = ''; return; }
    var tpl = OS_TEMPLATES[templateKey];
    if (!tpl) { infoDiv.innerHTML = ''; return; }

    var savedMap = loadImageMappings();
    var isSaved = savedMap[templateKey] === imageName;

    if (imageName) {
        infoDiv.innerHTML = '<span style="color:#3fb950;">Base image: <b>' + imageName + '.qcow2</b></span>' +
            (isSaved ? ' <small style="color:#8b949e;">(saved)</small>' : '');
    } else {
        infoDiv.innerHTML = '<span style="color:#d29922;">No image paired. Select a <b>Base Image</b> or upload one in ' +
            '<a href="#" onclick="switchTab(\'listimage\');return false;" style="color:#58a6ff;">List Image</a> tab.</span>';
    }
}

// Tab switching
document.querySelectorAll('.tab').forEach(function(tab) {
    tab.addEventListener('click', function() {
        document.querySelectorAll('.tab').forEach(function(t) { t.classList.remove('active'); });
        document.querySelectorAll('.tab-panel').forEach(function(p) { p.classList.remove('active'); });
        tab.classList.add('active');
        document.getElementById('tab-' + tab.dataset.tab).classList.add('active');
        // Auto-load MDS config when switching to metadata tab
        if (tab.dataset.tab === 'metadata') { loadMdsConfig(); }
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
        var apiPath = (operation.startsWith('vnc/') || operation.startsWith('disk/') || operation.startsWith('iso/') || operation.startsWith('backup/')) ? '/api/' + operation : '/api/vm/' + operation;
        console.log('apiCall:', operation, '->', apiPath);
        var response = await fetch(apiPath, {
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
        var response = await fetch('/api/backup/list');
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
                '<td style="padding:6px 8px;">' + b.vm_name + '</td>' +
                '<td style="padding:6px 8px;">' + (b.datetime || '<em>unknown</em>') + '</td>' +
                '<td style="padding:6px 8px;text-align:right;">' + formatSize(b.size) + '</td>' +
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
        };
    });

    var diskRows = document.querySelectorAll('#start-disks .disk-row');
    var disks = Array.from(diskRows).map(function(row) {
        return {
            diskid: row.querySelector('.disk-diskid').value,
            diskname: row.querySelector('.disk-diskname').value,
            'iops-total': row.querySelector('.disk-iops-total').value,
            'iops-total-max': row.querySelector('.disk-iops-total-max').value,
            'iops-total-max-length': row.querySelector('.disk-iops-total-max-length').value,
        };
    });

    return {
        cpu: {
            sockets: val('start-cpu-sockets'),
            cores: val('start-cpu-cores'),
            threads: val('start-cpu-threads'),
        },
        memory: { size: val('start-memory-size') },
        features: { is_windows: val('start-is-windows') },
        network_adapters: network_adapters,
        disks: disks,
    };
}

// Create VM — save config to DB + create disk
async function executeCreateVm() {
    var vmName = val('create-vm-name').trim();
    if (!vmName) {
        alert('Please enter a VM-NAME');
        return;
    }
    var config = collectVmConfig();
    // Set first disk's diskname to vmName if empty
    if (config.disks.length > 0 && !config.disks[0].diskname) {
        config.disks[0].diskname = vmName;
    }
    var ok = await apiCall('create-config', {
        smac: vmName,
        config: config,
    });
    if (ok) {
        loadVmList();
        loadVmListTable();
        // Reset edit mode
        window._editingVm = null;
        document.getElementById('create-title').textContent = 'Create VM';
        document.getElementById('create-submit-btn').textContent = 'Create VM';
        document.getElementById('create-submit-btn').setAttribute('onclick', 'executeCreateVm()');
        document.getElementById('create-vm-name').disabled = false;
        document.getElementById('create-os-template').value = 'custom';
        document.getElementById('create-base-image').value = '';
        document.getElementById('template-info').innerHTML = '';
    }
}

// Update VM config (edit mode)
async function executeUpdateVm() {
    var vmName = val('create-vm-name').trim();
    if (!vmName) {
        alert('Please enter a VM-NAME');
        return;
    }
    var config = collectVmConfig();
    if (config.disks.length > 0 && !config.disks[0].diskname) {
        config.disks[0].diskname = vmName;
    }
    var ok = await apiCall('update-config', {
        smac: vmName,
        config: config,
    });
    if (ok) {
        loadVmList();
        loadVmListTable();
        // Reset edit mode
        window._editingVm = null;
        document.getElementById('create-title').textContent = 'Create VM';
        document.getElementById('create-submit-btn').textContent = 'Create VM';
        document.getElementById('create-submit-btn').setAttribute('onclick', 'executeCreateVm()');
        document.getElementById('create-vm-name').disabled = false;
        document.getElementById('create-os-template').value = 'custom';
        document.getElementById('create-base-image').value = '';
        document.getElementById('template-info').innerHTML = '';
    }
}

// Generate random MAC address (52:54:00:xx:xx:xx)
function genMac() {
    var hex = '0123456789abcdef';
    var mac = '52:54:00';
    for (var i = 0; i < 3; i++) {
        mac += ':' + hex[Math.floor(Math.random() * 16)] + hex[Math.floor(Math.random() * 16)];
    }
    return mac;
}

// Dynamic rows - Network Adapter
function addNetworkAdapter() {
    var container = document.getElementById('start-network-adapters');
    var count = container.querySelectorAll('.adapter-row').length;
    var row = document.createElement('div');
    row.className = 'adapter-row';
    row.innerHTML =
        '<input class="adapter-netid" placeholder="Net ID" value="' + count + '">' +
        '<input class="adapter-mac" placeholder="MAC" value="' + genMac() + '">' +
        '<input class="adapter-vlan" placeholder="VLAN" value="0">' +
        '<button type="button" class="btn-remove" onclick="this.parentElement.remove()">X</button>';
    container.appendChild(row);
}

// Dynamic rows - Disk (with dropdown for disk name)
function addDisk(selectedValue) {
    var container = document.getElementById('start-disks');
    var count = container.querySelectorAll('.disk-row').length;
    var row = document.createElement('div');
    row.className = 'disk-row';
    row.innerHTML =
        '<input class="disk-diskid" placeholder="Disk ID" value="' + count + '">' +
        '<select class="disk-diskname"><option value="">-- select disk --</option></select>' +
        '<input class="disk-iops-total" placeholder="IOPS Total" value="9600">' +
        '<input class="disk-iops-total-max" placeholder="IOPS Max" value="11520">' +
        '<input class="disk-iops-total-max-length" placeholder="Max Length" value="60">' +
        '<button type="button" class="btn-remove" onclick="this.parentElement.remove()">X</button>';
    container.appendChild(row);
    populateDiskSelect(row.querySelector('.disk-diskname'), selectedValue || '');
}

// ======== Disk Management ========

// Load disk list from API and cache it
async function loadDiskList() {
    try {
        var response = await fetch('/api/disk/list');
        var disks = await safeJson(response);
        window._diskList = disks;
        // Render file list in Create Disk tab
        var listDiv = document.getElementById('disk-file-list');
        if (!listDiv) return;
        if (disks.length === 0) {
            listDiv.innerHTML = '<em>No disk files</em>';
        } else {
            listDiv.innerHTML = disks.map(function(d) {
                var ownerText = d.owner ? ' <small style="color:#58a6ff;">[' + d.owner + ']</small>' : ' <small style="color:#3fb950;">[free]</small>';
                var cloneBtn = '<button class="btn-clone" onclick="cloneDisk(\'' + d.name.replace(/'/g, "\\'") + '\')">Clone</button>';
                var deleteBtn = d.owner ? '' : '<button class="btn-remove" onclick="deleteDisk(\'' + d.name.replace(/'/g, "\\'") + '\')">X</button>';
                var sizeInfo = d.disk_size ? d.disk_size : formatSize(d.size);
                return '<div style="display:flex;justify-content:space-between;align-items:center;padding:4px 0;border-bottom:1px solid #333;">' +
                    '<span>' + d.name + '.qcow2 <small>(' + sizeInfo + ')</small>' + ownerText + '</span>' +
                    '<span>' + cloneBtn + ' ' + deleteBtn + '</span>' +
                    '</div>';
            }).join('');
        }
        // Refresh any disk selects on the page
        refreshAllDiskSelects();
        // Also refresh Base Image dropdown
        populateBaseImageSelect();
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

// Delete a disk file
async function deleteDisk(name) {
    if (!confirm('Delete disk: ' + name + '.qcow2?')) return;
    var statusEl = document.getElementById('status-indicator');
    statusEl.className = 'loading';
    statusEl.textContent = 'Deleting ' + name + '...';
    try {
        var response = await fetch('/api/disk/delete', {
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
        var response = await fetch('/api/disk/clone', {
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

// ======== Image Management ========

// Load image list
async function loadImageList() {
    try {
        var response = await fetch('/api/image/list');
        var images = await safeJson(response);
        var listDiv = document.getElementById('image-file-list');
        if (!listDiv) return;
        if (images.length === 0) {
            listDiv.innerHTML = '<em>No image files</em>';
        } else {
            listDiv.innerHTML = images.map(function(img) {
                return '<div style="display:flex;justify-content:space-between;align-items:center;padding:4px 0;border-bottom:1px solid #333;">' +
                    '<span>' + img.name + ' <small>(' + formatSize(img.size) + ')</small></span>' +
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
        var response = await fetch('/api/image/delete', {
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
function executeMountIso() {
    apiCall('mountiso', {
        smac: val('mountiso-smac'),
        isoname: val('mountiso-isoname'),
    });
}

// Unmount ISO
function executeUnmountIso() {
    apiCall('unmountiso', {
        smac: val('mountiso-smac'),
    });
}

// Load ISO list and populate dropdown + file list
async function loadIsoList() {
    try {
        var response = await fetch('/api/iso/list');
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
                    '<span>' + iso.name + ' <small>(' + formatSize(iso.size) + ')</small></span>' +
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
        var response = await fetch('/api/iso/delete', {
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
    var ok = await apiCall('vnc/start', { smac: smac, novncport: String(port) });
    if (ok) {
        window._vncActive[smac] = true;
        loadVmListTable();
        window.open('/vnc.html?port=' + port + '&smac=' + smac, '_blank');
    }
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

async function loadMdsConfig() {
    var smac = val('metadata-smac');
    if (!smac) return;
    var statusEl = document.getElementById('status-indicator');
    var outputEl = document.getElementById('output');
    statusEl.className = 'loading';
    statusEl.textContent = 'Loading MDS config for ' + smac + '...';
    try {
        var response = await fetch('/api/vm/' + encodeURIComponent(smac) + '/mds');
        var data = await safeJson(response);
        if (data.success && data.output) {
            var config = JSON.parse(data.output);
            // Auto-generate unique IDs if empty or still default placeholder
            var iid = config.instance_id;
            document.getElementById('mds-instance-id').value = (!iid || iid === 'i-0000000000000001') ? genInstanceId() : iid;
            var aid = config.ami_id;
            document.getElementById('mds-ami-id').value = (!aid || aid === 'ami-00000001') ? genAmiId() : aid;
            var hp = config.hostname_prefix;
            document.getElementById('mds-hostname-prefix').value = (!hp || hp === 'vm') ? smac : hp;
            // Auto-generate unique IPv4 if empty or default
            var ipv4 = config.local_ipv4;
            document.getElementById('mds-local-ipv4').value = (!ipv4 || ipv4 === '10.0.0.1') ? genUniqueIpv4(smac) : ipv4;
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
        ami_id: val('mds-ami-id'),
        hostname_prefix: val('mds-hostname-prefix'),
        local_ipv4: val('mds-local-ipv4'),
        vlan: val('mds-vlan') || '0',
        ssh_pubkey: val('mds-ssh-pubkey'),
        root_password: val('mds-root-password'),
        userdata_extra: document.getElementById('mds-userdata-extra').value,
        default_mac: val('mds-default-mac'),
        kea_socket_path: '',
    };
    try {
        var response = await fetch('/api/vm/' + encodeURIComponent(smac) + '/mds', {
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
        var response = await fetch('/api/vm/list');
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
        var response = await fetch('/api/vm/list');
        var vms = await safeJson(response);
        window._vmList = vms;
        window._vmListData = vms; // Cache for VNC port lookup

        if (vms.length === 0) {
            tbody.innerHTML = '<tr><td colspan="6"><em>No VMs created yet. Go to Create VM tab to add one.</em></td></tr>';
            return;
        }

        tbody.innerHTML = vms.map(function(vm) {
            var config = {};
            try { config = JSON.parse(vm.config); } catch(e) {}
            var cpuText = config.cpu ? config.cpu.cores + 'c/' + config.cpu.threads + 't' : '-';
            var memText = config.memory ? config.memory.size + 'MB' : '-';
            var diskText = (config.disks && config.disks.length > 0) ? config.disks.map(function(d) { return d.diskname || '-'; }).join(', ') : '-';
            var vncPort = config.vnc_port || '-';
            var statusClass = vm.status === 'running' ? 'status-running' : 'status-stopped';
            var statusText = vm.status || 'stopped';

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
                ? '<a href="/vnc.html?port=' + vncPort + '&smac=' + vm.smac + '" class="vm-name-link" target="_blank">' + vm.smac + '</a>'
                : vm.smac;

            return '<tr>' +
                '<td>' + nameCell + '</td>' +
                '<td>' + cpuText + '</td>' +
                '<td>' + memText + '</td>' +
                '<td>' + diskText + '</td>' +
                '<td><small style="color:#58a6ff;">:' + vncPort + '</small> <span class="' + statusClass + '">' + statusText + '</span></td>' +
                '<td>' + actions + '</td>' +
                '</tr>';
        }).join('');
    } catch (err) {
        tbody.innerHTML = '<tr><td colspan="6">Error loading VM list</td></tr>';
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
        var response = await fetch('/api/vm/get/' + encodeURIComponent(smac));
        var vm = await safeJson(response);
        if (vm.smac) {
            window._editingVm = smac;
            // Switch to create tab
            switchTab('create');
            // Reset template to custom when editing
            document.getElementById('create-os-template').value = 'custom';
            document.getElementById('create-base-image').value = '';
            document.getElementById('template-info').innerHTML = '';
            // Fill form
            document.getElementById('create-vm-name').value = vm.smac;
            document.getElementById('create-vm-name').disabled = true;
            document.getElementById('create-title').textContent = 'Edit VM: ' + vm.smac;
            document.getElementById('create-submit-btn').textContent = 'Save Changes';
            document.getElementById('create-submit-btn').setAttribute('onclick', 'executeUpdateVm()');

            var config = {};
            try { config = JSON.parse(vm.config); } catch(e) {}

            // Fill CPU/Memory/Features
            if (config.cpu) {
                document.getElementById('start-cpu-sockets').value = config.cpu.sockets || '1';
                document.getElementById('start-cpu-cores').value = config.cpu.cores || '2';
                document.getElementById('start-cpu-threads').value = config.cpu.threads || '1';
            }
            if (config.memory) {
                document.getElementById('start-memory-size').value = config.memory.size || '2048';
            }
            if (config.features) {
                document.getElementById('start-is-windows').value = config.features.is_windows || '0';
            }

            // Fill Network Adapters
            var adapterContainer = document.getElementById('start-network-adapters');
            adapterContainer.innerHTML = '';
            if (config.network_adapters && config.network_adapters.length > 0) {
                config.network_adapters.forEach(function(adapter) {
                    var row = document.createElement('div');
                    row.className = 'adapter-row';
                    row.innerHTML =
                        '<input class="adapter-netid" placeholder="Net ID" value="' + (adapter.netid || '0') + '">' +
                        '<input class="adapter-mac" placeholder="MAC" value="' + (adapter.mac || '') + '">' +
                        '<input class="adapter-vlan" placeholder="VLAN" value="' + (adapter.vlan || '0') + '">' +
                        '<button type="button" class="btn-remove" onclick="this.parentElement.remove()">X</button>';
                    adapterContainer.appendChild(row);
                });
            } else {
                addNetworkAdapter();
            }

            // Fill Disks
            var diskContainer = document.getElementById('start-disks');
            diskContainer.innerHTML = '';
            if (config.disks && config.disks.length > 0) {
                config.disks.forEach(function(disk) {
                    var row = document.createElement('div');
                    row.className = 'disk-row';
                    row.innerHTML =
                        '<input class="disk-diskid" placeholder="Disk ID" value="' + (disk.diskid || '0') + '">' +
                        '<select class="disk-diskname"><option value="">-- select disk --</option></select>' +
                        '<input class="disk-iops-total" placeholder="IOPS Total" value="' + (disk['iops-total'] || '9600') + '">' +
                        '<input class="disk-iops-total-max" placeholder="IOPS Max" value="' + (disk['iops-total-max'] || '11520') + '">' +
                        '<input class="disk-iops-total-max-length" placeholder="Max Length" value="' + (disk['iops-total-max-length'] || '60') + '">' +
                        '<button type="button" class="btn-remove" onclick="this.parentElement.remove()">X</button>';
                    diskContainer.appendChild(row);
                    populateDiskSelect(row.querySelector('.disk-diskname'), disk.diskname || '');
                });
            } else {
                addDisk();
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

// Init
window.addEventListener('DOMContentLoaded', function() {
    addNetworkAdapter();
    // Load disk list first, then add default disk row (so dropdown is populated)
    loadDiskList().then(function() {
        addDisk();
    });
    loadVmList();
    loadIsoList();
    loadVmListTable();
});
