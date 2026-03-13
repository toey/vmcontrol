use rusqlite::{params, Connection};
use serde::Serialize;
use std::sync::Mutex;

use crate::config::get_conf;

/// Global database connection pool (single connection protected by Mutex).
/// This avoids opening a new connection per operation, improves performance,
/// and prevents race conditions in read-modify-write sequences.
static DB_CONN: std::sync::OnceLock<Mutex<Connection>> = std::sync::OnceLock::new();

#[derive(Debug, Serialize, Clone)]
pub struct DiskRecord {
    pub name: String,
    pub size: String,
    pub owner: String,
    pub created_at: String,
    pub backing_file: String,
    pub is_template: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct VmRecord {
    pub smac: String,
    pub mac: String,
    pub disk_size: String,
    pub config: String,
    pub status: String,
    pub created_at: String,
    pub group_name: String,
}

/// Initialize the database connection and run migrations.
/// Called once via OnceLock; subsequent calls reuse the same connection.
fn init_db() -> Connection {
    let db_path = get_conf("db_path");
    if let Some(parent) = std::path::Path::new(&db_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(&db_path)
        .unwrap_or_else(|e| panic!("FATAL: Cannot open database '{}': {}", db_path, e));

    // Enable WAL mode for better concurrency
    if let Err(e) = conn.execute_batch("PRAGMA journal_mode=WAL;") {
        log::warn!("Failed to enable WAL mode: {}", e);
    }
    // Busy timeout: wait up to 5 seconds if DB is locked
    let _ = conn.execute_batch("PRAGMA busy_timeout=5000;");

    run_migrations(&conn).unwrap_or_else(|e| panic!("FATAL: DB migration failed: {}", e));
    conn
}

/// Get a locked reference to the global database connection.
/// Uses OnceLock to initialize on first call, then reuses the connection.
fn open_db() -> Result<std::sync::MutexGuard<'static, Connection>, String> {
    let mutex = DB_CONN.get_or_init(|| Mutex::new(init_db()));
    mutex.lock().map_err(|e| format!("DB lock error (mutex poisoned): {}", e))
}

/// Run all database migrations
fn run_migrations(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS vms (
            smac TEXT PRIMARY KEY,
            mac TEXT NOT NULL DEFAULT '',
            disk_size TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .map_err(|e| format!("DB init error: {}", e))?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS disks (
            name TEXT PRIMARY KEY,
            size TEXT NOT NULL DEFAULT '40G',
            owner TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .map_err(|e| format!("DB disks table init error: {}", e))?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS switches (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            mcast_port INTEGER NOT NULL UNIQUE,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .map_err(|e| format!("DB switches table init error: {}", e))?;

    // Migration: add config column if not exists
    let has_config: bool = conn
        .prepare("SELECT config FROM vms LIMIT 0")
        .is_ok();
    if !has_config {
        conn.execute_batch("ALTER TABLE vms ADD COLUMN config TEXT NOT NULL DEFAULT '{}';")
            .map_err(|e| format!("DB migrate config error: {}", e))?;
    }

    // Migration: add status column if not exists
    let has_status: bool = conn
        .prepare("SELECT status FROM vms LIMIT 0")
        .is_ok();
    if !has_status {
        conn.execute_batch("ALTER TABLE vms ADD COLUMN status TEXT NOT NULL DEFAULT 'stopped';")
            .map_err(|e| format!("DB migrate status error: {}", e))?;
    }

    // Migration: add group_name column if not exists
    let has_group: bool = conn
        .prepare("SELECT group_name FROM vms LIMIT 0")
        .is_ok();
    if !has_group {
        conn.execute_batch("ALTER TABLE vms ADD COLUMN group_name TEXT NOT NULL DEFAULT '';")
            .map_err(|e| format!("DB migrate group error: {}", e))?;
    }

    // Migration: add backing_file column to disks if not exists
    let has_backing: bool = conn
        .prepare("SELECT backing_file FROM disks LIMIT 0")
        .is_ok();
    if !has_backing {
        conn.execute_batch("ALTER TABLE disks ADD COLUMN backing_file TEXT NOT NULL DEFAULT '';")
            .map_err(|e| format!("DB migrate backing_file error: {}", e))?;
    }

    // Migration: add is_template column to disks if not exists
    let has_template: bool = conn
        .prepare("SELECT is_template FROM disks LIMIT 0")
        .is_ok();
    if !has_template {
        conn.execute_batch("ALTER TABLE disks ADD COLUMN is_template TEXT NOT NULL DEFAULT '0';")
            .map_err(|e| format!("DB migrate is_template error: {}", e))?;
    }

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS dhcp_leases (
            mac TEXT PRIMARY KEY,
            ip TEXT NOT NULL DEFAULT '',
            hostname TEXT NOT NULL DEFAULT '',
            vm_name TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .map_err(|e| format!("DB dhcp_leases table init error: {}", e))?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ssh_keys (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            pubkey TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .map_err(|e| format!("DB ssh_keys table init error: {}", e))?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS template_images (
            template_key TEXT PRIMARY KEY,
            disk_name TEXT NOT NULL
        );",
    )
    .map_err(|e| format!("DB template_images table init error: {}", e))?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS os_templates (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            key TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            vcpus TEXT NOT NULL DEFAULT '2',
            memory TEXT NOT NULL DEFAULT '2048',
            is_windows TEXT NOT NULL DEFAULT '0',
            arch TEXT NOT NULL DEFAULT 'x86_64',
            image TEXT NOT NULL DEFAULT ''
        );",
    )
    .map_err(|e| format!("DB os_templates table init error: {}", e))?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS backups (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            backup_id TEXT NOT NULL UNIQUE,
            vm_name TEXT NOT NULL DEFAULT '',
            disk_names TEXT NOT NULL DEFAULT '',
            backup_type TEXT NOT NULL DEFAULT 'full',
            note TEXT NOT NULL DEFAULT '',
            total_size INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .map_err(|e| format!("DB backups table init error: {}", e))?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS snapshots (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            snapshot_id TEXT NOT NULL,
            disk_name TEXT NOT NULL,
            vm_name TEXT NOT NULL DEFAULT '',
            note TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(snapshot_id, disk_name)
        );",
    )
    .map_err(|e| format!("DB snapshots table init error: {}", e))?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL DEFAULT ''
        );",
    )
    .map_err(|e| format!("DB settings table init error: {}", e))?;

    Ok(())
}

/// Get a setting by key
pub fn get_setting(key: &str) -> Result<Option<String>, String> {
    let conn = open_db()?;
    let mut stmt = conn
        .prepare("SELECT value FROM settings WHERE key = ?1")
        .map_err(|e| format!("DB query error: {}", e))?;
    let result = stmt
        .query_row(params![key], |row| row.get::<_, String>(0))
        .ok();
    Ok(result)
}

/// Set a setting (insert or update)
pub fn set_setting(key: &str, value: &str) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = ?2",
        params![key, value],
    )
    .map_err(|e| format!("DB setting upsert error: {}", e))?;
    Ok(())
}

