use core_domain::{AppError, AppResult, ConnectionProfile, DatabaseKind};
use driver_api::{ConnectionHandle, ConnectionProvider};
use driver_mysql::MySqlDriver;
use driver_postgres::PostgresDriver;
use ssh_tunnel::SshTunnelManager;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::sleep;

fn pool_key(profile_id: &str, database: Option<&str>) -> String {
    match database {
        Some(db) => format!("{profile_id}::{db}"),
        None => format!("{profile_id}::__no_db__"),
    }
}

/// 连接池 —— 缓存已建立的连接，按 profile + database 独立缓存，
/// 复用前 ping 验证，不健康自动重连。
pub struct ConnectionPool {
    entries: Arc<std::sync::Mutex<HashMap<String, Arc<AsyncMutex<ConnectionHandle>>>>>,
    pub postgres: PostgresDriver,
    pub mysql: MySqlDriver,
    ssh_tunnel: SshTunnelManager,
    keepalive_secs: u64,
}

impl ConnectionPool {
    pub fn new(keepalive_secs: u64) -> Self {
        Self {
            entries: Arc::new(std::sync::Mutex::new(HashMap::new())),
            postgres: PostgresDriver,
            mysql: MySqlDriver,
            ssh_tunnel: SshTunnelManager,
            keepalive_secs,
        }
    }

    fn provider(&self, kind: DatabaseKind) -> &dyn ConnectionProvider {
        match kind {
            DatabaseKind::Postgres => &self.postgres,
            DatabaseKind::MySql => &self.mysql,
        }
    }

    /// 获取一个健康的连接句柄。database 影响 pool key，确保不同 database 的连接独立缓存。
    pub async fn acquire(
        &self,
        profile: &ConnectionProfile,
        password: &str,
        database: Option<&str>,
    ) -> AppResult<Arc<AsyncMutex<ConnectionHandle>>> {
        self.ssh_tunnel
            .validate(profile.ssh_tunnel.as_ref())
            .map_err(|e| AppError::Validation(e.to_string()))?;

        let provider = self.provider(profile.kind);
        let key = pool_key(&profile.id, database);

        let cached = self.entries.lock().unwrap().get(&key).cloned();
        drop(self.entries.lock().unwrap());

        if let Some(handle) = cached {
            let mut guard = handle.lock().await;
            if provider.ping(&mut guard).await.is_ok() {
                return Ok(handle.clone());
            }
            drop(guard);
            self.entries.lock().unwrap().remove(&key);
        }

        let new = provider.connect(profile, password, database).await?;
        let handle = Arc::new(AsyncMutex::new(new));
        self.entries
            .lock()
            .unwrap()
            .insert(key, handle.clone());
        Ok(handle)
    }

    fn entry_snapshot(&self) -> Vec<(String, Arc<AsyncMutex<ConnectionHandle>>)> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// 驱逐某 connection 的所有 db 缓存
    pub fn evict(&self, connection_id: &str) {
        self.entries
            .lock()
            .unwrap()
            .retain(|k, _| !k.starts_with(connection_id));
    }

    pub fn disconnect_all(&self) {
        self.entries.lock().unwrap().clear();
    }

    pub fn start_keepalive(self: &Arc<Self>) {
        if tokio::runtime::Handle::try_current().is_ok() {
            let pool = Arc::downgrade(self);
            let interval = self.keepalive_secs;
            tokio::spawn(async move {
                loop {
                    sleep(std::time::Duration::from_secs(interval)).await;
                    let Some(pool) = pool.upgrade() else { break };
                    pool.keepalive_pass().await;
                }
            });
        }
    }

    async fn keepalive_pass(&self) {
        for (key, handle) in self.entry_snapshot() {
            let provider: &dyn ConnectionProvider = {
                let guard = handle.lock().await;
                if guard.is_postgres() {
                    &self.postgres
                } else {
                    &self.mysql
                }
            };
            let mut guard = handle.lock().await;
            if provider.ping(&mut guard).await.is_err() {
                drop(guard);
                self.entries.lock().unwrap().remove(&key);
            }
        }
    }
}
