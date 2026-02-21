// Tab switching
document.querySelectorAll('.tab').forEach(function(tab) {
    tab.addEventListener('click', function() {
        document.querySelectorAll('.tab').forEach(function(t) { t.classList.remove('active'); });
        document.querySelectorAll('.tab-panel').forEach(function(p) { p.classList.remove('active'); });
        tab.classList.add('active');
        document.getElementById('tab-' + tab.dataset.tab).classList.add('active');
        // Auto-load MDS config when switching to metadata tab
        if (tab.dataset.tab === 'metadata') { loadMdsConfig(); }
    });
});

// API call helper
async function apiCall(operation, payload) {
    var statusEl = document.getElementById('status-indicator');
    var outputEl = document.getElementById('output');

    statusEl.className = 'loading';
    statusEl.textContent = 'Executing ' + operation + '...';
    outputEl.textContent = '';

    document.querySelectorAll('.execute-btn').forEach(function(b) { b.disabled = true; });

    try {
        var apiPath = operation.startsWith('vnc/') ? '/api/' + operation : '/api/vm/' + operation;
        var response = await fetch(apiPath, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(payload),
        });
        var data = await response.json();

        if (data.success) {
            statusEl.className = 'success';
            statusEl.textContent = data.message;
            outputEl.textContent = data.output || '(no output)';
        } else {
            statusEl.className = 'error';
            statusEl.textContent = 'Error: ' + data.message;
            outputEl.textContent = data.output || '';
        }
    } catch (err) {
        statusEl.className = 'error';
        statusEl.textContent = 'Network error: ' + err.message;
    } finally {
        document.querySelectorAll('.execute-btn').forEach(function(b) { b.disabled = false; });
    }
}

// Helper
function val(id) {
    return document.getElementById(id).value;
}

// SimpleCmd operations (smac only)
function executeSimple(operation) {
    apiCall(operation, {
        smac: val(operation + '-smac'),
    });
}

// Start VM
function executeStart() {
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

    apiCall('start', {
        cpu: {
            sockets: val('start-cpu-sockets'),
            cores: val('start-cpu-cores'),
            threads: val('start-cpu-threads'),
        },
        memory: { size: val('start-memory-size') },
        features: { is_windows: val('start-is-windows') },
        network_adapters: network_adapters,
        disks: disks,
    });
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

// Dynamic rows - Disk
function addDisk() {
    var container = document.getElementById('start-disks');
    var row = document.createElement('div');
    row.className = 'disk-row';
    row.innerHTML =
        '<input class="disk-diskid" placeholder="Disk ID" value="0">' +
        '<input class="disk-diskname" placeholder="Disk Name">' +
        '<input class="disk-iops-total" placeholder="IOPS Total" value="9600">' +
        '<input class="disk-iops-total-max" placeholder="IOPS Max" value="11520">' +
        '<input class="disk-iops-total-max-length" placeholder="Max Length" value="60">' +
        '<button type="button" class="btn-remove" onclick="this.parentElement.remove()">X</button>';
    container.appendChild(row);
}

// Create disk
function executeCreate() {
    apiCall('create', {
        smac: val('create-smac'),
        size: val('create-size'),
    });
}

// Copy image
function executeCopyImage() {
    apiCall('copyimage', {
        itemplate: val('copyimage-itemplate'),
        smac: val('copyimage-smac'),
        size: val('copyimage-size'),
    });
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

// Live migrate
function executeLiveMigrate() {
    apiCall('livemigrate', {
        smac: val('livemigrate-smac'),
        to_node_ip: val('livemigrate-to-node-ip'),
    });
}

// VNC operations
async function executeVnc(mode) {
    var prefix = 'vnc-' + mode;
    var payload = {
        smac: val(prefix + '-smac'),
        novncport: val(prefix + '-novncport'),
    };
    await apiCall('vnc/' + mode, payload);

    // Show console link after successful VNC start
    if (mode === 'start') {
        var statusEl = document.getElementById('status-indicator');
        if (statusEl.className === 'success') {
            var port = val('vnc-start-novncport');
            var linkDiv = document.getElementById('vnc-console-link');
            var linkEl = document.getElementById('vnc-console-url');
            linkEl.href = '/vnc.html?port=' + port;
            linkDiv.style.display = 'block';
        }
    }
}

// Open VNC console in new tab
function openConsole() {
    var port = val('vnc-start-novncport');
    if (!port) {
        alert('Please enter the noVNC port first');
        return;
    }
    window.open('/vnc.html?port=' + port, '_blank');
}

// MDS Config - Load
async function loadMdsConfig() {
    var statusEl = document.getElementById('status-indicator');
    var outputEl = document.getElementById('output');
    statusEl.className = 'loading';
    statusEl.textContent = 'Loading MDS config...';
    try {
        var response = await fetch('/api/mds/config');
        var data = await response.json();
        if (data.success && data.output) {
            var config = JSON.parse(data.output);
            document.getElementById('mds-instance-id').value = config.instance_id || '';
            document.getElementById('mds-ami-id').value = config.ami_id || '';
            document.getElementById('mds-hostname-prefix').value = config.hostname_prefix || '';
            document.getElementById('mds-public-ipv4').value = config.public_ipv4 || '';
            document.getElementById('mds-local-ipv4').value = config.local_ipv4 || '';
            document.getElementById('mds-default-mac').value = config.default_mac || '';
            document.getElementById('mds-ssh-pubkey').value = config.ssh_pubkey || '';
            document.getElementById('mds-root-password').value = config.root_password || '';
            document.getElementById('mds-userdata-extra').value = config.userdata_extra || '';
            statusEl.className = 'success';
            statusEl.textContent = 'MDS config loaded';
            outputEl.textContent = data.output;
        } else {
            statusEl.className = 'error';
            statusEl.textContent = 'Error: ' + data.message;
        }
    } catch (err) {
        statusEl.className = 'error';
        statusEl.textContent = 'Network error: ' + err.message;
    }
}

// MDS Config - Save
async function saveMdsConfig() {
    var statusEl = document.getElementById('status-indicator');
    var outputEl = document.getElementById('output');
    statusEl.className = 'loading';
    statusEl.textContent = 'Saving MDS config...';
    var payload = {
        instance_id: val('mds-instance-id'),
        ami_id: val('mds-ami-id'),
        hostname_prefix: val('mds-hostname-prefix'),
        public_ipv4: val('mds-public-ipv4'),
        local_ipv4: val('mds-local-ipv4'),
        ssh_pubkey: val('mds-ssh-pubkey'),
        root_password: val('mds-root-password'),
        userdata_extra: document.getElementById('mds-userdata-extra').value,
        default_mac: val('mds-default-mac'),
        kea_socket_path: '',
    };
    try {
        var response = await fetch('/api/mds/config', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(payload),
        });
        var data = await response.json();
        if (data.success) {
            statusEl.className = 'success';
            statusEl.textContent = data.message;
            outputEl.textContent = JSON.stringify(payload, null, 2);
        } else {
            statusEl.className = 'error';
            statusEl.textContent = 'Error: ' + data.message;
        }
    } catch (err) {
        statusEl.className = 'error';
        statusEl.textContent = 'Network error: ' + err.message;
    }
}

// Init with one adapter and one disk row
window.addEventListener('DOMContentLoaded', function() {
    addNetworkAdapter();
    addDisk();
});