/// Insert a new VM record, or update config if it already exists (preserves group_name, created_at)
pub fn insert_vm(smac: &str, mac: &str, disk_size: &str, config: &str) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute(
        "INSERT INTO vms (smac, mac, disk_size, config, status) VALUES (?1, ?2, ?3, ?4, 'stopped')
         ON CONFLICT(smac) DO UPDATE SET mac = ?2, disk_size = ?3, config = ?4, status = 'stopped'",
        params![smac, mac, disk_size, config],
    )
    .map_err(|e| format!("DB insert error: {}", e))?;
    Ok(())
}

/// Update VM config
pub fn update_vm(smac: &str, config: &str) -> Result<(), String> {
    let conn = open_db()?;
    let updated = conn
        .execute(
            "UPDATE vms SET config = ?2 WHERE smac = ?1",
            params![smac, config],
        )
        .map_err(|e| format!("DB update error: {}", e))?;
    if updated == 0 {
        return Err(format!("VM '{}' not found", smac));
    }
    Ok(())
}

/// Set VM status (stopped/running)
pub fn set_vm_status(smac: &str, status: &str) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute(
        "UPDATE vms SET status = ?2 WHERE smac = ?1",
        params![smac, status],
    )
    .map_err(|e| format!("DB status error: {}", e))?;
    Ok(())
}

