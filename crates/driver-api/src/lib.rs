use async_trait::async_trait;
use core_domain::{AppResult, ConnectionProfile};

/// 持久化连接句柄——被连接池缓存
pub enum ConnectionHandle {
    Postgres {
        client: tokio_postgres::Client,
        connection: tokio::task::JoinHandle<()>,
    },
    MySql {
        conn: mysql_async::Conn,
    },
}

impl ConnectionHandle {
    pub fn is_postgres(&self) -> bool {
        matches!(self, Self::Postgres { .. })
    }
}

/// 连接池需要的基础操作
#[async_trait]
pub trait ConnectionProvider: Send + Sync {
    async fn connect(
        &self,
        profile: &ConnectionProfile,
        password: &str,
        database: Option<&str>,
    ) -> AppResult<ConnectionHandle>;

    async fn ping(&self, handle: &mut ConnectionHandle) -> AppResult<()>;
}

/// 数据库操作 trait —— 与之前完全相同
#[async_trait]
pub trait DatabaseDriver: Send + Sync {
    async fn test_connection(&self, profile: &ConnectionProfile, password: &str) -> AppResult<()>;
    async fn list_roots(
        &self,
        profile: &ConnectionProfile,
        password: &str,
    ) -> AppResult<Vec<core_domain::ExplorerNode>>;
    async fn list_children(
        &self,
        profile: &ConnectionProfile,
        password: &str,
        parent: &core_domain::ExplorerNode,
    ) -> AppResult<Vec<core_domain::ExplorerNode>>;
    async fn load_table_definition(
        &self,
        profile: &ConnectionProfile,
        password: &str,
        table: &core_domain::TableRef,
    ) -> AppResult<core_domain::TableDefinition>;
    async fn preview_table(
        &self,
        profile: &ConnectionProfile,
        password: &str,
        table: &core_domain::TableRef,
        limit: u32,
    ) -> AppResult<core_domain::QueryResult>;
    async fn execute_sql(
        &self,
        handle: &mut ConnectionHandle,
        profile: &ConnectionProfile,
        password: &str,
        execution: core_domain::QueryExecution,
    ) -> AppResult<core_domain::QueryResult>;
    async fn apply_table_changes(
        &self,
        profile: &ConnectionProfile,
        password: &str,
        changes: core_domain::TableChangeSet,
    ) -> AppResult<core_domain::QueryResult>;

    // DDL operations
    async fn create_database(&self, profile: &ConnectionProfile, password: &str, name: &str, charset: Option<&str>, collation: Option<&str>) -> AppResult<()>;
    async fn rename_database(&self, profile: &ConnectionProfile, password: &str, old_name: &str, new_name: &str) -> AppResult<()>;
    async fn drop_database(&self, profile: &ConnectionProfile, password: &str, name: &str) -> AppResult<()>;
    async fn create_schema(&self, profile: &ConnectionProfile, password: &str, database: &str, name: &str) -> AppResult<()>;
    async fn rename_schema(&self, profile: &ConnectionProfile, password: &str, database: &str, old_name: &str, new_name: &str) -> AppResult<()>;
    async fn drop_schema(&self, profile: &ConnectionProfile, password: &str, database: &str, name: &str) -> AppResult<()>;
    async fn rename_table(&self, profile: &ConnectionProfile, password: &str, database: &str, schema: Option<&str>, old_name: &str, new_name: &str) -> AppResult<()>;

    // Dump support
    async fn dump_table_all_data(&self, profile: &ConnectionProfile, password: &str, table: &core_domain::TableRef) -> AppResult<core_domain::QueryResult>;
}
