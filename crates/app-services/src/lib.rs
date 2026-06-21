use anyhow::{anyhow, Result};
use chrono::Utc;
use connection_store::ConnectionStore;
use core_domain::{
    AppError, ConnectionProfile, ConnectionProfileInput, ExplorerNode, QueryExecution, QueryResult,
    SavedQueryEntry, TableChangeSet, TableDefinition, TableRef, UiStateValue,
};
use export_service::ExportService;
use history_store::HistoryStore;
use secure_store::SecureStore;
use session_manager::{SessionManager, SessionStatus};
use std::path::Path;
use tracing::warn;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppServices {
    connection_store: ConnectionStore,
    history_store: HistoryStore,
    secure_store: SecureStore,
    export_service: ExportService,
    session_manager: SessionManager,
}

impl AppServices {
    pub fn new() -> Result<Self> {
        Ok(Self {
            connection_store: ConnectionStore::new()?,
            history_store: HistoryStore::new()?,
            secure_store: SecureStore,
            export_service: ExportService,
            session_manager: SessionManager::default(),
        })
    }

    pub fn list_connections(&self) -> Result<Vec<ConnectionProfile>> {
        let mut profiles = self.connection_store.list_connections()?;
        for profile in &mut profiles {
            profile.password_saved = self
                .secure_store
                .load_password(&profile.id)?
                .is_some();
        }
        Ok(profiles)
    }

    pub fn save_connection(&self, input: ConnectionProfileInput) -> Result<ConnectionProfile> {
        validate_connection_input(&input)?;
        let password = input.password.clone();
        let profile = ConnectionProfile::from_input(Uuid::new_v4().to_string(), input);
        if profile.password_saved {
            if let Some(password) = password {
                self.secure_store.save_password(&profile.id, &password)?;
            }
        } else {
            self.secure_store.delete_password(&profile.id)?;
        }
        self.connection_store.save_connection(&profile)?;
        Ok(profile)
    }

    pub fn update_connection(
        &self,
        connection_id: &str,
        input: ConnectionProfileInput,
    ) -> Result<ConnectionProfile> {
        validate_connection_input(&input)?;
        let mut profile = self
            .connection_store
            .get_connection(connection_id)?
            .ok_or_else(|| anyhow!("connection not found"))?;
        profile.name = input.name;
        profile.kind = input.kind;
        profile.group_name = input.group_name;
        profile.host = input.host;
        profile.port = input.port;
        profile.username = input.username;
        profile.default_database = input.default_database;
        profile.password_saved = input.save_password;
        profile.connect_timeout_secs = input.connect_timeout_secs;
        profile.ssl_mode = input.ssl_mode;
        profile.ssh_tunnel = input.ssh_tunnel;
        profile.updated_at = Utc::now();

        if profile.password_saved {
            if let Some(password) = input.password {
                self.secure_store.save_password(&connection_id, &password)?;
            }
        } else {
            self.secure_store.delete_password(&connection_id)?;
        }
        self.connection_store.update_connection(&profile)?;
        Ok(profile)
    }

    pub fn delete_connection(&self, connection_id: &str) -> Result<()> {
        self.connection_store.delete_connection(connection_id)?;
        self.secure_store.delete_password(connection_id)?;
        self.session_manager.disconnect_connection(connection_id);
        Ok(())
    }

    pub async fn test_connection(&self, input: ConnectionProfileInput) -> Result<()> {
        validate_connection_input(&input)?;
        let password = input
            .password
            .clone()
            .ok_or_else(|| anyhow!("测试连接需要密码"))?;
        let mut profile = ConnectionProfile::from_input("test-connection".into(), input);
        profile.password_saved = false;
        self.session_manager
            .test_connection(&profile, &password)
            .await
            .map_err(into_anyhow)
    }

    pub async fn load_connection_tree(&self, connection_id: &str) -> Result<Vec<ExplorerNode>> {
        let profile = self.require_connection(connection_id)?;
        let password = self.require_saved_password(connection_id)?;
        let nodes = self
            .session_manager
            .load_connection_tree(&profile, &password)
            .await
            .map_err(into_anyhow)?;
        if let Err(error) = self.connection_store.set_last_used_at(connection_id, Utc::now()) {
            warn!(
                connection_id = connection_id,
                error = %error,
                "failed to persist connection last_used_at"
            );
        }
        Ok(nodes)
    }