/// Get a single VM record
pub fn get_vm(smac: &str) -> Result<VmRecord, String> {
    let conn = open_db()?;
    conn.query_row(
        "SELECT smac, mac, disk_size, COALESCE(config,'{}'), COALESCE(status,'stopped'), created_at, COALESCE(group_name,'') FROM vms WHERE smac = ?1",
        params![smac],
        |row| {
            Ok(VmRecord {
                smac: row.get(0)?,
                mac: row.get(1)?,
                disk_size: row.get(2)?,
                config: row.get(3)?,
                status: row.get(4)?,
                created_at: row.get(5)?,
                group_name: row.get(6)?,
            })
        },
    )
    .map_err(|e| format!("VM '{}' not found: {}", smac, e))
}

/// Delete a VM record by smac
pub fn delete_vm(smac: &str) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute("DELETE FROM vms WHERE smac = ?1", params![smac])
        .map_err(|e| format!("DB delete error: {}", e))?;
    Ok(())
}

/// List all VM records, ordered by group then created_at descending
pub fn list_vms() -> Result<Vec<VmRecord>, String> {
    let conn = open_db()?;
    let mut stmt = conn
        .prepare("SELECT smac, mac, disk_size, COALESCE(config,'{}'), COALESCE(status,'stopped'), created_at, COALESCE(group_name,'') FROM vms ORDER BY group_name ASC, created_at DESC")
        .map_err(|e| format!("DB query error: {}", e))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(VmRecord {
                smac: row.get(0)?,
                mac: row.get(1)?,
                disk_size: row.get(2)?,
                config: row.get(3)?,
                status: row.get(4)?,
                created_at: row.get(5)?,
                group_name: row.get(6)?,
            })
        })
        .map_err(|e| format!("DB query error: {}", e))?;
    let mut vms = Vec::new();
    for row in rows {
        vms.push(row.map_err(|e| format!("DB row error: {}", e))?);
    }
    Ok(vms)
}

// ======== Group operations ========

/// Set VM group
pub fn set_vm_group(smac: &str, group_name: &str) -> Result<(), String> {
    let conn = open_db()?;
    let updated = conn
        .execute(
            "UPDATE vms SET group_name = ?2 WHERE smac = ?1",
            params![smac, group_name],
        )
        .map_err(|e| format!("DB set group error: {}", e))?;
    if updated == 0 {
        return Err(format!("VM '{}' not found", smac));
    }
    Ok(())
}

/// List distinct group names
pub fn list_groups() -> Result<Vec<String>, String> {
    let conn = open_db()?;
    let mut stmt = conn
        .prepare("SELECT DISTINCT group_name FROM vms WHERE group_name != '' ORDER BY group_name ASC")
        .map_err(|e| format!("DB query error: {}", e))?;
    let rows = stmt
        .query_map([], |row| row.get(0))
        .map_err(|e| format!("DB query error: {}", e))?;
    let mut groups = Vec::new();
    for row in rows {
        groups.push(row.map_err(|e| format!("DB row error: {}", e))?);
    }
    Ok(groups)
}

