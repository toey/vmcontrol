use rusqlite::{params, Connection};
use serde::Serialize;

const DB_PATH: &str = "/tmp/vmcontrol/vmcontrol.db";

#[derive(Debug, Serialize)]
pub struct VmRecord {
    pub smac: String,
    pub mac: String,
    pub disk_size: String,
    pub created_at: String,
}

/// Open (or create) the database, ensure the table exists
fn open_db() -> Result<Connection, String> {
    let _ = std::fs::create_dir_all("/tmp/vmcontrol");
    let conn =
        Connection::open(DB_PATH).map_err(|e| format!("DB open error: {}", e))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS vms (
            smac TEXT PRIMARY KEY,
            mac TEXT NOT NULL DEFAULT '',
            disk_size TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .map_err(|e| format!("DB init error: {}", e))?;
    Ok(conn)
}

/// Insert or replace a VM record
pub fn insert_vm(smac: &str, mac: &str, disk_size: &str) -> Result<(), String> {
    let conn = open_db()?;
    conn.execute(
        "INSERT OR REPLACE INTO vms (smac, mac, disk_size) VALUES (?1, ?2, ?3)",
        params![smac, mac, disk_size],
    )
    .map_err(|e| format!("DB insert error: {}", e))?;
    Ok(())
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
        .prepare("SELECT smac, mac, disk_size, created_at FROM vms ORDER BY created_at DESC")
        .map_err(|e| format!("DB query error: {}", e))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(VmRecord {
                smac: row.get(0)?,
                mac: row.get(1)?,
                disk_size: row.get(2)?,
                created_at: row.get(3)?,
            })
        })
        .map_err(|e| format!("DB query error: {}", e))?;
    let mut vms = Vec::new();
    for row in rows {
        vms.push(row.map_err(|e| format!("DB row error: {}", e))?);
    }
    Ok(vms)
}
