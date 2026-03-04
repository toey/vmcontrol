use rusqlite::{params, Connection};
use serde::Serialize;

use crate::config::get_conf;

#[derive(Debug, Serialize, Clone)]
pub struct DiskRecord {
    pub name: String,
    pub size: String,
    pub owner: String,
    pub created_at: String,
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

/// Open (or create) the database, ensure the table exists + migrate
fn open_db() -> Result<Connection, String> {
    let db_path = get_conf("db_path");
    if let Some(parent) = std::path::Path::new(&db_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn =
        Connection::open(&db_path).map_err(|e| format!("DB open error: {}", e))?;

    // Enable WAL mode for better concurrency
    let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");

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

    Ok(conn)
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
        .prepare("SELECT name, size, COALESCE(owner,''), created_at FROM disks ORDER BY created_at DESC")
        .map_err(|e| format!("DB query error: {}", e))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(DiskRecord {
                name: row.get(0)?,
                size: row.get(1)?,
                owner: row.get(2)?,
                created_at: row.get(3)?,
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