/// Rename a VM (update smac primary key and all disk owner references)
pub fn rename_vm(old_smac: &str, new_smac: &str) -> Result<(), String> {
    let conn = open_db()?;
    // Use a transaction to keep both tables consistent
    conn.execute_batch("BEGIN TRANSACTION")
        .map_err(|e| format!("DB transaction error: {}", e))?;
    // Insert new row with all data from old row
    let result = (|| {
        conn.execute(
            "INSERT INTO vms (smac, mac, disk_size, config, status, created_at, group_name)
             SELECT ?2, mac, disk_size, config, status, created_at, group_name FROM vms WHERE smac = ?1",
            params![old_smac, new_smac],
        ).map_err(|e| format!("DB rename insert error: {}", e))?;
        // Update disk owners
        conn.execute(
            "UPDATE disks SET owner = ?2 WHERE owner = ?1",
            params![old_smac, new_smac],
        ).map_err(|e| format!("DB rename disk owner error: {}", e))?;
        // Delete old row
        conn.execute(
            "DELETE FROM vms WHERE smac = ?1",
            params![old_smac],
        ).map_err(|e| format!("DB rename delete error: {}", e))?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT")
                .map_err(|e| format!("DB commit error: {}", e))?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

// ======== Disk operations ========

/// Insert a new disk record (INSERT OR IGNORE to avoid duplicate errors on auto-sync)
pub fn insert_disk(name: &str, size: &str) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute(
        "INSERT OR IGNORE INTO disks (name, size) VALUES (?1, ?2)",
        params![name, size],
    )
    .map_err(|e| format!("DB insert disk error: {}", e))?;
    Ok(())
}

/// List all disk records
pub fn list_disks() -> Result<Vec<DiskRecord>, String> {
    let conn = open_db()?;
    let mut stmt = conn
        .prepare("SELECT name, size, COALESCE(owner,''), created_at, COALESCE(backing_file,''), COALESCE(is_template,'0') FROM disks ORDER BY created_at DESC")
        .map_err(|e| format!("DB query error: {}", e))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(DiskRecord {
                name: row.get(0)?,
                size: row.get(1)?,
                owner: row.get(2)?,
                created_at: row.get(3)?,
                backing_file: row.get(4)?,
                is_template: row.get(5)?,
            })
        })
        .map_err(|e| format!("DB query error: {}", e))?;
    let mut disks = Vec::new();
    for row in rows {
        disks.push(row.map_err(|e| format!("DB row error: {}", e))?);
    }
    Ok(disks)
}

/// Delete a disk record
pub fn delete_disk(name: &str) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute("DELETE FROM disks WHERE name = ?1", params![name])
        .map_err(|e| format!("DB delete disk error: {}", e))?;
    Ok(())
}

/// Set disk owner (assign to a VM)
pub fn set_disk_owner(name: &str, owner: &str) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute(
        "UPDATE disks SET owner = ?2 WHERE name = ?1",
        params![name, owner],
    )
    .map_err(|e| format!("DB set disk owner error: {}", e))?;
    Ok(())
}

/// Update disk size
pub fn update_disk_size(name: &str, new_size: &str) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute(
        "UPDATE disks SET size = ?2 WHERE name = ?1",
        params![name, new_size],
    )
    .map_err(|e| format!("DB update disk size error: {}", e))?;
    Ok(())
}

/// Clear disk owner for all disks owned by a VM
pub fn clear_disk_owner_by_vm(smac: &str) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute(
        "UPDATE disks SET owner = '' WHERE owner = ?1",
        params![smac],
    )
    .map_err(|e| format!("DB clear disk owner error: {}", e))?;
    Ok(())
}

/// Insert a new disk with backing file reference (linked clone)
pub fn insert_disk_with_backing(name: &str, size: &str, backing_file: &str) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute(
        "INSERT OR IGNORE INTO disks (name, size, backing_file) VALUES (?1, ?2, ?3)",
        params![name, size, backing_file],
    )
    .map_err(|e| format!("DB insert disk with backing error: {}", e))?;
    Ok(())
}

/// Count how many disks use this disk as backing_file
pub fn count_linked_clones(backing_name: &str) -> Result<i64, String> {
    let conn = open_db()?;
    conn.query_row(
        "SELECT COUNT(*) FROM disks WHERE backing_file = ?1",
        params![backing_name],
        |row| row.get(0),
    )
    .map_err(|e| format!("DB count linked clones error: {}", e))
}