    pub async fn list_databases(&self, connection_id: &str) -> Result<Vec<String>> {
        let nodes = self.load_connection_tree(connection_id).await?;
        let databases: Vec<String> = nodes
            .into_iter()
            .filter(|n| n.node_type == core_domain::ExplorerNodeType::Database)
            .map(|n| n.name)
            .collect();
        Ok(databases)
    }

    pub async fn load_node_children(
        &self,
        connection_id: &str,
        node: &ExplorerNode,
    ) -> Result<Vec<ExplorerNode>> {
        let profile = self.require_connection(connection_id)?;
        let password = self.require_saved_password(connection_id)?;
        self.session_manager
            .load_node_children(&profile, &password, node)
            .await
            .map_err(into_anyhow)
    }

    pub async fn search_objects(
        &self,
        connection_id: &str,
        keyword: &str,
    ) -> Result<Vec<ExplorerNode>> {
        let roots = self.load_connection_tree(connection_id).await?;
        let mut matches = Vec::new();
        for root in roots {
            if root.name.to_ascii_lowercase().contains(&keyword.to_ascii_lowercase()) {
                matches.push(root.clone());
            }
            let children = self.load_node_children(connection_id, &root).await.unwrap_or_default();
            for child in children {
                if child.name.to_ascii_lowercase().contains(&keyword.to_ascii_lowercase()) {
                    matches.push(child.clone());
                }
                if child.expandable {
                    let grandchildren = self
                        .load_node_children(connection_id, &child)
                        .await
                        .unwrap_or_default();
                    for grandchild in grandchildren {
                        if grandchild
                            .name
                            .to_ascii_lowercase()
                            .contains(&keyword.to_ascii_lowercase())
                        {
                            matches.push(grandchild);
                        }
                    }
                }
            }
        }
        Ok(matches)
    }

    pub async fn load_table_definition(&self, table: &TableRef) -> Result<TableDefinition> {
        let profile = self.require_connection(&table.connection_id)?;
        let password = self.require_saved_password(&table.connection_id)?;
        self.session_manager
            .load_table_definition(&profile, &password, table)
            .await
            .map_err(into_anyhow)
    }

    pub async fn open_table_preview(&self, table: &TableRef, limit: u32) -> Result<QueryResult> {
        let profile = self.require_connection(&table.connection_id)?;
        let password = self.require_saved_password(&table.connection_id)?;
        self.session_manager
            .preview_table(&profile, &password, table, limit)
            .await
            .map_err(into_anyhow)
    }

    pub async fn execute_sql(&self, execution: QueryExecution) -> Result<QueryResult> {
        let profile = self.require_connection(&execution.connection_id)?;
        let password = self.require_saved_password(&execution.connection_id)?;
        let result = self
            .session_manager
            .execute_sql(&profile, &password, execution.clone())
            .await
            .map_err(into_anyhow)?;
        if let Err(error) = self.history_store.append(&execution.connection_id, &execution.sql) {
            warn!(
                connection_id = execution.connection_id.as_str(),
                error = %error,
                "failed to persist query history"
            );
        }
        Ok(result)
    }

    pub async fn apply_table_changes(&self, changes: TableChangeSet) -> Result<QueryResult> {
        let profile = self.require_connection(&changes.table.connection_id)?;
        let password = self.require_saved_password(&changes.table.connection_id)?;
        self.session_manager
            .apply_table_changes(&profile, &password, changes)
            .await
            .map_err(into_anyhow)
    }

    pub fn list_query_history(&self, connection_id: &str) -> Result<Vec<String>> {
        Ok(self
            .history_store
            .list_by_connection(connection_id, 50)?
            .into_iter()
            .map(|entry| entry.sql_text)
            .collect())
    }

