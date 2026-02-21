// Tab switching
document.querySelectorAll('.tab').forEach(function(tab) {
    tab.addEventListener('click', function() {
        document.querySelectorAll('.tab').forEach(function(t) { t.classList.remove('active'); });
        document.querySelectorAll('.tab-panel').forEach(function(p) { p.classList.remove('active'); });
        tab.classList.add('active');
        document.getElementById('tab-' + tab.dataset.tab).classList.add('active');
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
        var response = await fetch('/api/vm/' + operation, {
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

// SimpleCmd operations
function executeSimple(operation) {
    apiCall(operation, {
        node_ip: val(operation + '-node-ip'),
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
        node: { ip: val('start-node-ip') },
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

// Dynamic rows - Network Adapter
function addNetworkAdapter() {
    var container = document.getElementById('start-network-adapters');
    var row = document.createElement('div');
    row.className = 'adapter-row';
    row.innerHTML =
        '<input class="adapter-netid" placeholder="Net ID" value="0">' +
        '<input class="adapter-mac" placeholder="MAC (52:54:c4:ca:42:38)">' +
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
        node_ip: val('create-node-ip'),
        smac: val('create-smac'),
        size: val('create-size'),
    });
}

// Copy image
function executeCopyImage() {
    apiCall('copyimage', {
        node_ip: val('copyimage-node-ip'),
        itemplate: val('copyimage-itemplate'),
        smac: val('copyimage-smac'),
        size: val('copyimage-size'),
    });
}

// Mount ISO
function executeMountIso() {
    apiCall('mountiso', {
        node_ip: val('mountiso-node-ip'),
        smac: val('mountiso-smac'),
        isoname: val('mountiso-isoname'),
    });
}

// Live migrate
function executeLiveMigrate() {
    apiCall('livemigrate', {
        node_ip: val('livemigrate-node-ip'),
        smac: val('livemigrate-smac'),
        to_node_ip: val('livemigrate-to-node-ip'),
    });
}

// Init with one adapter and one disk row
window.addEventListener('DOMContentLoaded', function() {
    addNetworkAdapter();
    addDisk();
});