/// Set backing_file for a disk
pub fn set_disk_backing(name: &str, backing_file: &str) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute(
        "UPDATE disks SET backing_file = ?2 WHERE name = ?1",
        params![name, backing_file],
    )
    .map_err(|e| format!("DB set disk backing error: {}", e))?;
    Ok(())
}

/// Set is_template flag for a disk
pub fn set_disk_template(name: &str, is_template: &str) -> Result<(), String> {
    let conn = open_db()?;
    let updated = conn.execute(
        "UPDATE disks SET is_template = ?2 WHERE name = ?1",
        params![name, is_template],
    )
    .map_err(|e| format!("DB set disk template error: {}", e))?;
    if updated == 0 {
        return Err(format!("Disk '{}' not found", name));
    }
    Ok(())
}

// ======== Switch operations ========

#[derive(Debug, Serialize, Clone)]
pub struct SwitchRecord {
    pub id: i64,
    pub name: String,
    pub mcast_port: i64,
    pub created_at: String,
}

/// Insert a new switch with auto-assigned multicast port
pub fn insert_switch(name: &str) -> Result<i64, String> {
    let conn = open_db()?;
    let next_port: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(mcast_port), 10000) + 1 FROM switches",
            [],
            |row| row.get(0),
        )
        .unwrap_or(10001);
    conn.execute(
        "INSERT INTO switches (name, mcast_port) VALUES (?1, ?2)",
        params![name, next_port],
    )
    .map_err(|e| format!("DB insert switch error: {}", e))?;
    Ok(conn.last_insert_rowid())
}

/// List all switches
pub fn list_switches() -> Result<Vec<SwitchRecord>, String> {
    let conn = open_db()?;
    let mut stmt = conn
        .prepare("SELECT id, name, mcast_port, created_at FROM switches ORDER BY id")
        .map_err(|e| format!("DB query error: {}", e))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(SwitchRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                mcast_port: row.get(2)?,
                created_at: row.get(3)?,
            })
        })
        .map_err(|e| format!("DB query error: {}", e))?;
    let mut switches = Vec::new();
    for row in rows {
        switches.push(row.map_err(|e| format!("DB row error: {}", e))?);
    }
    Ok(switches)
}

/// Get a switch by name
pub fn get_switch_by_name(name: &str) -> Result<SwitchRecord, String> {
    let conn = open_db()?;
    conn.query_row(
        "SELECT id, name, mcast_port, created_at FROM switches WHERE name = ?1",
        params![name],
        |row| {
            Ok(SwitchRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                mcast_port: row.get(2)?,
                created_at: row.get(3)?,
            })
        },
    )
    .map_err(|e| format!("Switch '{}' not found: {}", name, e))
}

/// Get a switch by id
pub fn get_switch_by_id(id: i64) -> Result<SwitchRecord, String> {
    let conn = open_db()?;
    conn.query_row(
        "SELECT id, name, mcast_port, created_at FROM switches WHERE id = ?1",
        params![id],
        |row| {
            Ok(SwitchRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                mcast_port: row.get(2)?,
                created_at: row.get(3)?,
            })
        },
    )
    .map_err(|e| format!("Switch ID {} not found: {}", id, e))
}

/// Delete a switch by id
pub fn delete_switch(id: i64) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute("DELETE FROM switches WHERE id = ?1", params![id])
        .map_err(|e| format!("DB delete switch error: {}", e))?;
    Ok(())
}

/// Rename a switch
pub fn rename_switch(id: i64, new_name: &str) -> Result<(), String> {
    let conn = open_db()?;
    let updated = conn
        .execute(
            "UPDATE switches SET name = ?2 WHERE id = ?1",
            params![id, new_name],
        )
        .map_err(|e| format!("DB rename switch error: {}", e))?;
    if updated == 0 {
        return Err(format!("Switch ID {} not found", id));
    }
    Ok(())
}

