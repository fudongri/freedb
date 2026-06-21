/// SessionManager —— 管理数据库会话，内置连接池、自动重试、后台 keepalive。
///
/// 每次数据库操作都通过 acquire → 复用缓存连接 → ping 健康检查 流程。
/// 遇到 transient 错误（连接断开、超时等）自动 exponential backoff 重试（最多 3 次）。
/// 后台每 60s 对所有缓存连接做 ping，清理死连接。
use connection_pool::ConnectionPool;
use core_domain::{
    AppError, AppResult, ConnectionProfile, ConnectionState, DatabaseKind, ExplorerNode,
    QueryExecution, QueryResult, RetryConfig, TableChangeSet, TableDefinition, TableRef,
};
use driver_api::DatabaseDriver;
use parking_lot::RwLock;
use ssh_tunnel::SshTunnelManager;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

#[derive(Debug, Clone)]
pub struct SessionStatus {
    pub state: ConnectionState,
    pub last_error: Option<String>,
}

#[derive(Clone)]
pub struct SessionManager {
    pool: Arc<ConnectionPool>,
    ssh_tunnel: SshTunnelManager,
    statuses: Arc<RwLock<HashMap<String, SessionStatus>>>,
    retry: RetryConfig,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::with_retry_config(RetryConfig::default())
    }
}

impl SessionManager {
    pub fn with_retry_config(retry: RetryConfig) -> Self {
        let pool = Arc::new(ConnectionPool::new(retry.keepalive_interval_secs));
        Self {
            pool,
            ssh_tunnel: SshTunnelManager,
            statuses: Arc::new(RwLock::new(HashMap::new())),
            retry,
        }
    }

    /// 启动 keepalive —— 在 tokio runtime 启动后调用
    pub fn start_keepalive(&self) {
        self.pool.start_keepalive();
    }

    fn driver(&self, kind: DatabaseKind) -> &dyn DatabaseDriver {
        match kind {
            DatabaseKind::MySql => &self.pool.mysql,
            DatabaseKind::Postgres => &self.pool.postgres,
        }
    }

    fn backoff_ms(&self, attempt: u32) -> u64 {
        self.retry
            .base_delay_ms
            .saturating_mul(2u64.saturating_pow(attempt.saturating_sub(1)))
            .min(self.retry.max_delay_ms)
    }

    fn is_retryable(&self, error: &AppError) -> bool {
        error.is_transient()
    }

    // ── 公共 API ──

