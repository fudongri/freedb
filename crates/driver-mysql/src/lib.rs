use anyhow::Context;
use async_trait::async_trait;
use core_domain::{
    AppError, AppResult, ColumnDefinition, ConnectionProfile, ExplorerNode, ExplorerNodeType,
    QueryCellValue, QueryExecution, QueryResult, TableChangeSet, TableDefinition, TableRef,
};
use driver_api::{ConnectionHandle, ConnectionProvider, DatabaseDriver};
use mysql_async::{prelude::Queryable, Conn, OptsBuilder, Row, Value};
use std::collections::BTreeMap;
use std::time::Instant;

#[derive(Clone, Default)]
pub struct MySqlDriver;

#[async_trait]
impl ConnectionProvider for MySqlDriver {
    async fn connect(
        &self,
        profile: &ConnectionProfile,
        password: &str,
        database: Option<&str>,
    ) -> AppResult<ConnectionHandle> {
        let mut builder = OptsBuilder::default();
        builder = builder
            .ip_or_hostname(profile.host.clone())
            .tcp_port(profile.port)
            .user(Some(profile.username.clone()))
            .pass(Some(password.to_string()));
        if let Some(db) = database.or(profile.default_database.as_deref()) {
            builder = builder.db_name(Some(db.to_string()));
        }
        let conn = Conn::new(builder).await.map_err(map_mysql_error)?;
        Ok(ConnectionHandle::MySql { conn })
    }

    async fn ping(&self, handle: &mut ConnectionHandle) -> AppResult<()> {
        match handle {
            ConnectionHandle::MySql { conn } => {
                conn.ping().await.map_err(map_mysql_error)?;
                Ok(())
            }
            _ => Err(AppError::Validation("expected mysql handle".into())),
        }
    }
}

#[async_trait]
impl DatabaseDriver for MySqlDriver {
    async fn test_connection(&self, profile: &ConnectionProfile, password: &str) -> AppResult<()> {
        let mut conn = open_conn(profile, password, profile.default_database.as_deref()).await?;
        conn.ping().await.map_err(map_mysql_error)?;
        disconnect(conn).await;
        Ok(())
    }

    async fn list_roots(&self, profile: &ConnectionProfile, password: &str) -> AppResult<Vec<ExplorerNode>> {
        let mut conn = open_conn(profile, password, profile.default_database.as_deref()).await?;
        let dbs: Vec<String> = conn.query_map("SHOW DATABASES", |name: String| name).await.map_err(map_mysql_error)?;
        disconnect(conn).await;
        Ok(dbs.into_iter().map(|db| ExplorerNode {
            id: format!("mysql-db:{}:{db}", profile.id),
            connection_id: profile.id.clone(),
            name: db.clone(),
            node_type: ExplorerNodeType::Database,
            parent_id: None, database: Some(db), schema: None, expandable: true, loaded: false,
        }).collect())
    }

    async fn list_children(&self, profile: &ConnectionProfile, password: &str, parent: &ExplorerNode) -> AppResult<Vec<ExplorerNode>> {
        if matches!(parent.node_type, ExplorerNodeType::Connection) {
            return self.list_roots(profile, password).await;
        }
        let db = parent.database.as_ref().ok_or_else(|| AppError::Validation("missing database".into()))?;
        let mut conn = open_conn(profile, password, Some(db)).await?;
        let sql = format!("SHOW FULL TABLES FROM {}", quote_mysql(db));
        let rows: Vec<Row> = conn.query(sql).await.map_err(map_mysql_error)?;
        disconnect(conn).await;
        Ok(rows.into_iter().map(|row| {
            let name = row.get::<String, _>(0).unwrap_or_default();
            let kind = row.get::<String, _>(1).unwrap_or_else(|| "BASE TABLE".into());
            let is_view = kind.to_ascii_uppercase().contains("VIEW");
            ExplorerNode {
                id: format!("mysql-table:{}:{db}:{name}", profile.id),
                connection_id: profile.id.clone(),
                name: name.clone(),
                node_type: if is_view { ExplorerNodeType::View } else { ExplorerNodeType::Table },
                parent_id: Some(parent.id.clone()),
                database: Some(db.clone()),
                schema: None, expandable: false, loaded: true,
            }
        }).collect())
    }

    async fn load_table_definition(&self, profile: &ConnectionProfile, password: &str, table: &TableRef) -> AppResult<TableDefinition> {
        let db = table.database.as_ref().ok_or_else(|| AppError::Validation("mysql table requires database".into()))?;
        let mut conn = open_conn(profile, password, Some(db)).await?;
        let sql = format!(
            "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_KEY FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_SCHEMA = '{}' AND TABLE_NAME = '{}' ORDER BY ORDINAL_POSITION",
            escape_mysql_literal(db), escape_mysql_literal(&table.table),
        );
        let rows: Vec<Row> = conn.query(sql).await.map_err(map_mysql_error)?;
        let columns = rows.into_iter().map(|row| ColumnDefinition {
            name: row.get::<String, _>(0).unwrap_or_default(),
            data_type: row.get::<String, _>(1).unwrap_or_default(),
            nullable: row.get::<String, _>(2).map(|v| v.eq_ignore_ascii_case("YES")).unwrap_or(true),
            primary_key: row.get::<String, _>(3).map(|v| v.eq_ignore_ascii_case("PRI")).unwrap_or(false),
        }).collect();

        let create_sql = if table.is_view {
            let sql = format!("SHOW CREATE VIEW {}.{}", quote_mysql(db), quote_mysql(&table.table));
            conn.query(sql).await.map_err(map_mysql_error).ok()
                .and_then(|rows: Vec<Row>| rows.into_iter().next())
                .and_then(|row| row.get::<String, _>(1).or_else(|| row.get::<String, _>(0)))
        } else {
            let sql = format!("SHOW CREATE TABLE {}.{}", quote_mysql(db), quote_mysql(&table.table));
            conn.query(sql).await.map_err(map_mysql_error).ok()
                .and_then(|rows: Vec<Row>| rows.into_iter().next())
                .and_then(|row| row.get::<String, _>(1).or_else(|| row.get::<String, _>(0)))
        };
        disconnect(conn).await;
        Ok(TableDefinition { columns, create_sql })
    }