// ======== DHCP Lease operations ========

#[derive(Debug, Serialize, Clone)]
pub struct DhcpLease {
    pub mac: String,
    pub ip: String,
    pub hostname: String,
    pub vm_name: String,
    pub created_at: String,
}

/// Insert or update a DHCP lease
pub fn upsert_dhcp_lease(mac: &str, ip: &str, hostname: &str, vm_name: &str) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute(
        "INSERT INTO dhcp_leases (mac, ip, hostname, vm_name) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(mac) DO UPDATE SET ip = ?2, hostname = ?3, vm_name = ?4",
        params![mac, ip, hostname, vm_name],
    )
    .map_err(|e| format!("DB upsert dhcp lease error: {}", e))?;
    Ok(())
}

/// List all DHCP leases
pub fn list_dhcp_leases() -> Result<Vec<DhcpLease>, String> {
    let conn = open_db()?;
    let mut stmt = conn
        .prepare("SELECT mac, ip, hostname, vm_name, created_at FROM dhcp_leases ORDER BY ip ASC")
        .map_err(|e| format!("DB query error: {}", e))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(DhcpLease {
                mac: row.get(0)?,
                ip: row.get(1)?,
                hostname: row.get(2)?,
                vm_name: row.get(3)?,
                created_at: row.get(4)?,
            })
        })
        .map_err(|e| format!("DB query error: {}", e))?;
    let mut leases = Vec::new();
    for row in rows {
        leases.push(row.map_err(|e| format!("DB row error: {}", e))?);
    }
    Ok(leases)
}

/// Delete a DHCP lease by MAC
pub fn delete_dhcp_lease(mac: &str) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute("DELETE FROM dhcp_leases WHERE mac = ?1", params![mac])
        .map_err(|e| format!("DB delete dhcp lease error: {}", e))?;
    Ok(())
}

// ── SSH Keys ──

#[derive(Debug, Serialize, Clone)]
pub struct SshKeyRecord {
    pub id: i64,
    pub name: String,
    pub pubkey: String,
    pub created_at: String,
}

pub fn insert_ssh_key(name: &str, pubkey: &str) -> Result<i64, String> {
    let conn = open_db()?;
    conn.execute(
        "INSERT INTO ssh_keys (name, pubkey) VALUES (?1, ?2)",
        params![name, pubkey],
    )
    .map_err(|e| format!("DB insert ssh key error: {}", e))?;
    Ok(conn.last_insert_rowid())
}

pub fn list_ssh_keys() -> Result<Vec<SshKeyRecord>, String> {
    let conn = open_db()?;
    let mut stmt = conn
        .prepare("SELECT id, name, pubkey, created_at FROM ssh_keys ORDER BY name ASC")
        .map_err(|e| format!("DB query error: {}", e))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(SshKeyRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                pubkey: row.get(2)?,
                created_at: row.get(3)?,
            })
        })
        .map_err(|e| format!("DB query error: {}", e))?;
    let mut keys = Vec::new();
    for row in rows {
        keys.push(row.map_err(|e| format!("DB row error: {}", e))?);
    }
    Ok(keys)
}

pub fn delete_ssh_key(id: i64) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute("DELETE FROM ssh_keys WHERE id = ?1", params![id])
        .map_err(|e| format!("DB delete ssh key error: {}", e))?;
    Ok(())
}

// ── Template Image Mappings ──

pub fn set_template_image(template_key: &str, disk_name: &str) -> Result<(), String> {
    let conn = open_db()?;
    if disk_name.is_empty() {
        conn.execute("DELETE FROM template_images WHERE template_key = ?1", params![template_key])
            .map_err(|e| format!("DB delete template image error: {}", e))?;
    } else {
        conn.execute(
            "INSERT INTO template_images (template_key, disk_name) VALUES (?1, ?2)
             ON CONFLICT(template_key) DO UPDATE SET disk_name = ?2",
            params![template_key, disk_name],
        )
        .map_err(|e| format!("DB set template image error: {}", e))?;
    }
    Ok(())
}

