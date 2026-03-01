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

    Ok(conn)
}

/// Insert or replace a VM record with full config
pub fn insert_vm(smac: &str, mac: &str, disk_size: &str, config: &str) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute(
        "INSERT OR REPLACE INTO vms (smac, mac, disk_size, config, status) VALUES (?1, ?2, ?3, ?4, 'stopped')",
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
        "SELECT smac, mac, disk_size, COALESCE(config,'{}'), COALESCE(status,'stopped'), created_at FROM vms WHERE smac = ?1",
        params![smac],
        |row| {
            Ok(VmRecord {
                smac: row.get(0)?,
                mac: row.get(1)?,
                disk_size: row.get(2)?,
                config: row.get(3)?,
                status: row.get(4)?,
                created_at: row.get(5)?,
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

/// List all VM records, ordered by created_at descending (newest first)
pub fn list_vms() -> Result<Vec<VmRecord>, String> {
    let conn = open_db()?;
    let mut stmt = conn
        .prepare("SELECT smac, mac, disk_size, COALESCE(config,'{}'), COALESCE(status,'stopped'), created_at FROM vms ORDER BY created_at DESC")
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
            })
        })
        .map_err(|e| format!("DB query error: {}", e))?;
    let mut vms = Vec::new();
    for row in rows {
        vms.push(row.map_err(|e| format!("DB row error: {}", e))?);
    }
    Ok(vms)
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
