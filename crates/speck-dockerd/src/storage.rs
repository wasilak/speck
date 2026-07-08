use std::collections::HashMap;
use std::path::Path;

use rusqlite::OptionalExtension;
use speck_core::network::NetworkSummary;
use speck_core::volume::VolumeSummary;

use crate::error::{DockerApiError, Result};
use crate::handlers::containers::PortBindingBody;
use crate::state::ExecSpec;

pub struct Storage {
    conn: rusqlite::Connection,
}

impl Storage {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = rusqlite::Connection::open(path)
            .map_err(|e| DockerApiError::Internal(format!("open SQLite: {e}")))?;
        let mut storage = Self { conn };
        storage.migrate()?;
        Ok(storage)
    }

    fn migrate(&mut self) -> Result<()> {
        self.conn
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS volumes (
                    name TEXT PRIMARY KEY,
                    driver TEXT NOT NULL DEFAULT 'local',
                    mountpoint TEXT NOT NULL,
                    created_at TEXT
                );

                CREATE TABLE IF NOT EXISTS networks (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    driver TEXT NOT NULL DEFAULT 'bridge',
                    scope TEXT NOT NULL DEFAULT 'local'
                );

                CREATE TABLE IF NOT EXISTS exec_sessions (
                    id TEXT PRIMARY KEY,
                    container_id TEXT NOT NULL,
                    cmd TEXT NOT NULL,
                    env TEXT NOT NULL DEFAULT '[]',
                    attach_stdin INTEGER NOT NULL DEFAULT 0,
                    attach_stdout INTEGER NOT NULL DEFAULT 1,
                    attach_stderr INTEGER NOT NULL DEFAULT 1,
                    tty INTEGER NOT NULL DEFAULT 0,
                    running INTEGER NOT NULL DEFAULT 0,
                    exit_code INTEGER
                );

                CREATE TABLE IF NOT EXISTS container_meta (
                    id TEXT PRIMARY KEY,
                    image TEXT NOT NULL,
                    create_body_json TEXT NOT NULL,
                    port_bindings_json TEXT,
                    created_at TEXT NOT NULL
                );
                ",
            )
            .map_err(|e| DockerApiError::Internal(format!("migrate: {e}")))?;
        Ok(())
    }

    // --- Volume CRUD ---

    pub fn load_volumes(&self) -> Result<HashMap<String, VolumeSummary>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, driver, mountpoint, created_at FROM volumes")
            .map_err(|e| DockerApiError::Internal(format!("prepare load_volumes: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(VolumeSummary {
                    name: row.get(0)?,
                    driver: row.get(1)?,
                    mountpoint: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .map_err(|e| DockerApiError::Internal(format!("query load_volumes: {e}")))?;

        let mut map = HashMap::new();
        for row in rows {
            let vol =
                row.map_err(|e| DockerApiError::Internal(format!("row load_volumes: {e}")))?;
            map.insert(vol.name.clone(), vol);
        }
        Ok(map)
    }

    pub fn save_volume(&self, volume: &VolumeSummary) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO volumes (name, driver, mountpoint, created_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![volume.name, volume.driver, volume.mountpoint, volume.created_at],
            )
            .map_err(|e| DockerApiError::Internal(format!("save_volume: {e}")))?;
        Ok(())
    }

    pub fn delete_volume(&self, name: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM volumes WHERE name = ?1",
                rusqlite::params![name],
            )
            .map_err(|e| DockerApiError::Internal(format!("delete_volume: {e}")))?;
        Ok(())
    }

    // --- Network CRUD ---

    pub fn load_networks(&self) -> Result<HashMap<String, NetworkSummary>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, driver, scope FROM networks")
            .map_err(|e| DockerApiError::Internal(format!("prepare load_networks: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(NetworkSummary {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    driver: row.get(2)?,
                    scope: row.get(3)?,
                })
            })
            .map_err(|e| DockerApiError::Internal(format!("query load_networks: {e}")))?;

        let mut map = HashMap::new();
        for row in rows {
            let net =
                row.map_err(|e| DockerApiError::Internal(format!("row load_networks: {e}")))?;
            map.insert(net.id.clone(), net);
        }
        Ok(map)
    }

    pub fn save_network(&self, network: &NetworkSummary) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO networks (id, name, driver, scope) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![network.id, network.name, network.driver, network.scope],
            )
            .map_err(|e| DockerApiError::Internal(format!("save_network: {e}")))?;
        Ok(())
    }

    pub fn delete_network(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM networks WHERE id = ?1", rusqlite::params![id])
            .map_err(|e| DockerApiError::Internal(format!("delete_network: {e}")))?;
        Ok(())
    }

    // --- Exec CRUD ---

    pub fn save_exec(&self, spec: &ExecSpec) -> Result<()> {
        let cmd_json = serde_json::to_string(&spec.cmd)
            .map_err(|e| DockerApiError::Internal(format!("serialize cmd: {e}")))?;
        let env_json = serde_json::to_string(&spec.env)
            .map_err(|e| DockerApiError::Internal(format!("serialize env: {e}")))?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO exec_sessions \
                 (id, container_id, cmd, env, attach_stdin, attach_stdout, attach_stderr, tty, running, exit_code) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    spec.id,
                    spec.container_id,
                    cmd_json,
                    env_json,
                    if spec.attach_stdin { 1i64 } else { 0i64 },
                    if spec.attach_stdout { 1i64 } else { 0i64 },
                    if spec.attach_stderr { 1i64 } else { 0i64 },
                    if spec.tty { 1i64 } else { 0i64 },
                    if spec.running { 1i64 } else { 0i64 },
                    spec.exit_code,
                ],
            )
            .map_err(|e| DockerApiError::Internal(format!("save_exec: {e}")))?;
        Ok(())
    }

    pub fn update_exec(&self, spec: &ExecSpec) -> Result<()> {
        self.conn
            .execute(
                "UPDATE exec_sessions SET running = ?1, exit_code = ?2 WHERE id = ?3",
                rusqlite::params![
                    if spec.running { 1i64 } else { 0i64 },
                    spec.exit_code,
                    spec.id,
                ],
            )
            .map_err(|e| DockerApiError::Internal(format!("update_exec: {e}")))?;
        Ok(())
    }

    pub fn reconcile_execs(&self) -> Result<usize> {
        let changed = self
            .conn
            .execute(
                "UPDATE exec_sessions SET running = 0, exit_code = -1 WHERE running = 1",
                [],
            )
            .map_err(|e| DockerApiError::Internal(format!("reconcile_execs: {e}")))?;
        Ok(changed)
    }

    // --- Container metadata CRUD ---

    pub fn save_container_meta(
        &self,
        id: &str,
        image: &str,
        create_body_json: &str,
        port_bindings_json: &Option<String>,
        created_at: &str,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO container_meta \
                 (id, image, create_body_json, port_bindings_json, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![id, image, create_body_json, port_bindings_json, created_at],
            )
            .map_err(|e| DockerApiError::Internal(format!("save_container_meta: {e}")))?;
        Ok(())
    }

    pub fn load_port_bindings(
        &self,
        id: &str,
    ) -> Result<Option<HashMap<String, Vec<PortBindingBody>>>> {
        let mut stmt = self
            .conn
            .prepare("SELECT port_bindings_json FROM container_meta WHERE id = ?1")
            .map_err(|e| DockerApiError::Internal(format!("prepare load_port_bindings: {e}")))?;

        let row: Option<Option<String>> = stmt
            .query_row(rusqlite::params![id], |row| row.get::<_, Option<String>>(0))
            .optional()
            .map_err(|e| DockerApiError::Internal(format!("query load_port_bindings: {e}")))?;

        match row {
            None | Some(None) => Ok(None),
            Some(Some(json)) => {
                let bindings: HashMap<String, Vec<PortBindingBody>> = serde_json::from_str(&json)
                    .map_err(|e| {
                    DockerApiError::Internal(format!("deserialize port_bindings: {e}"))
                })?;
                Ok(Some(bindings))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use speck_core::network::NetworkSummary;
    use speck_core::volume::VolumeSummary;

    fn in_memory() -> Storage {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory SQLite");
        let mut storage = Storage { conn };
        storage.migrate().expect("migrate");
        storage
    }

    fn make_exec(id: &str, running: bool) -> ExecSpec {
        ExecSpec {
            id: id.to_owned(),
            container_id: "ctr1".to_owned(),
            cmd: vec!["echo".to_owned(), "hello".to_owned()],
            env: vec!["HOME=/root".to_owned()],
            attach_stdin: false,
            attach_stdout: true,
            attach_stderr: true,
            tty: false,
            running,
            exit_code: None,
        }
    }

    #[test]
    fn test_storage_volume_roundtrip() {
        let storage = in_memory();
        let vol = VolumeSummary {
            name: "myvol".to_owned(),
            driver: "local".to_owned(),
            mountpoint: "/var/lib/speck/volumes/myvol".to_owned(),
            created_at: Some("2024-01-01T00:00:00Z".to_owned()),
        };
        storage.save_volume(&vol).unwrap();
        let loaded = storage.load_volumes().unwrap();
        assert_eq!(loaded.len(), 1);
        let got = loaded.get("myvol").unwrap();
        assert_eq!(got.name, "myvol");
        assert_eq!(got.driver, "local");
        assert_eq!(got.mountpoint, "/var/lib/speck/volumes/myvol");
        assert_eq!(got.created_at.as_deref(), Some("2024-01-01T00:00:00Z"));
    }

    #[test]
    fn test_storage_network_roundtrip() {
        let storage = in_memory();
        let net = NetworkSummary {
            id: "net1".to_owned(),
            name: "mynet".to_owned(),
            driver: "bridge".to_owned(),
            scope: "local".to_owned(),
        };
        storage.save_network(&net).unwrap();
        let loaded = storage.load_networks().unwrap();
        assert_eq!(loaded.len(), 1);
        let got = loaded.get("net1").unwrap();
        assert_eq!(got.name, "mynet");
        assert_eq!(got.driver, "bridge");
        assert_eq!(got.scope, "local");
    }

    #[test]
    fn test_storage_exec_save_update() {
        let storage = in_memory();
        let mut spec = make_exec("exec1", false);
        storage.save_exec(&spec).unwrap();

        spec.running = true;
        spec.exit_code = None;
        storage.update_exec(&spec).unwrap();

        spec.running = false;
        spec.exit_code = Some(0);
        storage.update_exec(&spec).unwrap();

        // Verify by saving another exec and re-saving the updated one
        let spec2 = make_exec("exec2", true);
        storage.save_exec(&spec2).unwrap();

        // reconcile will reset exec2
        let count = storage.reconcile_execs().unwrap();
        assert_eq!(count, 1, "only exec2 was running");
    }

    #[test]
    fn test_storage_reconcile_execs() {
        let storage = in_memory();
        let running = make_exec("running1", true);
        let not_running = make_exec("stopped1", false);
        storage.save_exec(&running).unwrap();
        storage.save_exec(&not_running).unwrap();

        let count = storage.reconcile_execs().unwrap();
        assert_eq!(count, 1, "one running exec reconciled");

        // Running exec should now be reset; verify by saving a new version with running=true
        // and reconciling again to see count drops to 0
        let count2 = storage.reconcile_execs().unwrap();
        assert_eq!(count2, 0, "no more running execs after reconcile");
    }

    #[test]
    fn test_storage_container_meta_roundtrip() {
        let storage = in_memory();
        let port_bindings: HashMap<String, Vec<PortBindingBody>> = {
            let mut m = HashMap::new();
            m.insert(
                "80/tcp".to_owned(),
                vec![PortBindingBody {
                    host_ip: Some("0.0.0.0".to_owned()),
                    host_port: Some("8080".to_owned()),
                }],
            );
            m
        };
        let pb_json = serde_json::to_string(&port_bindings).unwrap();

        storage
            .save_container_meta(
                "ctr1",
                "nginx:latest",
                r#"{"Image":"nginx:latest"}"#,
                &Some(pb_json),
                "2024-01-01T00:00:00Z",
            )
            .unwrap();

        let loaded = storage.load_port_bindings("ctr1").unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        let binding = loaded.get("80/tcp").unwrap();
        assert_eq!(binding.len(), 1);
        assert_eq!(binding[0].host_port.as_deref(), Some("8080"));
        assert_eq!(binding[0].host_ip.as_deref(), Some("0.0.0.0"));
    }

    #[test]
    fn test_storage_volume_delete() {
        let storage = in_memory();
        let vol = VolumeSummary {
            name: "todel".to_owned(),
            driver: "local".to_owned(),
            mountpoint: "/tmp/todel".to_owned(),
            created_at: None,
        };
        storage.save_volume(&vol).unwrap();
        storage.delete_volume("todel").unwrap();
        let loaded = storage.load_volumes().unwrap();
        assert!(loaded.is_empty());
    }
}