pub fn list_template_images() -> Result<Vec<(String, String)>, String> {
    let conn = open_db()?;
    let mut stmt = conn
        .prepare("SELECT template_key, disk_name FROM template_images ORDER BY template_key ASC")
        .map_err(|e| format!("DB query error: {}", e))?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(|e| format!("DB query error: {}", e))?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row.map_err(|e| format!("DB row error: {}", e))?);
    }
    Ok(result)
}

// ── OS Templates ──

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct OsTemplate {
    pub id: i64,
    pub key: String,
    pub name: String,
    pub vcpus: String,
    pub memory: String,
    pub is_windows: String,
    pub arch: String,
    pub image: String,
}

pub fn list_os_templates() -> Result<Vec<OsTemplate>, String> {
    let conn = open_db()?;
    let mut stmt = conn
        .prepare("SELECT id, key, name, vcpus, memory, is_windows, arch, image FROM os_templates ORDER BY id ASC")
        .map_err(|e| format!("DB query error: {}", e))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(OsTemplate {
                id: row.get(0)?,
                key: row.get(1)?,
                name: row.get(2)?,
                vcpus: row.get(3)?,
                memory: row.get(4)?,
                is_windows: row.get(5)?,
                arch: row.get(6)?,
                image: row.get(7)?,
            })
        })
        .map_err(|e| format!("DB query error: {}", e))?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row.map_err(|e| format!("DB row error: {}", e))?);
    }
    Ok(result)
}

pub fn create_os_template(key: &str, name: &str, vcpus: &str, memory: &str, is_windows: &str, arch: &str, image: &str) -> Result<i64, String> {
    let conn = open_db()?;
    conn.execute(
        "INSERT INTO os_templates (key, name, vcpus, memory, is_windows, arch, image) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![key, name, vcpus, memory, is_windows, arch, image],
    )
    .map_err(|e| format!("DB insert os_template error: {}", e))?;
    Ok(conn.last_insert_rowid())
}

pub fn update_os_template(id: i64, key: &str, name: &str, vcpus: &str, memory: &str, is_windows: &str, arch: &str, image: &str) -> Result<(), String> {
    let conn = open_db()?;
    let changed = conn.execute(
        "UPDATE os_templates SET key=?2, name=?3, vcpus=?4, memory=?5, is_windows=?6, arch=?7, image=?8 WHERE id=?1",
        params![id, key, name, vcpus, memory, is_windows, arch, image],
    )
    .map_err(|e| format!("DB update os_template error: {}", e))?;
    if changed == 0 {
        return Err(format!("OS template id {} not found", id));
    }
    Ok(())
}

pub fn delete_os_template(id: i64) -> Result<(), String> {
    let conn = open_db()?;
    let changed = conn.execute("DELETE FROM os_templates WHERE id = ?1", params![id])
        .map_err(|e| format!("DB delete os_template error: {}", e))?;
    if changed == 0 {
        return Err(format!("OS template id {} not found", id));
    }
    Ok(())
}

// ======== Backup operations ========

#[derive(Debug, Serialize, Clone)]
pub struct BackupRecord {
    pub id: i64,
    pub backup_id: String,
    pub vm_name: String,
    pub disk_names: String,
    pub backup_type: String,
    pub note: String,
    pub total_size: i64,
    pub created_at: String,
}

pub fn insert_backup(backup_id: &str, vm_name: &str, disk_names: &str, backup_type: &str, note: &str, total_size: i64) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute(
        "INSERT INTO backups (backup_id, vm_name, disk_names, backup_type, note, total_size) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![backup_id, vm_name, disk_names, backup_type, note, total_size],
    ).map_err(|e| format!("DB insert backup error: {}", e))?;
    Ok(())
}