    pub async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: &str,
    ) -> AppResult<()> {
        self.ssh_tunnel
            .validate(profile.ssh_tunnel.as_ref())
            .map_err(|e| AppError::Validation(e.to_string()))?;
        let result = self
            .driver(profile.kind)
            .test_connection(profile, password)
            .await;
        self.store_status(profile.id.clone(), &result);
        result
    }

    pub async fn load_connection_tree(
        &self,
        profile: &ConnectionProfile,
        password: &str,
    ) -> AppResult<Vec<ExplorerNode>> {
        let mut last_err = None;
        for attempt in 0..=self.retry.max_retries {
            if attempt > 0 {
                sleep(Duration::from_millis(self.backoff_ms(attempt))).await;
                self.set_reconnecting(&profile.id, last_err.as_ref());
            }
            let handle = match self.pool.acquire(profile, password, None).await {
                Ok(h) => h,
                Err(e) => { last_err = Some(e); continue; }
            };
            let result = {
                let guard = handle.lock().await;
                let d: &dyn DatabaseDriver = if guard.is_postgres() {
                    &self.pool.postgres
                } else {
                    &self.pool.mysql
                };
                d.list_roots(profile, password).await
            };
            match result {
                Ok(nodes) => { self.set_connected(&profile.id); return Ok(nodes); }
                Err(e) if self.is_retryable(&e) => {
                    self.pool.evict(&profile.id);
                    last_err = Some(e);
                }
                Err(e) => { self.set_failed(&profile.id, &e); return Err(e); }
            }
        }
        let err = last_err.unwrap_or_else(|| AppError::Connection("retry exhausted".into()));
        self.set_failed(&profile.id, &err);
        Err(err)
    }

    pub async fn load_node_children(
        &self,
        profile: &ConnectionProfile,
        password: &str,
        node: &ExplorerNode,
    ) -> AppResult<Vec<ExplorerNode>> {
        let mut last_err = None;
        for attempt in 0..=self.retry.max_retries {
            if attempt > 0 {
                sleep(Duration::from_millis(self.backoff_ms(attempt))).await;
                self.set_reconnecting(&profile.id, last_err.as_ref());
            }
            let handle = match self.pool.acquire(profile, password, None).await {
                Ok(h) => h,
                Err(e) => { last_err = Some(e); continue; }
            };
            let result = {
                let guard = handle.lock().await;
                let d: &dyn DatabaseDriver = if guard.is_postgres() {
                    &self.pool.postgres
                } else {
                    &self.pool.mysql
                };
                d.list_children(profile, password, node).await
            };
            match result {
                Ok(nodes) => { self.set_connected(&profile.id); return Ok(nodes); }
                Err(e) if self.is_retryable(&e) => {
                    self.pool.evict(&profile.id);
                    last_err = Some(e);
                }
                Err(e) => { self.set_failed(&profile.id, &e); return Err(e); }
            }
        }
        let err = last_err.unwrap_or_else(|| AppError::Connection("retry exhausted".into()));
        self.set_failed(&profile.id, &err);
        Err(err)
    }

    pub async fn load_table_definition(
        &self,
        profile: &ConnectionProfile,
        password: &str,
        table: &TableRef,
    ) -> AppResult<TableDefinition> {
        let mut last_err = None;
        let db = table.database.clone().or_else(|| profile.default_database.clone());
        for attempt in 0..=self.retry.max_retries {
            if attempt > 0 { sleep(Duration::from_millis(self.backoff_ms(attempt))).await; }
            let handle = match self.pool.acquire(profile, password, db.as_deref()).await {
                Ok(h) => h,
                Err(e) => { last_err = Some(e); continue; }
            };
            let result = {
                let guard = handle.lock().await;
                let d: &dyn DatabaseDriver = if guard.is_postgres() {
                    &self.pool.postgres
                } else {
                    &self.pool.mysql
                };
                d.load_table_definition(profile, password, table).await
            };
            match result {
                Ok(def) => return Ok(def),
                Err(e) if self.is_retryable(&e) => {
                    self.pool.evict(&profile.id);
                    last_err = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err.unwrap_or_else(|| AppError::Connection("retry exhausted".into())))
    }

    pub async fn preview_table(
        &self,
        profile: &ConnectionProfile,
        password: &str,
        table: &TableRef,
        limit: u32,
    ) -> AppResult<QueryResult> {
        let mut last_err = None;
        let db = table.database.clone().or_else(|| profile.default_database.clone());
        for attempt in 0..=self.retry.max_retries {
            if attempt > 0 { sleep(Duration::from_millis(self.backoff_ms(attempt))).await; }
            let handle = match self.pool.acquire(profile, password, db.as_deref()).await {
                Ok(h) => h,
                Err(e) => { last_err = Some(e); continue; }
            };
            let result = {
                let guard = handle.lock().await;
                let d: &dyn DatabaseDriver = if guard.is_postgres() {
                    &self.pool.postgres
                } else {
                    &self.pool.mysql
                };
                d.preview_table(profile, password, table, limit).await
            };
            match result {
                Ok(r) => return Ok(r),
                Err(e) if self.is_retryable(&e) => {
                    self.pool.evict(&profile.id);
                    last_err = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err.unwrap_or_else(|| AppError::Connection("retry exhausted".into())))
    }

    pub async fn execute_sql(
        &self,
        profile: &ConnectionProfile,
        password: &str,
        execution: QueryExecution,
    ) -> AppResult<QueryResult> {
        let mut last_err = None;
        let db = execution.database.clone().or_else(|| profile.default_database.clone());
        for attempt in 0..=self.retry.max_retries {
            if attempt > 0 {
                sleep(Duration::from_millis(self.backoff_ms(attempt))).await;
                self.set_reconnecting(&profile.id, last_err.as_ref());
            }
            let handle = match self.pool.acquire(profile, password, db.as_deref()).await {
                Ok(h) => h,
                Err(e) => { last_err = Some(e); continue; }
            };
            let result = {
                let guard = handle.lock().await;
                let d: &dyn DatabaseDriver = if guard.is_postgres() {
                    &self.pool.postgres
                } else {
                    &self.pool.mysql
                };
                d.execute_sql(profile, password, execution.clone()).await
            };
            match result {
                Ok(r) => { self.set_connected(&profile.id); return Ok(r); }
                Err(e) if self.is_retryable(&e) => {
                    self.pool.evict(&profile.id);
                    last_err = Some(e);
                }
                Err(e) => { self.set_failed(&profile.id, &e); return Err(e); }
            }
        }
        let err = last_err.unwrap_or_else(|| AppError::Connection("retry exhausted".into()));
        self.set_failed(&profile.id, &err);
        Err(err)
    }

    pub async fn apply_table_changes(
        &self,
        profile: &ConnectionProfile,
        password: &str,
        changes: TableChangeSet,
    ) -> AppResult<QueryResult> {
        let mut last_err = None;
        let db = changes.table.database.clone().or_else(|| profile.default_database.clone());
        for attempt in 0..=self.retry.max_retries {
            if attempt > 0 { sleep(Duration::from_millis(self.backoff_ms(attempt))).await; }
            let handle = match self.pool.acquire(profile, password, db.as_deref()).await {
                Ok(h) => h,
                Err(e) => { last_err = Some(e); continue; }
            };
            let result = {
                let guard = handle.lock().await;
                let d: &dyn DatabaseDriver = if guard.is_postgres() {
                    &self.pool.postgres
                } else {
                    &self.pool.mysql
                };
                d.apply_table_changes(profile, password, changes.clone()).await
            };
            match result {
                Ok(r) => return Ok(r),
                Err(e) if self.is_retryable(&e) => {
                    self.pool.evict(&profile.id);
                    last_err = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err.unwrap_or_else(|| AppError::Connection("retry exhausted".into())))
    }

    pub fn disconnect_connection(&self, connection_id: &str) {
        self.pool.evict(connection_id);
        self.statuses.write().insert(
            connection_id.to_string(),
            SessionStatus { state: ConnectionState::Disconnected, last_error: None },
        );
    }

    pub fn disconnect_all(&self) {
        self.pool.disconnect_all();
        self.statuses.write().clear();
    }

    pub fn connection_status(&self, connection_id: &str) -> SessionStatus {
        self.statuses.read().get(connection_id).cloned().unwrap_or(SessionStatus {
            state: ConnectionState::Disconnected, last_error: None,
        })
    }

    fn store_status<T>(&self, id: String, result: &AppResult<T>) {
        self.statuses.write().insert(id, session_from_result(result));
    }

    fn set_connected(&self, id: &str) {
        self.statuses.write().insert(id.to_string(), SessionStatus {
            state: ConnectionState::Connected, last_error: None,
        });
    }

    fn set_failed(&self, id: &str, error: &AppError) {
        self.statuses.write().insert(id.to_string(), SessionStatus {
            state: ConnectionState::Failed, last_error: Some(error.to_string()),
        });
    }

    fn set_reconnecting(&self, id: &str, error: Option<&AppError>) {
        self.statuses.write().insert(id.to_string(), SessionStatus {
            state: ConnectionState::Reconnecting, last_error: error.map(|e| e.to_string()),
        });
    }
}

fn session_from_result<T>(result: &AppResult<T>) -> SessionStatus {
    match result {
        Ok(_) => SessionStatus { state: ConnectionState::Connected, last_error: None },
        Err(e) => SessionStatus { state: ConnectionState::Failed, last_error: Some(e.to_string()) },
    }
}