    async fn preview_table(&self, profile: &ConnectionProfile, password: &str, table: &TableRef, limit: u32) -> AppResult<QueryResult> {
        let db = table.database.as_ref().ok_or_else(|| AppError::Validation("mysql table requires database".into()))?;
        let sql = format!("SELECT * FROM {}.{} LIMIT {}", quote_mysql(db), quote_mysql(&table.table), limit);
        let mut conn = open_conn(profile, password, Some(db)).await?;
        let result = query_rows(&mut conn, &sql).await;
        disconnect(conn).await;
        result
    }

    async fn execute_sql(&self, profile: &ConnectionProfile, password: &str, execution: QueryExecution) -> AppResult<QueryResult> {
        let mut conn = open_conn(profile, password, execution.database.as_deref().or(profile.default_database.as_deref())).await?;
        let result = exec_on_conn(&mut conn, execution).await;
        disconnect(conn).await;
        result
    }

    async fn apply_table_changes(&self, _profile: &ConnectionProfile, _password: &str, _changes: TableChangeSet) -> AppResult<QueryResult> {
        Err(AppError::Unsupported("MySQL 表格编辑将在后续迭代中补全".into()))
    }
}

// ── helpers ──

fn open_conn(profile: &ConnectionProfile, password: &str, database: Option<&str>) -> impl std::future::Future<Output = AppResult<Conn>> {
    let mut builder = OptsBuilder::default();
    builder = builder.ip_or_hostname(profile.host.clone()).tcp_port(profile.port).user(Some(profile.username.clone())).pass(Some(password.to_string()));
    if let Some(db) = database {
        builder = builder.db_name(Some(db.to_string()));
    }
    async move { Conn::new(builder).await.map_err(map_mysql_error) }
}

async fn exec_on_conn(conn: &mut Conn, execution: QueryExecution) -> AppResult<QueryResult> {
    let start = Instant::now();
    let sql = execution.sql.trim().to_string();
    if let Some(ref db) = execution.database {
        let lower = sql.to_ascii_lowercase();
        if !lower.starts_with("use ") {
            conn.query_drop(format!("USE {}", quote_mysql(db))).await.map_err(map_mysql_error)?;
        }
    }
    let lower = sql.to_ascii_lowercase();
    if lower.starts_with("select") || lower.starts_with("show") || lower.starts_with("desc") || lower.starts_with("describe") || lower.starts_with("explain") {
        query_rows(conn, &sql).await
    } else {
        conn.query_drop(sql).await.map_err(map_mysql_error)?;
        Ok(QueryResult { columns: Vec::new(), rows: Vec::new(), affected_rows: Some(conn.affected_rows()), elapsed_ms: start.elapsed().as_millis(), message: Some("语句执行成功".into()) })
    }
}

async fn query_rows(conn: &mut Conn, sql: &str) -> AppResult<QueryResult> {
    let start = Instant::now();
    let rows: Vec<Row> = conn.query(sql).await.map_err(map_mysql_error)?;
    let columns = rows.first().map(|row| row.columns_ref().iter().map(|c| c.name_str().to_string()).collect::<Vec<_>>()).unwrap_or_default();
    let mapped = rows.iter().map(|row| {
        let mut m = BTreeMap::new();
        for (i, col) in columns.iter().enumerate() {
            m.insert(col.clone(), row.as_ref(i).map(mysql_cell).unwrap_or(QueryCellValue::Null));
        }
        m
    }).collect();
    Ok(QueryResult { columns, rows: mapped, affected_rows: None, elapsed_ms: start.elapsed().as_millis(), message: None })
}

fn mysql_cell(value: &Value) -> QueryCellValue {
    match value {
        Value::NULL => QueryCellValue::Null,
        Value::Bytes(b) => String::from_utf8_lossy(b).to_string().into(),
        Value::Int(v) => v.to_string().into(),
        Value::UInt(v) => v.to_string().into(),
        Value::Float(v) => v.to_string().into(),
        Value::Double(v) => v.to_string().into(),
        Value::Date(y, m, d, hh, mm, ss, us) => format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}.{us:06}").into(),
        Value::Time(neg, days, h, m, s, us) => format!("{}{days} {h:02}:{m:02}:{s:02}.{us:06}", if *neg { "-" } else { "" }).into(),
    }
}

fn quote_mysql(s: &str) -> String { format!("`{}`", s.replace('`', "``")) }
fn escape_mysql_literal(s: &str) -> String { s.replace('\\', "\\\\").replace('\'', "\\'") }
fn map_mysql_error(e: mysql_async::Error) -> AppError { AppError::Connection(e.to_string()) }
async fn disconnect(conn: Conn) { let _ = conn.disconnect().await.context("disconnect mysql"); }