pub fn list_backups() -> Result<Vec<BackupRecord>, String> {
    let conn = open_db()?;
    let mut stmt = conn.prepare(
        "SELECT id, backup_id, vm_name, disk_names, backup_type, note, total_size, created_at FROM backups ORDER BY created_at DESC"
    ).map_err(|e| format!("DB query error: {}", e))?;
    let rows = stmt.query_map([], |row| {
        Ok(BackupRecord {
            id: row.get(0)?,
            backup_id: row.get(1)?,
            vm_name: row.get(2)?,
            disk_names: row.get(3)?,
            backup_type: row.get(4)?,
            note: row.get(5)?,
            total_size: row.get(6)?,
            created_at: row.get(7)?,
        })
    }).map_err(|e| format!("DB query error: {}", e))?;
    let mut result = Vec::new();
    for r in rows {
        result.push(r.map_err(|e| format!("DB row error: {}", e))?);
    }
    Ok(result)
}

pub fn get_backup(backup_id: &str) -> Result<BackupRecord, String> {
    let conn = open_db()?;
    conn.query_row(
        "SELECT id, backup_id, vm_name, disk_names, backup_type, note, total_size, created_at FROM backups WHERE backup_id = ?1",
        params![backup_id],
        |row| Ok(BackupRecord {
            id: row.get(0)?,
            backup_id: row.get(1)?,
            vm_name: row.get(2)?,
            disk_names: row.get(3)?,
            backup_type: row.get(4)?,
            note: row.get(5)?,
            total_size: row.get(6)?,
            created_at: row.get(7)?,
        }),
    ).map_err(|e| format!("Backup '{}' not found: {}", backup_id, e))
}

pub fn delete_backup_record(backup_id: &str) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute("DELETE FROM backups WHERE backup_id = ?1", params![backup_id])
        .map_err(|e| format!("DB delete backup error: {}", e))?;
    Ok(())
}

// ======== Snapshot operations ========

#[derive(Debug, Serialize, Clone)]
pub struct SnapshotRecord {
    pub id: i64,
    pub snapshot_id: String,
    pub disk_name: String,
    pub vm_name: String,
    pub note: String,
    pub created_at: String,
}

pub fn insert_snapshot(snapshot_id: &str, disk_name: &str, vm_name: &str, note: &str) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute(
        "INSERT OR IGNORE INTO snapshots (snapshot_id, disk_name, vm_name, note) VALUES (?1, ?2, ?3, ?4)",
        params![snapshot_id, disk_name, vm_name, note],
    ).map_err(|e| format!("DB insert snapshot error: {}", e))?;
    Ok(())
}

pub fn list_snapshots_by_vm(vm_name: &str) -> Result<Vec<SnapshotRecord>, String> {
    let conn = open_db()?;
    let mut stmt = conn.prepare(
        "SELECT id, snapshot_id, disk_name, vm_name, note, created_at FROM snapshots WHERE vm_name = ?1 ORDER BY created_at DESC"
    ).map_err(|e| format!("DB query error: {}", e))?;
    let rows = stmt.query_map(params![vm_name], |row| {
        Ok(SnapshotRecord {
            id: row.get(0)?,
            snapshot_id: row.get(1)?,
            disk_name: row.get(2)?,
            vm_name: row.get(3)?,
            note: row.get(4)?,
            created_at: row.get(5)?,
        })
    }).map_err(|e| format!("DB query error: {}", e))?;
    let mut result = Vec::new();
    for r in rows {
        result.push(r.map_err(|e| format!("DB row error: {}", e))?);
    }
    Ok(result)
}

pub fn delete_snapshot_record(snapshot_id: &str, disk_name: &str) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute("DELETE FROM snapshots WHERE snapshot_id = ?1 AND disk_name = ?2", params![snapshot_id, disk_name])
        .map_err(|e| format!("DB delete snapshot error: {}", e))?;
    Ok(())
}

pub fn delete_snapshots_by_id(snapshot_id: &str) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute("DELETE FROM snapshots WHERE snapshot_id = ?1", params![snapshot_id])
        .map_err(|e| format!("DB delete snapshots error: {}", e))?;
    Ok(())
}