    pub fn save_query(
        &self,
        connection_id: &str,
        database: Option<&str>,
        title: &str,
        sql_text: &str,
    ) -> Result<SavedQueryEntry> {
        let title = title.trim();
        let sql_text = sql_text.trim();
        if sql_text.is_empty() {
            return Err(anyhow!("没有可保存的 SQL"));
        }
        let title = if title.is_empty() {
            build_saved_query_title(sql_text)
        } else {
            title.to_string()
        };
        let record = self.history_store.save_query(connection_id, database, &title, sql_text)?;
        Ok(SavedQueryEntry {
            id: record.id,
            connection_id: record.connection_id,
            database: record.database,
            title: record.title,
            sql_text: record.sql_text,
            saved_at: record.saved_at,
        })
    }

    pub fn list_saved_queries(&self, connection_id: &str) -> Result<Vec<SavedQueryEntry>> {
        Ok(self
            .history_store
            .list_saved_queries(connection_id, 100)?
            .into_iter()
            .map(|record| SavedQueryEntry {
                id: record.id,
                connection_id: record.connection_id,
                database: record.database,
                title: record.title,
                sql_text: record.sql_text,
                saved_at: record.saved_at,
            })
            .collect())
    }

    pub fn rename_saved_query(&self, id: &str, title: &str) -> Result<()> {
        let title = title.trim();
        if title.is_empty() {
            return Err(anyhow!("查询名称不能为空"));
        }
        self.history_store.rename_saved_query(id, title)
    }

    pub fn delete_saved_query(&self, id: &str) -> Result<()> {
        self.history_store.delete_saved_query(id)
    }

    pub fn export_query_result_csv(
        &self,
        result: &QueryResult,
        path: impl AsRef<Path>,
    ) -> Result<()> {
        self.export_service.export_query_result_csv(result, path)
    }

    pub fn disconnect_connection(&self, connection_id: &str) {
        self.session_manager.disconnect_connection(connection_id);
    }

    /// 在 tokio runtime 启动后调用
    pub fn start_keepalive(&self) {
        self.session_manager.start_keepalive();
    }

    pub fn connection_status(&self, connection_id: &str) -> SessionStatus {
        self.session_manager.connection_status(connection_id)
    }

    pub fn save_ui_state(&self, key: &str, value: &str) -> Result<()> {
        if let Err(error) = self.connection_store.save_ui_state(UiStateValue {
            key: key.to_string(),
            value: value.to_string(),
        }) {
            warn!(key = key, error = %error, "failed to persist ui state");
        }
        Ok(())
    }

    pub fn load_ui_state(&self, key: &str) -> Result<Option<String>> {
        self.connection_store.load_ui_state(key)
    }

    pub fn update_sort_orders(&self, orders: &[(String, i64)]) -> Result<()> {
        self.connection_store.update_sort_orders(orders)
    }

    pub fn load_password(&self, connection_id: &str) -> Result<Option<String>> {
        self.secure_store.load_password(connection_id)
    }

    fn require_connection(&self, connection_id: &str) -> Result<ConnectionProfile> {
        self.connection_store
            .get_connection(connection_id)?
            .ok_or_else(|| anyhow!("connection not found"))
    }

    fn require_saved_password(&self, connection_id: &str) -> Result<String> {
        self.secure_store
            .load_password(connection_id)?
            .ok_or_else(|| anyhow!("该连接未保存密码，请重新编辑连接后保存密码"))
    }
}

fn validate_connection_input(input: &ConnectionProfileInput) -> Result<()> {
    if input.name.trim().is_empty() {
        return Err(anyhow!("连接名称不能为空"));
    }
    if input.host.trim().is_empty() {
        return Err(anyhow!("主机地址不能为空"));
    }
    if input.username.trim().is_empty() {
        return Err(anyhow!("用户名不能为空"));
    }
    Ok(())
}

fn into_anyhow(error: AppError) -> anyhow::Error {
    anyhow!(error.to_string())
}

fn build_saved_query_title(sql_text: &str) -> String {
    let first_line = sql_text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("未命名查询");
    let compact = first_line.split_whitespace().collect::<Vec<_>>().join(" ");
    let char_count = compact.chars().count();
    if char_count > 36 {
        format!("{}...", compact.chars().take(36).collect::<String>())
    } else if compact.is_empty() {
        "未命名查询".into()
    } else {
        compact
    }
}
